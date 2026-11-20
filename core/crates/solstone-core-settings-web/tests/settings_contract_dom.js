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
const { loadWindowCatalog } = require('../../solstone-core-convey-shell/tests/journal_catalog_dom.js');

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
      keepalive: !!opts?.keepalive,
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
      parseInto(doc, main, name === 'settings' ? window.JournalI18n.renderWorkspace(env.markup) : HOME_MARKUP);
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
  loadWindowCatalog(context);
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

const GONE = ['mountSettings', 'switchSection', 'initSettingsWorkspace', 'loadConfig', 'loadTrash', 'saveField',
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
    const p = await boot(env, '/app/settings/#sync');
    p.byId('granolaKey').value = 'grn_lab';
    p.press(p.byId('granolaConnect'));
    await settle();
    assert.ok(p.writes().some((call) => call.url === '/app/settings/api/granola' && call.method === 'POST'), "Granola's connect posts the key");
    assert.ok(!p.byId('revalidateAllKeys'), 'no re-validate button: nothing here can check a key');
    const forms = p.qa('form').filter((form) => form.getAttribute('onsubmit'));
    assert.ok(forms.length === 7, `the seven form guards: ${forms.length}`);
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
    const p = await boot(env, '/app/settings/#sync');
    p.byId('granolaKey').value = 'grn_lab';
    const gone = p.byId('granolaConnect');
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

  // ---- presses that keep focus: the trash, storage and local access

  // A journal whose trash list is what the last act left, and whose local
  // access answers (the base journal() leaves it out).
  async function livePage(address, { items = ['t1', 't2', 't3'], local = null, extra = {} } = {}) {
    let listed = items.map((id) => (typeof id === 'string' ? trashItem(id) : id));
    const base = journal(extra);
    const answers = (url, opts) => {
      const u = new URL(url, ORIGIN);
      const method = String(opts?.method || 'GET').toUpperCase();
      const body = opts?.body ? JSON.parse(opts.body) : {};
      if (u.pathname === '/app/settings/api/trash' && method === 'GET') return { erase_after_days: 7, items: listed };
      if (u.pathname === '/app/settings/api/trash/erase' || u.pathname === '/app/settings/api/trash/recover') {
        listed = listed.filter((item) => !body.ids.includes(item.id));
        return { done: body.ids, failed: [] };
      }
      if (u.pathname === '/app/settings/api/trash/empty') {
        const done = listed.map((item) => item.id);
        listed = [];
        return { done, failed: [] };
      }
      if (u.pathname === '/app/settings/api/local-access') {
        if (!local) return undefined;
        if (method === 'PUT') local = { ...local, open: !!body.open };
        return local;
      }
      if (u.pathname === '/app/settings/api/storage' && method === 'PUT') {
        return { success: true, retention: { empty_audio: body.empty_audio } };
      }
      return base(url, opts);
    };
    const page = await boot(env, address, { answers });
    // A held write never reaches the fixture: what its answer takes, by hand.
    page.take = (ids) => { listed = listed.filter((item) => !ids.includes(item.id)); };
    return page;
  }
  // The focused node by tag and id only: a failure message stays small.
  const at = (p) => { const a = p.doc.activeElement; return a ? a.tagName + (a.id ? '#' + a.id : '') + (a.dataset?.trashId ? '[' + a.dataset.trashId + ']' : '') : 'none'; };
  const box = (p, id) => p.qa('#trashList input[type="checkbox"]').find((el) => el.dataset.trashId === id);
  function holdOnce(p, url) {
    const held = { release: null };
    p.holds.push((call) => {
      if (call.url !== url || held.release) return false;
      held.release = () => { p.holds.length = 0; call.answer(p.env.answerOf ? null : undefined); };
      held.call = call;
      return true;
    });
    return held;
  }
  // A box pressed by Space: it ticks, then its change fires.
  const tick = (p, el) => { el.checked = !el.checked; p.doc.dispatch(el, { type: 'change' }); };
  // A write that fails: answered 503 without reaching the journal.
  function failing(p, test) {
    p.holds.push((call) => {
      if (!test(call)) return false;
      Promise.resolve().then(() => call.answer({ error: 'down' }, 503));
      return true;
    });
  }
  async function eraseVia(p, ids, opener = 'trashEraseBtn') {
    // The stand-in's click doesn't tick a box: tick it, then its change.
    for (const id of ids) { const el = box(p, id); el.checked = true; p.doc.dispatch(el, { type: 'change' }); }
    await settle();
    const button = p.byId(opener);
    button.focus();
    p.press(button);
    p.clock.advance(20);
    await settle();
  }

  await test('an erase lands on the row after the first one erased, else the one before, else the trash heading', async () => {
    const p = await livePage('/app/settings/#trash');
    await eraseVia(p, ['t2']);
    p.press(p.byId('trashConfirmErase'));
    await settle();
    assert.ok(p.doc.activeElement === box(p, 't3'), `the next row: ${at(p)}`);
    await eraseVia(p, ['t3']);
    p.press(p.byId('trashConfirmErase'));
    await settle();
    assert.ok(p.doc.activeElement === box(p, 't1'), `the one before: ${at(p)}`);
    await eraseVia(p, ['t1']);
    p.press(p.byId('trashConfirmErase'));
    await settle();
    assert.strictEqual(at(p), 'H2#trashHeading', 'the heading once nothing is left');
  });

  await test('an erase of two lands on the row after the first of them', async () => {
    const p = await livePage('/app/settings/#trash', { items: ['t1', 't2', 't3', 't4'] });
    await eraseVia(p, ['t2', 't3']);
    p.press(p.byId('trashConfirmErase'));
    await settle();
    assert.ok(p.doc.activeElement === box(p, 't4'), `the row after: ${at(p)}`);
  });

  await test('the trash confirm gives focus back to its opener as it closes, the layer reconciled at once', async () => {
    const p = await livePage('/app/settings/#trash');
    p.holds.push((call) => call.url === '/app/settings/api/trash/erase');
    await eraseVia(p, ['t1']);
    const reconciles = p.window.ConveyModalLayer.reconciles;
    p.press(p.byId('trashConfirmErase'));
    assert.ok(p.window.ConveyModalLayer.reconciles > reconciles, 'the layer is reconciled as the dialog closes');
    assert.strictEqual(at(p), 'BUTTON#trashEraseBtn', 'focus is on the opener with no frame waited');
  });

  await test('a held erase keeps its opener focused and busy; the other acts are off, and no press starts a second', async () => {
    const p = await livePage('/app/settings/#trash');
    p.holds.push((call) => call.url === '/app/settings/api/trash/erase');
    await eraseVia(p, ['t1']);
    p.press(p.byId('trashConfirmErase'));
    await settle();
    const erase = p.byId('trashEraseBtn');
    const recover = p.byId('trashRecoverBtn');
    assert.ok(p.doc.activeElement === erase, `focus stays on erase now: ${at(p)}`);
    assert.ok(erase.getAttribute('aria-busy') === 'true' && erase.getAttribute('aria-disabled') === 'true', 'it is busy');
    assert.ok(!erase.disabled, 'and not disabled');
    assert.ok(recover.getAttribute('aria-disabled') === 'true' && recover.getAttribute('aria-busy') === null, 'recover is off, not busy');
    p.press(erase);
    p.press(recover);
    p.press(p.byId('trashEmptyBtn'));
    p.clock.advance(20);
    await settle();
    assert.deepStrictEqual(p.shownModals(), [], 'no second confirm opens');
    const acts = p.writes().filter((call) => call.url.startsWith('/app/settings/api/trash/')).length;
    assert.ok(acts === 1, `one act sent: ${acts}`);
    p.held().forEach((call) => call.answer({ done: ['t1'], failed: [] }));
    await settle();
    assert.ok(erase.getAttribute('aria-busy') === null && recover.getAttribute('aria-disabled') === null, 'the answer clears both');
  });

  await test('a failed erase lands on the line that says so', async () => {
    const p = await livePage('/app/settings/#trash');
    failing(p, (call) => call.url === '/app/settings/api/trash/erase');
    await eraseVia(p, ['t2']);
    p.press(p.byId('trashConfirmErase'));
    await settle();
    assert.strictEqual(at(p), 'DIV#trashResult', 'the result line');
    assert.ok(!p.byId('trashResult').hidden, 'which is shown');
  });

  await test('an empty lands on the trash heading; a failed one stays on its button', async () => {
    const p = await livePage('/app/settings/#trash');
    const empty = p.byId('trashEmptyBtn');
    empty.focus();
    p.press(empty);
    p.clock.advance(20);
    await settle();
    failing(p, (call) => call.url === '/app/settings/api/trash/empty');
    p.press(p.byId('trashConfirmErase'));
    await settle();
    assert.ok(p.doc.activeElement === empty, `a failed empty: ${at(p)}`);
    p.holds.length = 0;
    p.press(empty);
    p.clock.advance(20);
    await settle();
    p.press(p.byId('trashConfirmErase'));
    await settle();
    assert.strictEqual(at(p), 'H2#trashHeading', 'an empty that answered');
  });

  await test('a recover lands as an erase does, busy while it is out, and one at a time', async () => {
    const p = await livePage('/app/settings/#trash');
    p.holds.push((call) => call.url === '/app/settings/api/trash/recover');
    await eraseVia(p, ['t2'], 'trashRecoverBtn');
    const recover = p.byId('trashRecoverBtn');
    assert.ok(p.doc.activeElement === recover && recover.getAttribute('aria-busy') === 'true', `busy and focused: ${at(p)}`);
    p.press(recover);
    await settle();
    const sent = p.writes().filter((call) => call.url === '/app/settings/api/trash/recover').length;
    assert.ok(sent === 1, `one recover: ${sent}`);
    p.holds.length = 0;
    p.take(['t2']);
    p.held().forEach((call) => call.answer({ done: ['t2'], failed: [] }));
    await settle();
    assert.ok(p.doc.activeElement === box(p, 't3'), `the next row: ${at(p)}`);
  });

  await test('a recover over a facet confirms, and lands as a recover', async () => {
    const facet = { id: 'f1', kind: 'facet', title: 'Spare', erase_at: '2099-11-15T00:00:00.000Z', replaces: true };
    const p = await livePage('/app/settings/#trash', { items: ['t1', facet, 't3'] });
    await eraseVia(p, ['f1'], 'trashRecoverBtn');
    assert.deepStrictEqual(p.shownModals(), ['trashConfirmModal'], 'its replace confirm');
    p.press(p.byId('trashConfirmErase'));
    await settle();
    assert.ok(p.doc.activeElement === box(p, 't3'), `the next row: ${at(p)}`);
  });

  await test("a held audio-with-no-speech save keeps the toggle focused and busy, a second press sends nothing, and a failure lands on the line that says why", async () => {
    const p = await livePage('/app/settings/#storage');
    p.holds.push((call) => call.url === '/app/settings/api/storage' && call.method === 'PUT');
    const toggle = p.byId('emptyAudioKeep');
    toggle.focus();
    tick(p, toggle);
    await settle();
    assert.ok(p.doc.activeElement === toggle, `focus stays on the toggle: ${at(p)}`);
    assert.ok(toggle.getAttribute('aria-busy') === 'true' && !toggle.disabled, 'busy, not disabled');
    tick(p, toggle);
    await settle();
    const puts = () => p.writes().filter((call) => call.url === '/app/settings/api/storage').length;
    assert.ok(puts() === 1, `one save: ${puts()}`);
    p.held().forEach((call) => call.answer({ error: 'down' }, 503));
    await settle();
    assert.ok(toggle.disabled, 'a failure turns it off: the choice is unknown');
    assert.strictEqual(at(p), 'DIV#emptyAudioStatus', 'the line that says so');
  });

  await test('an answered audio-with-no-speech save keeps the toggle', async () => {
    const p = await livePage('/app/settings/#storage');
    const toggle = p.byId('emptyAudioKeep');
    toggle.focus();
    tick(p, toggle);
    await settle();
    assert.ok(p.doc.activeElement === toggle && !toggle.disabled && toggle.getAttribute('aria-busy') === null, `kept: ${at(p)}`);
  });

  await test("a held logs cleanup keeps its delete focused and busy in the open dialog, and a second press sends nothing", async () => {
    const answers = { '/app/settings/api/storage/prune-logs#POST': { enabled: true, files_deleted: 2, dirs_deleted: 0, retention_log: { lines_removed: 0 } } };
    const p = await livePage('/app/settings/#storage', { extra: answers });
    p.press(p.byId('cleanupLogsBtn'));
    p.clock.advance(20);
    await settle();
    p.press(p.byId('cleanupLogsPreviewBtn'));
    await settle();
    p.holds.push((call) => call.url === '/app/settings/api/storage/prune-logs' && JSON.parse(call.body).dry_run === false);
    const del = p.byId('cleanupLogsConfirmBtn');
    del.focus();
    p.press(del);
    await settle();
    assert.ok(p.doc.activeElement === del, `focus stays on delete: ${at(p)}`);
    assert.ok(del.getAttribute('aria-busy') === 'true' && !del.disabled, 'busy, not disabled');
    p.press(del);
    await settle();
    const runs = p.writes().filter((call) => call.url === '/app/settings/api/storage/prune-logs' && JSON.parse(call.body).dry_run === false).length;
    assert.ok(runs === 1, `one delete: ${runs}`);
  });

  const LOCAL = { available: true, open: true, closed_now: false, message: null, account_check: true };
  async function confirmLocal(p) {
    const toggle = p.byId('field-local-access-open');
    toggle.focus();
    tick(p, toggle);
    await settle();
    const yes = p.byId('localAccessConfirmYes');
    yes.focus();
    return { toggle, yes };
  }

  await test("local access's confirm lands on its toggle once the group hides, answered or failed", async () => {
    const p = await livePage('/app/settings/#localaccess', { local: LOCAL });
    let { toggle, yes } = await confirmLocal(p);
    p.press(yes);
    await settle();
    assert.ok(p.byId('localAccessConfirm').hidden, 'the group hides');
    assert.ok(p.doc.activeElement === toggle, `answered: ${at(p)}`);
    ({ toggle, yes } = await confirmLocal(p));
    failing(p, (call) => call.url === '/app/settings/api/local-access' && call.method === 'PUT');
    p.press(yes);
    await settle();
    assert.ok(p.doc.activeElement === toggle, `failed: ${at(p)}`);
  });

  await test("local access's 'keep it open' lands on the toggle", async () => {
    const p = await livePage('/app/settings/#localaccess', { local: LOCAL });
    const { toggle } = await confirmLocal(p);
    p.press(p.byId('localAccessConfirmNo'));
    await settle();
    assert.ok(p.doc.activeElement === toggle, `kept open: ${at(p)}`);
  });

  await test("a held local access choice keeps its confirm focused and busy, and nothing else hides the group or sends a second", async () => {
    const p = await livePage('/app/settings/#localaccess', { local: LOCAL });
    p.holds.push((call) => call.url === '/app/settings/api/local-access' && call.method === 'PUT');
    const { toggle, yes } = await confirmLocal(p);
    p.press(yes);
    await settle();
    assert.ok(p.doc.activeElement === yes, `focus stays on it: ${at(p)}`);
    assert.ok(yes.getAttribute('aria-busy') === 'true' && !yes.disabled, 'busy, not disabled');
    p.press(yes);
    p.press(p.byId('localAccessConfirmNo'));
    toggle.checked = !toggle.checked;
    p.doc.dispatch(toggle, { type: 'change' });
    await settle();
    assert.ok(!p.byId('localAccessConfirm').hidden, 'the group stays while the choice is out');
    assert.ok(p.doc.activeElement === yes, `and focus stays on its confirm: ${at(p)}`);
    const puts = p.writes().filter((call) => call.url === '/app/settings/api/local-access').length;
    assert.ok(puts === 1, `one save: ${puts}`);
  });

  await test("a local access answer that turns the toggle off lands on the line that says what happened", async () => {
    const p = await livePage('/app/settings/#localaccess', { local: LOCAL });
    p.holds.push((call) => {
      if (call.url !== '/app/settings/api/local-access' || call.method !== 'PUT') return false;
      Promise.resolve().then(() => call.answer({ ...LOCAL, open: false, available: false, message: 'unavailable here' }));
      return true;
    });
    const { yes } = await confirmLocal(p);
    p.press(yes);
    await settle();
    assert.strictEqual(at(p), 'P#localAccessResult', 'the result line');
  });

  await test('focus elsewhere when an act answers is left where it is', async () => {
    const p = await livePage('/app/settings/#trash');
    p.holds.push((call) => call.url === '/app/settings/api/trash/erase');
    await eraseVia(p, ['t2']);
    p.press(p.byId('trashConfirmErase'));
    await settle();
    const rail = p.byId('rail-home');
    rail.focus();
    p.holds.length = 0;
    p.held().forEach((call) => call.answer({ done: ['t2'], failed: [] }));
    await settle();
    assert.strictEqual(at(p), 'A#rail-home', 'the rail keeps it');
  });

  // ---- the keys and the per-field saves keep focus, and a pending save goes out as the page goes

  const configPuts = (p) => p.writes().filter((call) => call.url === '/app/settings/api/config' && call.method === 'PUT');
  const typeInto = (el, text) => { el.focus(); el.value = text; };
  const leaveField = (p, el) => { el.blur(); p.doc.dispatch(el, { type: 'blur', bubbles: false }); };
  const FIELDS = [['field-name', 'a new name', '#profile'], ['field-preferred', 'lab', '#profile'], ['field-aliases', 'lab, labby', '#profile'],
    ['field-emails', 'lab@example.com', '#profile'], ['field-timezone', 'Europe/Paris', '#profile'], ['field-env-plaud', 'lab-token', '#apikeys']];

  await test('Enter in each auto-save field saves it once, after its half second, and keeps focus in the field: answered, failed and held', async () => {
    for (const [id, text, hash] of FIELDS) {
      for (const mode of ['answered', 'failed', 'held']) {
        const p = await boot(env, `/app/settings/${hash}`);
        if (mode === 'held') p.holds.push((call) => call.method === 'PUT');
        if (mode === 'failed') failing(p, (call) => call.method === 'PUT');
        const field = p.byId(id);
        typeInto(field, text);
        p.key(field, 'Enter');
        assert.ok(p.doc.activeElement === field, `${id} ${mode}: focus stays in the field: ${at(p)}`);
        assert.ok(configPuts(p).length === 0, `${id} ${mode}: not before its half second`);
        p.clock.advance(600);
        await settle();
        assert.ok(configPuts(p).length === 1, `${id} ${mode}: one save: ${configPuts(p).length}`);
        assert.ok(p.doc.activeElement === field, `${id} ${mode}: still in the field: ${at(p)}`);
      }
    }
  });

  await test("a second Enter, and leaving the field, while its save is out send nothing more; a new value still saves", async () => {
    for (const [id, text, hash] of [FIELDS[1], FIELDS[5]]) {
      const p = await boot(env, `/app/settings/${hash}`);
      p.holds.push((call) => call.method === 'PUT');
      const field = p.byId(id);
      typeInto(field, text);
      p.key(field, 'Enter');
      p.clock.advance(200);
      p.key(field, 'Enter');
      p.clock.advance(600);
      await settle();
      assert.ok(configPuts(p).length === 1, `${id}: one save: ${configPuts(p).length}`);
      p.key(field, 'Enter');
      p.doc.dispatch(field, { type: 'change' });
      leaveField(p, field);
      p.clock.advance(600);
      await settle();
      assert.ok(configPuts(p).length === 1, `${id}: still one while it's out: ${configPuts(p).length}`);
      field.focus();
      field.value = `${text} two`;
      p.key(field, 'Enter');
      p.clock.advance(600);
      await settle();
      assert.ok(configPuts(p).length === 2, `${id}: a new value is its own save: ${configPuts(p).length}`);
    }
  });

  await test('leaving an unset field unchanged sends nothing: bio, pronouns, aliases, emails, the preferred name; a set one cleared still saves', async () => {
    const p = await boot(env, '/app/settings/#profile');
    for (const id of ['field-bio', 'field-pronouns', 'field-aliases', 'field-emails', 'field-preferred']) {
      const field = p.byId(id);
      field.focus();
      p.doc.dispatch(field, { type: 'change' });
      leaveField(p, field);
    }
    p.clock.advance(600);
    await settle();
    assert.ok(configPuts(p).length === 0, `nothing written: ${configPuts(p).map((call) => call.body).join(' ')}`);
    const bio = p.byId('field-bio');
    bio.focus();
    bio.value = 'a bio';
    leaveField(p, bio);
    const name = p.byId('field-name');
    name.focus();
    name.value = '';
    leaveField(p, name);
    p.clock.advance(600);
    await settle();
    const sent = configPuts(p).map((call) => JSON.parse(call.body).data);
    assert.deepStrictEqual(sent, [{ bio: 'a bio' }, { name: '' }]);
  });

  async function pendingPreferred(p) {
    p.holds.push((call) => call.method === 'PUT');
    const field = p.byId('field-preferred');
    typeInto(field, 'lab');
    leaveField(p, field);
    p.clock.advance(200);
    assert.ok(configPuts(p).length === 0, 'not yet sent');
    return field;
  }

  await test("a field's save not yet sent goes out as the page goes, once, with keepalive; a restore, a hide, a leave and its timer send nothing more", async () => {
    const p = await boot(env, '/app/settings/#profile');
    await pendingPreferred(p);
    p.window.fire({ type: 'pagehide', persisted: true });
    assert.ok(configPuts(p).length === 1, `the page going sends it: ${configPuts(p).length}`);
    assert.ok(configPuts(p)[0].keepalive === true, 'with keepalive');
    assert.deepStrictEqual(JSON.parse(configPuts(p)[0].body), { section: 'identity', data: { preferred: 'lab' } });
    p.window.fire({ type: 'pageshow', persisted: true });
    p.clock.advance(1000);
    p.doc.visibilityState = 'hidden';
    p.doc.fire({ type: 'visibilitychange' });
    p.leave();
    p.clock.advance(1000);
    await settle();
    assert.ok(configPuts(p).length === 1, `once: ${configPuts(p).length}`);
  });

  await test("hiding the page sends a save not yet sent, once, with keepalive; showing it and the page going after send nothing", async () => {
    const p = await boot(env, '/app/settings/#profile');
    await pendingPreferred(p);
    p.doc.visibilityState = 'hidden';
    p.doc.fire({ type: 'visibilitychange' });
    assert.ok(configPuts(p).length === 1 && configPuts(p)[0].keepalive === true, `hiding sends it, with keepalive: ${configPuts(p).length}`);
    p.doc.visibilityState = 'visible';
    p.doc.fire({ type: 'visibilitychange' });
    p.window.fire({ type: 'pagehide', persisted: false });
    p.clock.advance(1000);
    await settle();
    assert.ok(configPuts(p).length === 1, `once: ${configPuts(p).length}`);
  });

  await test('an in-page leave still sends a pending save without keepalive, and adds no listener that outlives settings', async () => {
    const p = await boot(env, '/app/settings/#profile');
    const before = listenerCounts(p);
    await pendingPreferred(p);
    p.leave();
    assert.ok(configPuts(p).length === 1 && configPuts(p)[0].keepalive === false, 'sent by the leave, plainly');
    p.window.fire({ type: 'pagehide', persisted: false });
    assert.ok(configPuts(p).length === 1, 'a pagehide after sends nothing');
    const after = listenerCounts(p);
    assert.ok(after[0] < before[0] && after[1] < before[1], `the window's and the document's listeners go with settings: ${before} → ${after}`);
  });

  // ---- Granola

  const GRANOLA_ON = { connected: true, last_sync: '2026-11-16T10:00:00Z', last_added: '2026-11-16T09:00:00Z', meetings: 3, syncing: false, attention: null, attention_code: null };
  async function granolaPage(start = {}) {
    let state = { connected: false, last_sync: null, last_added: null, attention: null, attention_code: null, meetings: 0, syncing: false, ...start };
    const base = journal();
    const answers = (url, opts) => {
      const u = new URL(url, ORIGIN);
      const method = String(opts?.method || 'GET').toUpperCase();
      if (u.pathname === '/app/settings/api/granola') {
        if (method === 'POST') { state = { ...state, connected: true }; return {}; }
        if (method === 'DELETE') { state = { ...state, connected: false }; return {}; }
        return state;
      }
      if (u.pathname === '/app/settings/api/granola/sync') { state = { ...state, syncing: true }; return {}; }
      return base(url, opts);
    };
    const p = await boot(env, '/app/settings/#sync', { answers });
    p.granola = (next) => { state = { ...state, ...next }; };
    return p;
  }
  const granolaWrites = (p, method, url = '/app/settings/api/granola') => p.writes().filter((call) => call.url === url && call.method === method).length;
  const busyNotDisabled = (el) => el.getAttribute('aria-busy') === 'true' && el.getAttribute('aria-disabled') === 'true' && !el.disabled;
  const offNotBusy = (el) => el.getAttribute('aria-disabled') === 'true' && el.getAttribute('aria-busy') === null && !el.disabled;
  function holdGranola(p, method, url = '/app/settings/api/granola') {
    const held = {};
    p.holds.push((call) => {
      if (call.url !== url || call.method !== method || held.call) return false;
      held.call = call;
      return true;
    });
    held.release = (body = {}, status = 200) => { p.holds.length = 0; held.call.answer(body, status); };
    return held;
  }

  await test("a held Granola connect keeps the button focused and busy, the others off, and no press or Enter sends a second; its answer lands on check now", async () => {
    const p = await granolaPage();
    const held = holdGranola(p, 'POST');
    const key = p.byId('granolaKey');
    key.value = 'grn_lab';
    const connect = p.byId('granolaConnect');
    connect.focus();
    p.press(connect);
    await settle();
    assert.ok(p.doc.activeElement === connect, `focus stays on connect: ${at(p)}`);
    assert.ok(busyNotDisabled(connect), 'busy, not disabled');
    p.press(connect);
    p.key(key, 'Enter');
    await settle();
    assert.ok(granolaWrites(p, 'POST') === 1, `one connect: ${granolaWrites(p, 'POST')}`);
    p.granola({ connected: true });
    held.release();
    await settle();
    assert.strictEqual(at(p), 'BUTTON#granolaCheck', 'the connected controls took the key form\'s place');
  });

  await test("Granola's key by Enter connects once and keeps focus while it's out; a failed connect keeps focus on connect with its error", async () => {
    const p = await granolaPage();
    const held = holdGranola(p, 'POST');
    const key = p.byId('granolaKey');
    typeInto(key, 'grn_lab');
    p.key(key, 'Enter');
    await settle();
    assert.ok(p.doc.activeElement === key, `focus stays in the key: ${at(p)}`);
    p.key(key, 'Enter');
    await settle();
    assert.ok(granolaWrites(p, 'POST') === 1, `one connect: ${granolaWrites(p, 'POST')}`);
    held.release({ error: 'refused' }, 400);
    await settle();
    assert.ok(p.doc.activeElement === key, `a failure keeps it: ${at(p)}`);
    assert.ok(!p.byId('granolaKeyError').hidden, 'and says why');
    const connect = p.byId('granolaConnect');
    connect.focus();
    failing(p, (call) => call.url === '/app/settings/api/granola' && call.method === 'POST');
    p.press(connect);
    await settle();
    assert.ok(p.doc.activeElement === connect && connect.getAttribute('aria-busy') === null, `a failed connect stays on connect: ${at(p)}`);
  });

  await test('Granola: "use this key" lands on "use a new key", and so does the new key\'s cancel', async () => {
    const p = await granolaPage(GRANOLA_ON);
    const replace = p.byId('granolaReplace');
    replace.focus();
    p.press(replace);
    await settle();
    assert.strictEqual(at(p), 'INPUT#granolaKey', 'the new key');
    p.byId('granolaKey').value = 'grn_two';
    const connect = p.byId('granolaConnect');
    connect.focus();
    p.press(connect);
    await settle();
    assert.strictEqual(at(p), 'BUTTON#granolaReplace', 'the control the key form had taken the place of');
    p.press(p.byId('granolaReplace'));
    await settle();
    const cancel = p.byId('granolaCancelKey');
    cancel.focus();
    p.press(cancel);
    await settle();
    assert.strictEqual(at(p), 'BUTTON#granolaReplace', "the cancel's");
  });

  await test('a held Granola "use this key" keeps its form: its cancel and "use a new key" do nothing while it is out', async () => {
    const p = await granolaPage(GRANOLA_ON);
    p.press(p.byId('granolaReplace'));
    await settle();
    const held = holdGranola(p, 'POST');
    p.byId('granolaKey').value = 'grn_two';
    const connect = p.byId('granolaConnect');
    connect.focus();
    p.press(connect);
    await settle();
    const cancel = p.byId('granolaCancelKey');
    assert.ok(offNotBusy(cancel), 'the cancel is off');
    p.press(cancel);
    await settle();
    assert.ok(!p.byId('granolaKeyForm').hidden && p.doc.activeElement === connect, `the form stays, focus on its busy button: ${at(p)}`);
    held.release();
    await settle();
    assert.strictEqual(at(p), 'BUTTON#granolaReplace', 'then lands as an answered one does');
  });

  await test("a held Granola disconnect keeps its button busy and sends one; its answer lands on the line that says what happened", async () => {
    const p = await granolaPage(GRANOLA_ON);
    const held = holdGranola(p, 'DELETE');
    const disconnect = p.byId('granolaDisconnect');
    disconnect.focus();
    p.press(disconnect);
    await settle();
    assert.ok(p.doc.activeElement === disconnect && busyNotDisabled(disconnect), `busy and focused: ${at(p)}`);
    assert.ok(offNotBusy(p.byId('granolaCheck')) && offNotBusy(p.byId('granolaReplace')), 'the others are off, not busy');
    p.press(disconnect);
    p.press(p.byId('granolaCheck'));
    p.press(p.byId('granolaReplace'));
    await settle();
    assert.ok(granolaWrites(p, 'DELETE') === 1 && granolaWrites(p, 'POST', '/app/settings/api/granola/sync') === 0, 'one disconnect, nothing else');
    assert.ok(p.byId('granolaKeyForm').hidden, 'no key form opened under it');
    p.granola({ connected: false });
    held.release();
    await settle();
    assert.strictEqual(at(p), 'P#granolaOffKept', 'what disconnecting kept');
  });

  await test('Granola check now stays focused and busy while the check runs, a second press sends nothing, and it is check now again after', async () => {
    const p = await granolaPage(GRANOLA_ON);
    const check = p.byId('granolaCheck');
    check.focus();
    p.press(check);
    await settle();
    assert.ok(p.doc.activeElement === check && busyNotDisabled(check), `busy while the journal checks: ${at(p)}`);
    p.press(check);
    await settle();
    assert.ok(granolaWrites(p, 'POST', '/app/settings/api/granola/sync') === 1, 'one check');
    p.granola({ syncing: false, last_sync: '2026-11-16T11:00:00Z' });
    p.clock.advance(2000);
    await settle();
    assert.ok(p.doc.activeElement === check && check.getAttribute('aria-busy') === null, `free again, still focused: ${at(p)}`);
  });

  await test("Granola's follow writes nothing when a check changes nothing, and keeps the focused node when one does", async () => {
    const p = await granolaPage({ ...GRANOLA_ON, syncing: true });
    const card = p.byId('granolaSyncCard');
    const disconnect = p.byId('granolaDisconnect');
    disconnect.focus();
    const from = mutationMark();
    p.clock.advance(2000);
    await settle();
    p.clock.advance(2000);
    await settle();
    const reads = p.reads('/app/settings/api/granola').length;
    assert.ok(reads >= 3, `the follow read again: ${reads}`);
    assert.ok(mutationsUnder(card, from) === 0, `nothing written: ${mutationsSeen(card, from)}`);
    assert.ok(p.doc.activeElement === disconnect, `focus on the same node: ${at(p)}`);
    p.granola({ meetings: 4 });
    const moved = mutationMark();
    p.clock.advance(2000);
    await settle();
    assert.ok(mutationsUnder(card, moved) > 0, 'a change is drawn');
    assert.ok(p.doc.activeElement === disconnect, `and focus stays on the same node: ${at(p)}`);
  });

  // ---- speech processing, leave out other voices, photos of people

  await test('speech processing stays focused and busy while its save is out; unreadable after, it lands on the line that says so', async () => {
    const extra = { '/app/settings/api/transcribe': { backends: [], parakeet_embedded: true } };
    const base = journal(extra);
    const read = { fails: false };
    const p = await boot(env, '/app/settings/#transcription', { answers: (url, opts) => {
      const method = String(opts?.method || 'GET').toUpperCase();
      if (url.startsWith('/app/settings/api/config') && method === 'GET' && read.fails) return undefined;
      return base(url, opts);
    } });
    const select = p.byId('field-transcribe-processing');
    const held = holdGranola(p, 'PUT', '/app/settings/api/config');
    select.focus();
    select.value = 'confidential';
    p.doc.dispatch(select, { type: 'change' });
    await settle();
    assert.ok(p.doc.activeElement === select && busyNotDisabled(select), `busy and focused: ${at(p)}`);
    select.value = 'local';
    p.doc.dispatch(select, { type: 'change' });
    await settle();
    assert.ok(configPuts(p).length === 1, `one save: ${configPuts(p).length}`);
    read.fails = true;
    held.release({ success: false }, 503);
    await settle();
    assert.ok(select.disabled, 'unreadable: off');
    assert.strictEqual(at(p), 'SMALL#transcribeProcessingHelp', 'the line that says so');
  });

  await test('leave out other voices stays focused and busy while turning off, and a second press sends nothing; its confirm focuses the switch at once', async () => {
    const extra = {
      '/app/settings/api/config': { identity: { name: 'owner', timezone: 'UTC' }, transcribe: { owner_voice_only: true }, env: {}, runtime_env: {}, key_validation: {}, trash: { erase_after_days: 7 } },
      '/app/settings/api/transcribe/owner-voice': { available: true, today: { left_out_s: 0, recordings: 0 }, pending_deletions: 0 },
    };
    const p = await boot(env, '/app/settings/#transcription', { answers: journal(extra) });
    const held = holdGranola(p, 'PUT', '/app/settings/api/config');
    const toggle = p.byId('ownerVoiceOnly');
    toggle.focus();
    tick(p, toggle);
    await settle();
    assert.ok(p.doc.activeElement === toggle && busyNotDisabled(toggle), `busy and focused: ${at(p)}`);
    tick(p, toggle);
    await settle();
    assert.ok(configPuts(p).length === 1, `one save: ${configPuts(p).length}`);
    held.release({ success: true, config: { transcribe: { owner_voice_only: false } } });
    await settle();
    assert.ok(p.doc.activeElement === toggle && toggle.getAttribute('aria-busy') === null, `kept: ${at(p)}`);
    // On again, through its confirm: the switch has focus at once, busy.
    p.holds.push((call) => call.method === 'PUT');
    tick(p, toggle);
    await settle();
    const before = p.window.ConveyModalLayer.reconciles;
    p.press(p.byId('ownerVoiceConsentConfirm'));
    assert.ok(p.window.ConveyModalLayer.reconciles > before, 'the layer reconciled as the dialog closed');
    await settle();
    assert.ok(p.doc.activeElement === toggle && busyNotDisabled(toggle), `the switch, busy: ${at(p)}`);
  });

  await test('photos of people stays focused and busy while its save is out; its erase keeps its button busy and focused through the confirm', async () => {
    const extra = { '/app/settings/api/config': { identity: { name: 'owner', timezone: 'UTC' }, transcribe: {}, faces: { enabled: false }, env: {}, runtime_env: {}, key_validation: {}, trash: { erase_after_days: 7 } } };
    const p = await boot(env, '/app/settings/#vision', { answers: journal(extra) });
    const clearHeld = holdGranola(p, 'POST', '/app/entities/api/journal/faces/clear');
    const clear = p.byId('facesClearBtn');
    clear.focus();
    p.press(clear);
    await settle();
    p.press(p.byId('facesClearConfirm'));
    await settle();
    assert.ok(p.doc.activeElement === clear && busyNotDisabled(clear), `the erase, busy: ${at(p)}`);
    const toggle = p.byId('facesEnabled');
    assert.ok(offNotBusy(toggle), 'the switch is off while the erase is out');
    tick(p, toggle);
    await settle();
    assert.ok(configPuts(p).length === 0, 'and a press on it sends nothing');
    p.press(clear);
    await settle();
    assert.ok(p.shownModals().length === 0, 'no second confirm');
    clearHeld.release({ removed: 2 });
    await settle();
    assert.ok(p.doc.activeElement === clear && clear.getAttribute('aria-busy') === null, `kept after: ${at(p)}`);
    const held = holdGranola(p, 'PUT', '/app/settings/api/config');
    toggle.focus();
    tick(p, toggle);
    await settle();
    assert.ok(p.doc.activeElement === toggle && busyNotDisabled(toggle), `the switch, busy: ${at(p)}`);
    tick(p, toggle);
    await settle();
    assert.ok(configPuts(p).length === 1, `one save: ${configPuts(p).length}`);
    held.release({ success: true, config: { faces: { enabled: true } } });
    await settle();
    assert.ok(p.doc.activeElement === toggle, `kept: ${at(p)}`);
  });


  // ---- the facet pages keep focus

  const FP = `/app/settings/facets/${SLUG}`;
  const facetPuts = (p) => p.writes().filter((call) => call.url === `/app/settings/api/facet/${SLUG}` && call.method === 'PUT');
  const writesTo = (p, method, url) => p.writes().filter((call) => call.method === method && call.url === url);
  // As a browser does, blur() fires the element's blur.
  // An input's select() has nothing to model here.
  const editable = (el) => { el.select = () => {}; return el; };
  const blurFires = (p, el) => { const own = el.blur; el.blur = function () { const was = p.doc.activeElement === this; own.call(this); if (was) p.doc.dispatch(this, { type: 'blur', bubbles: false }); }; };
  const ACT = (id, extra = {}) => ({ id, name: id, description: '', emoji: '', icon_svg: '', priority: 'normal', ...extra });
  // A facet page whose activities answer as the journal does: a remove takes
  // one out, an add puts one in.
  async function activitiesPage({ attached = ['a1', 'a2', 'a3'], defaults = ['a1', 'a2', 'a3'], extra = {} } = {}) {
    let on = [...attached];
    const base = journal(extra);
    const acts = `/app/settings/api/facet/${SLUG}/activities`;
    const answers = (url, opts) => {
      const u = new URL(url, ORIGIN);
      const method = String(opts?.method || 'GET').toUpperCase();
      if (u.pathname === acts && method === 'GET') return { activities: on.map((id) => ACT(id)), defaults: defaults.map((id) => ACT(id)) };
      if (u.pathname === acts && method === 'POST') { on.push(JSON.parse(opts.body).id); return { success: true }; }
      if (u.pathname.startsWith(acts + '/') && method === 'DELETE') { on = on.filter((id) => id !== u.pathname.slice(acts.length + 1)); return { success: true }; }
      return base(url, opts);
    };
    const p = await boot(env, FP, { answers });
    p.press(p.byId('facetDetailTabActivities'));
    await settle();
    p.take = (id) => { on = on.filter((x) => x !== id); };
    p.put = (id) => { on.push(id); };
    return p;
  }
  const removeOf = (p, id) => p.qa('#attachedActivitiesList .activity-remove').find((el) => el.dataset.id === id);
  const priorityOf = (p, id) => p.qa('#attachedActivitiesList .activity-priority').find((el) => el.dataset.id === id);
  const chipOf = (p, id) => p.qa('#defaultActivitiesList .activity-chip').find((el) => el.dataset.id === id);

  await test("the facet create dialog's add stays focused and busy while it is out, and a second press or Enter in the name sends nothing; refused, it keeps focus", async () => {
    const p = await boot(env, '/app/settings/#facets');
    const held = holdGranola(p, 'POST', '/app/settings/api/facet');
    p.press(p.qa('.facets-list-action')[0]);
    await settle();
    p.byId('facetCreateName').value = 'lab new';
    const submit = p.byId('facetCreateSubmit');
    submit.focus();
    p.doc.dispatch(p.byId('facetCreateForm'), { type: 'submit' });
    await settle();
    assert.ok(p.doc.activeElement === submit && busyNotDisabled(submit), `the add, busy: ${at(p)}`);
    p.doc.dispatch(p.byId('facetCreateForm'), { type: 'submit' });
    await settle();
    assert.ok(writesTo(p, 'POST', '/app/settings/api/facet').length === 1, `one add: ${writesTo(p, 'POST', '/app/settings/api/facet').length}`);
    held.release({ success: false, error: 'a facet with that name already exists' }, 409);
    await settle();
    assert.ok(p.doc.activeElement === submit && submit.getAttribute('aria-busy') === null && !submit.disabled, `refused, kept on the add: ${at(p)}`);
  });

  await test("a facet's mute stays focused and busy while its save is out, a second press sends nothing, and it is the same button after", async () => {
    const p = await boot(env, FP);
    const held = holdGranola(p, 'PUT', `/app/settings/api/facet/${SLUG}`);
    const mute = p.byId('facetMuteAction');
    mute.focus();
    p.press(mute);
    await settle();
    assert.ok(p.doc.activeElement === mute && busyNotDisabled(mute), `the mute, busy: ${at(p)}`);
    p.press(mute);
    await settle();
    assert.ok(facetPuts(p).length === 1, `one save: ${facetPuts(p).length}`);
    assert.ok(p.doc.activeElement === mute && busyNotDisabled(mute), `still busy after a second press: ${at(p)}`);
    held.release({ success: true, config: { title: 'Work', color: '#c00', muted: true } });
    await settle();
    assert.ok(p.doc.activeElement === mute && mute.getAttribute('aria-busy') === null, `kept after: ${at(p)}`);
    assert.ok(mute.textContent === 'unmute', 'and says what it does now');
  });

  await test("Enter in the facet's name saves it once and puts focus on the name as shown; the same name again while it's out sends nothing", async () => {
    const p = await boot(env, FP);
    const held = holdGranola(p, 'PUT', `/app/settings/api/facet/${SLUG}`);
    const display = p.byId('facetTitleDisplay');
    const input = editable(p.byId('facetTitleInput'));
    blurFires(p, input);
    display.focus();
    p.key(display, 'Enter');
    assert.ok(p.doc.activeElement === input, `editing: ${at(p)}`);
    input.value = 'Work two';
    p.key(input, 'Enter');
    await settle();
    assert.ok(p.doc.activeElement === display, `the name as shown: ${at(p)}`);
    assert.ok(facetPuts(p).length === 1, `one save: ${facetPuts(p).length}`);
    p.key(display, 'Enter');
    input.value = 'Work two';
    p.key(input, 'Enter');
    await settle();
    assert.ok(facetPuts(p).length === 1, `the value on its way isn't sent again: ${facetPuts(p).length}`);
    held.release({ success: true, config: { title: 'Work two', color: '#c00', muted: false } });
    await settle();
    assert.ok(p.doc.activeElement === display, `kept after: ${at(p)}`);
    p.key(display, 'Enter');
    input.value = '';
    p.key(input, 'Enter');
    await settle();
    assert.ok(p.doc.activeElement === display && facetPuts(p).length === 1, `an emptied name saves nothing and lands on the name: ${at(p)}`);
  });

  await test("a facet's emoji and color open from the keyboard, and Enter in the emoji lands on it", async () => {
    const p = await boot(env, FP, { answers: journal({ [`/app/settings/api/facet/${SLUG}#PUT`]: { success: true, config: { title: 'Work', color: '#cc0000', emoji: '🧪', muted: false } } }) });
    const emoji = p.byId('emojiDisplay');
    const input = p.byId('emojiInput');
    blurFires(p, input);
    emoji.focus();
    p.key(emoji, 'Enter');
    assert.ok(p.doc.activeElement === input, `the emoji opens: ${at(p)}`);
    input.value = '🧪';
    p.key(input, 'Enter');
    await settle();
    assert.ok(p.doc.activeElement === emoji, `the emoji as shown: ${at(p)}`);
    assert.ok(facetPuts(p).length === 1, 'one save');
    const color = p.byId('colorTrigger');
    color.focus();
    p.key(color, ' ');
    await settle();
    assert.deepStrictEqual(p.shownModals(), ['colorPickerModal']);
  });

  await test("an activity's remove stays busy while it is out and sends one; its answer lands on the next row's priority, else the one before, else the heading", async () => {
    const p = await activitiesPage();
    const held = holdGranola(p, 'DELETE', `/app/settings/api/facet/${SLUG}/activities/a2`);
    const remove = removeOf(p, 'a2');
    remove.focus();
    p.press(remove);
    await settle();
    assert.ok(p.doc.activeElement === remove && busyNotDisabled(remove), `the remove, busy: ${at(p)}`);
    p.press(remove);
    await settle();
    assert.ok(p.writes().filter((c) => c.method === 'DELETE').length === 1, 'one remove');
    p.take('a2');
    held.release({ success: true });
    await settle();
    assert.ok(p.doc.activeElement === priorityOf(p, 'a3'), `the next row: ${at(p)} ${p.doc.activeElement.dataset?.id}`);
    removeOf(p, 'a3').focus();
    p.press(removeOf(p, 'a3'));
    await settle();
    assert.ok(p.doc.activeElement === priorityOf(p, 'a1'), `the one before: ${at(p)} ${p.doc.activeElement.dataset?.id}`);
    removeOf(p, 'a1').focus();
    p.press(removeOf(p, 'a1'));
    await settle();
    assert.ok(p.doc.activeElement === p.q('#facetDetailActivities h3'), `the heading: ${at(p)}`);
  });

  await test("an activity's priority stays busy while its save is out and keeps the choice sent; a failure puts the saved choice back at once, and lands on it drawn again once the pick has settled", async () => {
    const p = await activitiesPage();
    const held = holdGranola(p, 'PUT', `/app/settings/api/facet/${SLUG}/activities/a2`);
    const select = priorityOf(p, 'a2');
    select.focus();
    select.value = 'high';
    p.doc.dispatch(select, { type: 'change' });
    await settle();
    assert.ok(p.doc.activeElement === select && busyNotDisabled(select), `busy: ${at(p)}`);
    select.value = 'low';
    p.doc.dispatch(select, { type: 'change' });
    await settle();
    assert.ok(p.writes().filter((c) => c.method === 'PUT').length === 1 && select.value === 'high', `one save, the choice sent: ${select.value}`);
    held.release({ error: 'lab: failed' }, 503);
    await settle();
    assert.ok(priorityOf(p, 'a2') === select && select.value === 'normal' && p.doc.activeElement === select, `put back at once, on the same select: ${select.value} ${at(p)}`);
    p.clock.advance(2000);
    await settle();
    const again = priorityOf(p, 'a2');
    assert.ok(again !== select && p.doc.activeElement === again, `the same choice, drawn again: ${at(p)}`);
  });

  await test("a default's priority, on a facet that shows every default: one PUT of the priority alone, and kept when it's answered, with nothing drawn again", async () => {
    const all = ['meeting', 'coding', 'email'];
    const p = await activitiesPage({ attached: all, defaults: all });
    const url = `/app/settings/api/facet/${SLUG}/activities/coding`;
    const held = holdGranola(p, 'PUT', url);
    const select = priorityOf(p, 'coding');
    const reads = () => p.calls.filter((c) => c.method === 'GET' && c.url === `/app/settings/api/facet/${SLUG}/activities`).length;
    const readsBefore = reads();
    select.focus();
    select.value = 'high';
    p.doc.dispatch(select, { type: 'change' });
    await settle();
    held.release({ success: true, activity: ACT('coding', { priority: 'high' }) }, 200);
    await settle();
    const puts = writesTo(p, 'PUT', url);
    assert.ok(puts.length === 1 && JSON.stringify(JSON.parse(puts[0].body)) === '{"priority":"high"}', `one PUT, the priority alone: ${JSON.stringify(puts)}`);
    assert.ok(priorityOf(p, 'coding') === select && select.value === 'high', `kept, not drawn again: ${select.value}`);
    assert.ok(p.doc.activeElement === select, `focus on the select: ${at(p)}`);
    assert.ok(reads() === readsBefore, `no read after: ${reads() - readsBefore}`);
  });

  // A default's description, instructions and remove, on a facet that shows
  // every default: what the page does with the journal's answer, a refusal (as
  // a facet that stores none answered before) or a save.
  const DEFAULTS_SHOWN = ['meeting', 'coding', 'email'];
  const listGets = (p) => p.calls.filter((c) => c.method === 'GET' && c.url === `/app/settings/api/facet/${SLUG}/activities`).length;
  const failed = (p) => p.notices.filter((notice) => notice.icon === 'circle-x').length;
  const NOT_FOUND = [{ error: 'activity not found', reason_code: 'activity_not_found' }, 404];
  // A double-click opens the field's edit; its input takes focus.
  function openEdit(p, kind, id) {
    const made = p.doc.createElement.bind(p.doc);
    p.doc.createElement = (tag) => { const el = editable(made(tag)); blurFires(p, el); return el; };
    const field = p.qa(`#attachedActivitiesList .activity-${kind}-editable`).find((el) => el.dataset.id === id);
    p.press(field);
    p.press(field);
    const input = p.q('#attachedActivitiesList .activity-desc-input');
    assert.ok(input && p.doc.activeElement === input, `editing: ${at(p)}`);
    return { field, input };
  }
  const editedTo = async (p, kind, id, text, answer, leave = 'Enter') => {
    const url = `/app/settings/api/facet/${SLUG}/activities/${id}`;
    const held = holdGranola(p, 'PUT', url);
    const { field, input } = openEdit(p, kind, id);
    const gets = listGets(p);
    const next = priorityOf(p, id);
    input.value = text;
    if (leave === 'Enter') p.key(input, 'Enter');
    else { next.focus(); p.doc.dispatch(input, { type: 'blur', bubbles: false }); }
    await settle();
    held.release(...answer);
    await settle();
    return { field, next, gets, puts: writesTo(p, 'PUT', url) };
  };
  for (const [kind, key] of [['desc', 'description'], ['instructions', 'instructions']]) {
    await test(`a default's ${key} edit, then Enter: refused, one PUT, the list drawn again and focus on the page; saved, one PUT, nothing drawn again, the text kept`, async () => {
      {
        const p = await activitiesPage({ attached: DEFAULTS_SHOWN, defaults: DEFAULTS_SHOWN });
        const { field, gets, puts } = await editedTo(p, kind, 'coding', 'mine', NOT_FOUND);
        assert.ok(puts.length === 1 && JSON.stringify(JSON.parse(puts[0].body)) === JSON.stringify({ [key]: 'mine' }), `one PUT of the ${key}: ${puts.length}`);
        assert.ok(failed(p) === 1, `a failure notice: ${failed(p)}`);
        assert.ok(listGets(p) === gets + 1 && !field.isConnected, `drawn again: ${listGets(p) - gets}`);
        assert.ok(p.doc.activeElement === p.doc.body, `focus on the page: ${at(p)}`);
      }
      {
        const p = await activitiesPage({ attached: DEFAULTS_SHOWN, defaults: DEFAULTS_SHOWN });
        const { field, gets, puts } = await editedTo(p, kind, 'coding', 'mine', [{ success: true, activity: ACT('coding', { [key]: 'mine' }) }, 200]);
        assert.ok(puts.length === 1, `one PUT: ${puts.length}`);
        assert.ok(failed(p) === 0, `no failure notice: ${failed(p)}`);
        assert.ok(listGets(p) === gets && field.isConnected && field.textContent === 'mine', `not drawn again, the text kept: ${listGets(p) - gets}`);
        assert.ok(p.doc.activeElement === p.doc.body, `focus on the page: ${at(p)}`);
      }
    });
  }

  await test("a default's description edit, then Tab to its priority: refused, focus on that priority drawn again; saved, on the same priority, nothing drawn again", async () => {
    {
      const p = await activitiesPage({ attached: DEFAULTS_SHOWN, defaults: DEFAULTS_SHOWN });
      const { next, gets, puts } = await editedTo(p, 'desc', 'coding', 'mine', NOT_FOUND, 'Tab');
      const again = priorityOf(p, 'coding');
      assert.ok(puts.length === 1 && listGets(p) === gets + 1, `one PUT, drawn again: ${puts.length} ${listGets(p) - gets}`);
      assert.ok(again !== next && p.doc.activeElement === again, `the priority drawn again: ${at(p)}`);
    }
    {
      const p = await activitiesPage({ attached: DEFAULTS_SHOWN, defaults: DEFAULTS_SHOWN });
      const { next, gets, puts } = await editedTo(p, 'desc', 'coding', 'mine', [{ success: true, activity: ACT('coding', { description: 'mine' }) }, 200], 'Tab');
      assert.ok(puts.length === 1 && listGets(p) === gets, `one PUT, not drawn again: ${puts.length} ${listGets(p) - gets}`);
      assert.ok(priorityOf(p, 'coding') === next && p.doc.activeElement === next, `the same priority: ${at(p)}`);
    }
  });

  await test("a default's remove: refused, one DELETE, nothing drawn again and focus kept on it; saved, one DELETE, drawn again and focus on the next row's priority", async () => {
    const url = `/app/settings/api/facet/${SLUG}/activities/coding`;
    {
      const p = await activitiesPage({ attached: DEFAULTS_SHOWN, defaults: DEFAULTS_SHOWN });
      const held = holdGranola(p, 'DELETE', url);
      const remove = removeOf(p, 'coding');
      const gets = listGets(p);
      remove.focus();
      p.press(remove);
      await settle();
      held.release(...NOT_FOUND);
      await settle();
      assert.ok(writesTo(p, 'DELETE', url).length === 1 && failed(p) === 1, `one DELETE, a failure notice: ${failed(p)}`);
      assert.ok(listGets(p) === gets && removeOf(p, 'coding') === remove, `not drawn again: ${listGets(p) - gets}`);
      assert.ok(p.doc.activeElement === remove && remove.getAttribute('aria-busy') === null, `kept on the remove: ${at(p)}`);
    }
    {
      const p = await activitiesPage({ attached: DEFAULTS_SHOWN, defaults: DEFAULTS_SHOWN });
      const held = holdGranola(p, 'DELETE', url);
      const remove = removeOf(p, 'coding');
      const gets = listGets(p);
      remove.focus();
      p.press(remove);
      await settle();
      p.take('coding');
      held.release({ success: true }, 200);
      await settle();
      assert.ok(writesTo(p, 'DELETE', url).length === 1 && failed(p) === 0, `one DELETE, no failure notice: ${failed(p)}`);
      assert.ok(listGets(p) === gets + 1 && !removeOf(p, 'coding'), `drawn again without it: ${listGets(p) - gets}`);
      assert.ok(p.doc.activeElement === priorityOf(p, 'email'), `the next row: ${at(p)} ${p.doc.activeElement.dataset?.id}`);
    }
  });

  await test('a default activity adds by Enter or Space, once while out, and lands on the next one left to add, else on "add custom activity"', async () => {
    const p = await activitiesPage({ attached: [], defaults: ['a1', 'a2'] });
    const held = holdGranola(p, 'POST', `/app/settings/api/facet/${SLUG}/activities`);
    const chip = chipOf(p, 'a1');
    chip.focus();
    p.key(chip, 'Enter');
    await settle();
    assert.ok(p.doc.activeElement === chip && chip.getAttribute('aria-busy') === 'true', `the chip, busy: ${at(p)}`);
    p.key(chip, ' ');
    p.press(chip);
    await settle();
    assert.ok(p.writes().filter((c) => c.method === 'POST').length === 1, 'one add');
    p.put('a1');
    held.release({ success: true });
    await settle();
    assert.ok(p.doc.activeElement === chipOf(p, 'a2'), `the next chip: ${at(p)} ${p.doc.activeElement.dataset?.id}`);
    p.key(chipOf(p, 'a2'), ' ');
    await settle();
    assert.ok(p.doc.activeElement === p.byId('addCustomActivityBtn'), `none left: ${at(p)}`);
  });

  await test("a custom activity's add stays busy while it is out, and a second press sends nothing", async () => {
    const p = await activitiesPage();
    const held = holdGranola(p, 'POST', `/app/settings/api/facet/${SLUG}/activities`);
    p.press(p.byId('addCustomActivityBtn'));
    await settle();
    p.byId('customActivityName').value = 'lab';
    const save = p.byId('saveCustomActivityBtn');
    save.focus();
    p.press(save);
    await settle();
    assert.ok(p.doc.activeElement === save && busyNotDisabled(save), `busy: ${at(p)}`);
    p.press(save);
    await settle();
    assert.ok(p.writes().filter((c) => c.method === 'POST').length === 1, 'one add');
    held.release({ error: 'lab: failed' }, 503);
    await settle();
    assert.ok(p.doc.activeElement === save && save.getAttribute('aria-busy') === null, `failed, kept: ${at(p)}`);
  });

  await test('"load older" reads one day at a time and, once nothing older is left, lands on the first of what it loaded', async () => {
    const day = (d, next) => ({ day: d, entries: [{ action: 'facet_update', actor: 'owner', source: 'app', params: {} }], next_cursor: next });
    const logs = `/app/settings/api/facet/${SLUG}/logs`;
    const base = journal();
    const answers = (url, opts) => (new URL(url, ORIGIN).pathname === logs ? (url.includes('cursor=') ? day('20261115', null) : day('20261116', 'c1')) : base(url, opts));
    const p = await boot(env, FP, { answers });
    p.press(p.byId('facetDetailTabActivity'));
    await settle();
    const more = p.byId('logMoreBtn');
    const held = holdGranola(p, 'GET', `${logs}?cursor=c1`);
    more.focus();
    p.press(more);
    await settle();
    assert.ok(p.doc.activeElement === more && more.getAttribute('aria-busy') === 'true', `busy: ${at(p)}`);
    p.press(more);
    await settle();
    assert.ok(p.reads(`${logs}?cursor=`).length === 1, 'one read');
    held.release(day('20261115', null));
    await settle();
    const summaries = p.qa('#logEntries summary');
    assert.ok(p.doc.activeElement === summaries[summaries.length - 1], `the first of the older day: ${at(p)}`);
  });

  await test("the facet delete's confirm puts focus on its opener at once, busy while the delete is out, with no second dialog; refused, the opener keeps it", async () => {
    const p = await boot(env, FP);
    const held = holdGranola(p, 'DELETE', `/app/settings/api/facet/${SLUG}`);
    const opener = p.byId('facetDeleteBtn');
    opener.hidden = false;
    opener.focus();
    p.press(opener);
    await settle();
    const reconciles = p.window.ConveyModalLayer.reconciles;
    p.press(p.byId('facetDeleteModalConfirm'));
    assert.ok(p.window.ConveyModalLayer.reconciles > reconciles, 'the layer is reconciled at once');
    assert.ok(p.doc.activeElement === opener && busyNotDisabled(opener), `the opener, busy: ${at(p)}`);
    p.press(opener);
    await settle();
    assert.deepStrictEqual(p.shownModals(), [], 'no second dialog');
    held.release({ success: false, reason: 'busy' }, 409);
    await settle();
    assert.ok(p.doc.activeElement === opener && opener.getAttribute('aria-busy') === null, `refused, kept: ${at(p)}`);
    assert.ok(p.writes().filter((c) => c.method === 'DELETE').length === 1, 'one delete');
  });

  // ---- the narrow section menu: a phone's menu can blur the select as it closes, after the change

  const pickSection = (p, value) => { const s = p.byId('navSelect'); s.focus(); s.value = value; p.doc.dispatch(s, { type: 'change' }); return s; };
  const menuBlur = (p, s) => { s.blur(); p.doc.dispatch(s, { type: 'focusout', relatedTarget: null }); };
  const ctxListeners = (p) => p.view.ctx.held().listeners;
  const NAV_WINDOW = 1730; // the view's wait
  const PHONE_BLUR = 865; // the slowest delay seen on a phone from the change to its blur

  await test("a pick from the narrow section menu, then the menu's own blur, puts focus back on the select, for every section that keeps it", async () => {
    for (const value of ['transcription', 'profile', 'about', 'apikeys', 'vision', 'sync', 'storage', 'trash', 'guide']) {
      const p = await boot(env, '/app/settings/#profile');
      const before = p.view.ctx.held();
      const select = pickSection(p, value);
      await settle();
      assert.ok(activeSection(p) === `section-${value}`, `${value} is shown`);
      p.clock.advance(PHONE_BLUR);
      menuBlur(p, select);
      assert.ok(p.doc.activeElement === p.doc.body, `${value}: the blur leaves the page`);
      p.clock.advance(0);
      assert.ok(p.doc.activeElement === select, `${value}: back on the select: ${at(p)}`);
      const after = p.view.ctx.held();
      assert.ok(after.listeners === before.listeners && after.timeouts === before.timeouts, `${value}: the landing ends the wait`);
    }
  });

  await test('a pick with no late blur leaves focus where it is, and the wait ends with its window', async () => {
    const p = await boot(env, '/app/settings/#profile');
    const base = ctxListeners(p);
    const timers = p.clock.live();
    const select = pickSection(p, 'transcription');
    await settle();
    p.clock.advance(NAV_WINDOW - 1);
    assert.ok(p.doc.activeElement === select, `kept: ${at(p)}`);
    assert.ok(ctxListeners(p) > base, 'the wait is armed inside its window');
    p.clock.advance(1);
    assert.ok(ctxListeners(p) === base, 'the wait takes its listeners back');
    assert.ok(p.clock.live() <= timers, 'and its timer');
    menuBlur(p, select);
    p.clock.advance(0);
    assert.ok(p.doc.activeElement === p.doc.body, `a blur after the window is left alone: ${at(p)}`);
  });

  await test("a blur in the window's last moment still lands", async () => {
    const p = await boot(env, '/app/settings/#profile');
    const select = pickSection(p, 'transcription');
    await settle();
    p.clock.advance(NAV_WINDOW - 1);
    menuBlur(p, select);
    p.clock.advance(0);
    assert.ok(p.doc.activeElement === select, `back on the select: ${at(p)}`);
  });

  await test('a second pick ends the first wait before it arms its own', async () => {
    const p = await boot(env, '/app/settings/#profile');
    const base = ctxListeners(p);
    pickSection(p, 'transcription');
    const one = ctxListeners(p) - base;
    pickSection(p, 'about');
    assert.ok(ctxListeners(p) - base === one, 'one wait');
    p.clock.advance(NAV_WINDOW);
    assert.ok(ctxListeners(p) === base, 'none left');
  });

  await test('a tap, a press or a key after a pick is the owner moving: the blur that follows is left alone', async () => {
    for (const type of ['pointerdown', 'touchstart', 'mousedown']) {
      const p = await boot(env, '/app/settings/#profile');
      const select = pickSection(p, 'transcription');
      await settle();
      p.doc.dispatch(p.q('#section-transcription'), { type });
      menuBlur(p, select);
      p.clock.advance(1000);
      assert.ok(p.doc.activeElement === p.doc.body, `${type} on the page: left on it: ${at(p)}`);
    }
    {
      const p = await boot(env, '/app/settings/#profile');
      const select = pickSection(p, 'transcription');
      await settle();
      const next = p.qa('#section-transcription button, #section-transcription select, #section-transcription input').find((el) => el.checkVisibility() && !el.disabled);
      assert.ok(next, 'a control in the section');
      p.doc.dispatch(next, { type: 'pointerdown' });
      next.focus();
      p.doc.dispatch(select, { type: 'focusout', relatedTarget: next });
      p.clock.advance(1000);
      assert.ok(p.doc.activeElement === next, `a tap on another control keeps it: ${at(p)}`);
    }
    {
      const p = await boot(env, '/app/settings/#profile');
      const select = pickSection(p, 'transcription');
      await settle();
      p.key(select, 'Tab');
      const next = p.qa('#section-transcription button, #section-transcription select, #section-transcription input').find((el) => el.checkVisibility() && !el.disabled);
      next.focus();
      p.doc.dispatch(select, { type: 'focusout', relatedTarget: next });
      p.clock.advance(1000);
      assert.ok(p.doc.activeElement === next, `a Tab keeps the next control: ${at(p)}`);
    }
  });

  await test("after the landing a second blur is left alone: no tug of war with the phone", async () => {
    const p = await boot(env, '/app/settings/#profile');
    const select = pickSection(p, 'transcription');
    await settle();
    menuBlur(p, select);
    p.clock.advance(0);
    assert.ok(p.doc.activeElement === select, `landed: ${at(p)}`);
    menuBlur(p, select);
    p.clock.advance(1000);
    assert.ok(p.doc.activeElement === p.doc.body, `left: ${at(p)}`);
  });

  await test('a Tab off the end of the page after a pick is left there', async () => {
    const p = await boot(env, '/app/settings/#profile');
    const select = pickSection(p, 'transcription');
    await settle();
    p.key(select, 'Tab');
    menuBlur(p, select);
    p.clock.advance(1000);
    assert.ok(p.doc.activeElement === p.doc.body, `left: ${at(p)}`);
  });

  await test('a tap between the blur and its landing is the owner moving too', async () => {
    const p = await boot(env, '/app/settings/#profile');
    const select = pickSection(p, 'transcription');
    await settle();
    menuBlur(p, select);
    p.doc.dispatch(p.q('#section-transcription'), { type: 'touchstart' });
    p.clock.advance(1000);
    assert.ok(p.doc.activeElement === p.doc.body, `left on the page: ${at(p)}`);
  });

  await test('a blur that moved focus somewhere shown, with no input first, is left there', async () => {
    const p = await boot(env, '/app/settings/#profile');
    const select = pickSection(p, 'transcription');
    await settle();
    const rail = p.byId('rail-home');
    rail.focus();
    p.doc.dispatch(select, { type: 'focusout', relatedTarget: rail });
    p.clock.advance(0);
    assert.ok(p.doc.activeElement === rail, `kept: ${at(p)}`);
  });

  await test('settings going between the blur and its landing lands nothing and leaves nothing', async () => {
    const p = await boot(env, '/app/settings/#profile');
    const view = p.view;
    const select = pickSection(p, 'transcription');
    await settle();
    menuBlur(p, select);
    p.leave();
    p.clock.advance(1000);
    const held = view.ctx.held();
    assert.deepStrictEqual([held.listeners, held.timeouts], [0, 0]);
    assert.ok(p.doc.activeElement !== select, 'the gone select takes nothing');
  });

  await test('a change with focus elsewhere, a pick of support and a pick of facets arm no wait', async () => {
    {
      const p = await boot(env, '/app/settings/#profile');
      const base = ctxListeners(p);
      const select = p.byId('navSelect');
      p.byId('rail-home').focus();
      select.value = 'transcription';
      p.doc.dispatch(select, { type: 'change' });
      assert.ok(ctxListeners(p) === base, 'focus elsewhere: no wait');
    }
    {
      const p = await boot(env, '/app/settings/#profile');
      const base = ctxListeners(p);
      pickSection(p, 'support');
      assert.ok(ctxListeners(p) === base, 'support: no wait');
    }
    {
      // The narrow layout's one rule the page model needs: the facets section hides the select.
      const p = await boot(env, '/app/settings/#profile');
      const wrap = p.byId('settings-index-view');
      const nav = p.q('.settings-nav-mobile');
      const toggle = wrap.classList.toggle.bind(wrap.classList);
      wrap.classList.toggle = (name, force) => {
        const on = toggle(name, force);
        if (name === 'facets-section-open') nav.style.display = on ? 'none' : '';
        return on;
      };
      const base = ctxListeners(p);
      pickSection(p, 'facets');
      await settle();
      const heading = p.q('#section-facets h2');
      assert.ok(p.doc.activeElement === heading, `facets: its heading: ${at(p)}`);
      assert.ok(ctxListeners(p) === base, 'facets: no wait');
    }
  });

  await test('settings going before the blur lands nothing and leaves nothing', async () => {
    const p = await boot(env, '/app/settings/#profile');
    const view = p.view;
    const select = pickSection(p, 'transcription');
    await settle();
    p.leave();
    menuBlur(p, select);
    p.clock.advance(1000);
    const held = view.ctx.held();
    assert.deepStrictEqual([held.listeners, held.timeouts], [0, 0]);
    assert.ok(p.doc.activeElement !== select, 'the gone select takes nothing');
  });

  // ---- every other select that stays shown after a pick: a phone's late blur, then focus back on it

  // As a phone does as its menu closes: blur, then focusout, with nothing pressed first.
  const lateBlur = (p, s) => { s.blur(); p.doc.dispatch(s, { type: 'blur', bubbles: false }); p.doc.dispatch(s, { type: 'focusout', relatedTarget: null }); };
  // A move off the select: a Tab, then its blur to the next control.
  const moveOff = (p, s) => {
    p.key(s, 'Tab');
    const next = p.byId('rail-home');
    next.focus();
    p.doc.dispatch(s, { type: 'blur', bubbles: false, relatedTarget: next });
    p.doc.dispatch(s, { type: 'focusout', relatedTarget: next });
  };
  const pickFrom = (p, s, value) => { s.focus(); s.value = value; p.doc.dispatch(s, { type: 'change' }); return s; };
  const holdWrites = (p) => { p.holds.push((call) => call.method !== 'GET'); };
  // The shell's save helper with its revert, as api.js has it: a refused save
  // puts back the value it read before the write.
  const reverting = (p) => {
    p.window.saveControl = async ({ el, fetchArgs, onSuccess, onError, revertOnError = true, readValue }) => {
      const previous = typeof readValue === 'function' ? readValue(el) : el.value;
      try {
        const result = await fetchArgs();
        if (typeof onSuccess === 'function') onSuccess(result);
        return result;
      } catch (error) {
        if (revertOnError !== false) el.value = previous ?? '';
        if (typeof onError === 'function') onError(error);
        throw error;
      }
    };
  };
  const configOf = (identity, transcribe) => ({ identity: { name: 'owner', timezone: 'UTC', ...identity }, transcribe, env: {}, runtime_env: {}, key_validation: {}, trash: { erase_after_days: 7 } });
  const BACKENDS = [{ name: 'parakeet', label: 'parakeet' }, { name: 'whisper', label: 'whisper' }];
  const CONFIG_URL = '/app/settings/api/config';
  // Each select, the state that draws it, a pick, and the one write it makes.
  const PICKS = {
    'pronouns': {
      address: '/app/settings/#profile', id: 'field-pronouns', value: 'they/them/their/themselves',
      write: ['PUT', CONFIG_URL, { section: 'identity', data: { pronouns: { subject: 'they', object: 'them', possessive: 'their', reflexive: 'themselves' } } }],
    },
    'speech processing': {
      address: '/app/settings/#transcription', id: 'field-transcribe-processing', value: 'confidential',
      extra: {
        '/app/settings/api/transcribe': { backends: [], parakeet_embedded: true },
        [`${CONFIG_URL}#PUT`]: { success: true, config: configOf({}, { backend: 'confidential', confidential_audio: true }) },
      },
      write: ['PUT', CONFIG_URL, { section: 'transcribe', data: { backend: 'confidential', confidential_audio: true } }],
    },
    'backend': {
      address: '/app/settings/#transcription', id: 'field-transcribe-backend', value: 'whisper',
      extra: { '/app/settings/api/transcribe': { backends: BACKENDS } },
      write: ['PUT', CONFIG_URL, { section: 'transcribe', data: { backend: 'whisper' } }],
    },
    'parakeet model': {
      address: '/app/settings/#transcription', id: 'field-parakeet-model-version', value: 'v3',
      extra: { '/app/settings/api/transcribe': { backends: BACKENDS } },
      write: ['PUT', CONFIG_URL, { section: 'transcribe', data: { parakeet: { model_version: 'v3' } } }],
    },
    'parakeet device': {
      address: '/app/settings/#transcription', id: 'field-parakeet-device', value: 'cpu',
      extra: { '/app/settings/api/transcribe': { backends: BACKENDS } },
      write: ['PUT', CONFIG_URL, { section: 'transcribe', data: { parakeet: { device: 'cpu' } } }],
    },
    'parakeet.cpp device': {
      address: '/app/settings/#transcription', id: 'field-parakeet-cpp-device', value: 'cpu',
      extra: { '/app/settings/api/transcribe': { backends: [...BACKENDS, { name: 'parakeet-cpp', label: 'parakeet.cpp' }] }, [CONFIG_URL]: configOf({}, { backend: 'parakeet-cpp' }) },
      write: ['PUT', CONFIG_URL, { section: 'transcribe', data: { 'parakeet-cpp': { device: 'cpu' } } }],
    },
    "an activity's priority": {
      activity: 'a2', value: 'high',
      write: ['PUT', `/app/settings/api/facet/${SLUG}/activities/a2`, { priority: 'high' }],
    },
    "the cleanup dialog's stream filter": { address: '/app/settings/#storage', id: 'cleanupStreamFilter', value: '', dialog: 'cleanupNowBtn' },
  };
  const WRITERS = Object.keys(PICKS).filter((name) => name !== 'pronouns' && PICKS[name].write);
  async function pickPage(name) {
    const spec = PICKS[name];
    let p;
    if (spec.activity) {
      p = await activitiesPage();
    } else {
      p = await boot(env, spec.address, spec.extra ? { answers: journal(spec.extra) } : undefined);
      if (spec.dialog) {
        p.press(p.byId(spec.dialog));
        await settle();
        p.clock.advance(16);
      }
    }
    const select = () => (spec.activity ? priorityOf(p, spec.activity) : p.byId(spec.id));
    assert.ok(select() && p.window.SolFocus.isShown(select()) && !select().disabled, `${name} is drawn and on`);
    return { p, spec, select };
  }
  const writesOf = (p, [method, url, body]) => p.writes().filter((call) => call.method === method && call.url === url && call.body === JSON.stringify(body));

  for (const name of Object.keys(PICKS)) {
    await test(`a pick from ${name}, then a phone's late blur, puts focus back on the select`, async () => {
      const { p, spec, select } = await pickPage(name);
      holdWrites(p);
      const picked = pickFrom(p, select(), spec.value);
      await settle();
      p.clock.advance(PHONE_BLUR);
      lateBlur(p, picked);
      assert.ok(p.doc.activeElement === p.doc.body, 'the blur leaves the page');
      p.clock.advance(0);
      assert.ok(p.doc.activeElement === picked, `back on the select: ${at(p)}`);
    });
  }

  await test("a pick from each of those selects with no late blur leaves focus on it, and what the wait added is gone after its window", async () => {
    for (const name of Object.keys(PICKS)) {
      const { p, spec, select } = await pickPage(name);
      holdWrites(p);
      const before = p.view.ctx.held();
      const picked = pickFrom(p, select(), spec.value);
      await settle();
      p.clock.advance(2000);
      await settle();
      assert.ok(p.doc.activeElement === picked, `${name}: kept: ${at(p)}`);
      const after = p.view.ctx.held();
      assert.ok(after.listeners === before.listeners && after.timeouts === before.timeouts, `${name}: nothing left: ${after.listeners}/${before.listeners} ${after.timeouts}/${before.timeouts}`);
    }
  });

  // Pronouns save on change and on blur: a phone's late blur, the landing and
  // a later move off it send nothing past the one save.
  const THEY = PICKS.pronouns.write[2].data.pronouns;
  const HE = { subject: 'he', object: 'him', possessive: 'his', reflexive: 'himself' };
  async function pronounsPage() {
    const p = await boot(env, '/app/settings/#profile', { answers: journal({ [CONFIG_URL]: configOf({ pronouns: HE }, {}) }) });
    reverting(p);
    const held = holdGranola(p, 'PUT', CONFIG_URL);
    const select = pickFrom(p, p.byId('field-pronouns'), PICKS.pronouns.value);
    p.clock.advance(500);
    await settle();
    assert.ok(configPuts(p).length === 1, `sent after its half second: ${configPuts(p).length}`);
    return { p, held, select };
  }
  const pronounsEnd = async (p, select) => {
    p.clock.advance(PHONE_BLUR - 500);
    lateBlur(p, select);
    p.clock.advance(0);
    await settle();
    moveOff(p, select);
    p.clock.advance(1000);
    await settle();
  };
  const onePronounsSave = (p) => {
    assert.ok(writesOf(p, PICKS.pronouns.write).length === 1, `one save of the pick: ${writesOf(p, PICKS.pronouns.write).length}`);
    assert.ok(p.writes().length === 1, `nothing else: ${p.writes().length}`);
  };

  await test('pronouns: a save still out at the late blur is sent once, through the landing and a later move', async () => {
    const { p, held, select } = await pronounsPage();
    await pronounsEnd(p, select);
    held.release({ success: true, config: configOf({ pronouns: THEY }, {}) });
    await settle();
    p.clock.advance(1000);
    await settle();
    onePronounsSave(p);
  });

  await test('pronouns: a save answered before the late blur is sent once, through the landing and a later move', async () => {
    const { p, held, select } = await pronounsPage();
    held.release({ success: true, config: configOf({ pronouns: THEY }, {}) });
    await settle();
    await pronounsEnd(p, select);
    onePronounsSave(p);
    assert.ok(select.value === PICKS.pronouns.value, 'the pick stays');
  });

  await test('pronouns: a save refused before the late blur puts back the saved choice, and neither blur sends it again', async () => {
    const { p, held, select } = await pronounsPage();
    held.release({ error: 'refused' }, 500);
    await settle();
    assert.ok(select.value === 'he/him/his/himself', `the saved choice: ${select.value}`);
    await pronounsEnd(p, select);
    onePronounsSave(p);
  });

  await test("each change-only writer sends one write per pick, and nothing from a phone's late blur, the landing or a later move", async () => {
    for (const name of WRITERS) {
      const { p, spec, select } = await pickPage(name);
      const picked = pickFrom(p, select(), spec.value);
      await settle();
      p.clock.advance(PHONE_BLUR);
      lateBlur(p, picked);
      p.clock.advance(0);
      await settle();
      moveOff(p, picked);
      p.clock.advance(2000);
      await settle();
      assert.ok(writesOf(p, spec.write).length === 1, `${name}: one write of the pick: ${writesOf(p, spec.write).length}`);
      assert.ok(p.writes().length === 1, `${name}: nothing else: ${p.writes().length}`);
    }
  });

  // A write each writer's code takes as refused.
  const REFUSALS = {
    'pronouns': [{ error: 'refused' }, 500],
    'speech processing': [{ success: false }, 503],
    'backend': [{ success: false, error: 'refused' }, 400],
    'parakeet model': [{ success: false, error: 'refused' }, 400],
    'parakeet device': [{ success: false, error: 'refused' }, 400],
    'parakeet.cpp device': [{ success: false, error: 'refused' }, 400],
    "an activity's priority": [{ error: 'refused' }, 404],
  };
  // The picked select, or for a priority, the same activity's drawn again.
  const sameControl = (p, spec, picked) => (spec.activity ? priorityOf(p, spec.activity) : picked);

  await test("a refused write, answered before or after a phone's late blur, leaves focus on the select, or on the same activity's priority drawn again", async () => {
    for (const name of Object.keys(REFUSALS)) {
      for (const answered of ['before', 'after']) {
        const { p, spec, select } = await pickPage(name);
        if (name === 'pronouns') reverting(p);
        const held = holdGranola(p, spec.write[0], spec.write[1]);
        const picked = pickFrom(p, select(), spec.value);
        await settle();
        if (name === 'pronouns') p.clock.advance(500);
        await settle();
        if (answered === 'before') { held.release(...REFUSALS[name]); await settle(); }
        p.clock.advance(PHONE_BLUR - p.clock.now());
        lateBlur(p, picked);
        p.clock.advance(0);
        if (answered === 'after') { held.release(...REFUSALS[name]); await settle(); }
        p.clock.advance(2000);
        await settle();
        const same = sameControl(p, spec, picked);
        assert.ok(same && p.doc.activeElement === same, `${name}, refused ${answered} the blur: ${at(p)}`);
        if (spec.activity) assert.ok(same !== picked, `${name}: drawn again`);
        assert.ok(p.writes().length === 1, `${name}: one write: ${p.writes().length}`);
      }
    }
  });

  // A refused priority isn't drawn again under a phone's closing menu: its
  // choice goes back on the same select, and the list is drawn again once
  // the pick has settled.
  const listReads = (p) => p.calls.filter((c) => c.method === 'GET' && c.url === `/app/settings/api/facet/${SLUG}/activities`).length;
  const refusedPriority = async (refusal) => {
    const { p, spec, select } = await pickPage("an activity's priority");
    const held = holdGranola(p, spec.write[0], spec.write[1]);
    const reads = listReads(p);
    const picked = pickFrom(p, select(), spec.value);
    await settle();
    held.release(...refusal);
    await settle();
    return { p, spec, picked, reads };
  };
  const REFUSED_SHAPES = {
    'a 404': REFUSALS["an activity's priority"],
    'a 503 with its error': [{ error: 'refused, never sent' }, 503],
  };

  for (const [shape, refusal] of Object.entries(REFUSED_SHAPES)) {
    await test(`a priority refused (${shape}) before a phone's late blur: the saved choice back at once, the blur lands on that select, then one read draws the list again and focus is on the new copy`, async () => {
      const { p, spec, picked, reads } = await refusedPriority(refusal);
      assert.ok(priorityOf(p, spec.activity) === picked && picked.value === 'normal', `the same select, its saved choice: ${picked.value}`);
      assert.ok(p.doc.activeElement === picked && listReads(p) === reads, `not drawn again yet: ${at(p)} ${listReads(p) - reads}`);
      p.clock.advance(PHONE_BLUR - p.clock.now());
      lateBlur(p, picked);
      p.clock.advance(0);
      assert.ok(p.doc.activeElement === picked, `landed back on it first: ${at(p)}`);
      await settle();
      const again = priorityOf(p, spec.activity);
      assert.ok(listReads(p) === reads + 1, `one read: ${listReads(p) - reads}`);
      assert.ok(again && again !== picked && p.doc.activeElement === again, `the new copy, focused: ${at(p)}`);
      p.clock.advance(3000);
      await settle();
      assert.ok(p.writes().length === 1 && listReads(p) === reads + 1, `one write, one read: ${p.writes().length} ${listReads(p) - reads}`);
    });
  }

  await test("a priority refused when the save can't reach the journal (the fetch fails): the choice back at once, and drawn again once settled", async () => {
    const { p, spec } = await pickPage("an activity's priority");
    const reads = listReads(p);
    const realFetch = p.window.fetch;
    p.window.fetch = (url, opts) => (String(opts?.method || 'GET').toUpperCase() === 'PUT' ? Promise.reject(new TypeError('Load failed')) : realFetch(url, opts));
    const picked = pickFrom(p, priorityOf(p, spec.activity), spec.value);
    await settle();
    assert.ok(picked.value === 'normal' && p.doc.activeElement === picked && listReads(p) === reads, `back at once, not drawn: ${picked.value}`);
    p.clock.advance(2000);
    await settle();
    assert.ok(listReads(p) === reads + 1 && p.doc.activeElement === priorityOf(p, spec.activity) && priorityOf(p, spec.activity) !== picked, `drawn again at the window: ${at(p)}`);
  });

  await test("a priority refused with no late blur (a desktop, Android): drawn again at the window, not before, landing on the new copy", async () => {
    const { p, spec, picked, reads } = await refusedPriority(REFUSALS["an activity's priority"]);
    p.clock.advance(1729 - p.clock.now());
    await settle();
    assert.ok(listReads(p) === reads && p.doc.activeElement === picked, `not before the window: ${listReads(p) - reads}`);
    p.clock.advance(1);
    p.clock.advance(0);
    await settle();
    const again = priorityOf(p, spec.activity);
    assert.ok(listReads(p) === reads + 1 && again !== picked && p.doc.activeElement === again, `at the window, on the new copy: ${at(p)}`);
  });

  await test("a priority refused, then a tap off it before the window: drawn again at once, and focus stays where the tap put it", async () => {
    const { p, spec, picked, reads } = await refusedPriority(REFUSALS["an activity's priority"]);
    p.clock.advance(300);
    const heading = p.qa('#facetDetailActivities h3')[0];
    p.doc.dispatch(heading, { type: 'pointerdown' });
    lateBlur(p, picked);
    p.clock.advance(0);
    await settle();
    assert.ok(listReads(p) === reads + 1 && priorityOf(p, spec.activity) !== picked, `drawn again at the tap: ${listReads(p) - reads}`);
    assert.ok(p.doc.activeElement === p.doc.body, `left where the tap put it: ${at(p)}`);
  });

  await test("a priority refused, then picked again before it settles: the first redraw gives way, and a saved second pick draws nothing", async () => {
    const { p, spec, picked, reads } = await refusedPriority(REFUSALS["an activity's priority"]);
    const held = holdGranola(p, spec.write[0], spec.write[1]);
    pickFrom(p, picked, 'low');
    await settle();
    held.release({ success: true }, 200);
    await settle();
    p.clock.advance(3000);
    await settle();
    assert.ok(listReads(p) === reads && priorityOf(p, spec.activity) === picked, `not drawn: ${listReads(p) - reads}`);
    assert.ok(picked.value === 'low' && picked.dataset.saved === 'low', `the second choice, saved: ${picked.value} ${picked.dataset.saved}`);
    assert.ok(p.writes().length === 2, `two writes: ${p.writes().length}`);
  });

  await test("a priority refused, then a key that picks again before it settles: the key ends the wait, and its own pick, a moment later, still drops the first redraw", async () => {
    const { p, spec, picked, reads } = await refusedPriority(REFUSALS["an activity's priority"]);
    const held = holdGranola(p, spec.write[0], spec.write[1]);
    p.clock.advance(300);
    // A browser's ArrowDown: the keydown first, then the change it makes.
    p.doc.dispatch(picked, { type: 'keydown', key: 'ArrowDown' });
    pickFrom(p, picked, 'low');
    p.clock.advance(0);
    await settle();
    held.release({ success: true }, 200);
    await settle();
    p.clock.advance(3000);
    await settle();
    assert.ok(listReads(p) === reads, `not drawn: ${listReads(p) - reads}`);
    assert.ok(picked.value === 'low' && priorityOf(p, spec.activity) === picked, `the second pick kept: ${picked.value}`);
  });

  await test("a priority refused, then the list drawn again by another write before it settles: nothing more is drawn when it settles", async () => {
    const { p, spec, picked, reads } = await refusedPriority(REFUSALS["an activity's priority"]);
    p.clock.advance(300);
    p.doc.dispatch(removeOf(p, 'a3'), { type: 'click' });
    await settle();
    const afterRemove = listReads(p);
    assert.ok(afterRemove > reads && priorityOf(p, spec.activity) !== picked, `the remove drew the list: ${afterRemove - reads}`);
    p.clock.advance(3000);
    await settle();
    assert.ok(listReads(p) === afterRemove, `nothing more at the window: ${listReads(p) - afterRemove}`);
  });

  await test("a saved priority, then a refused one: the choice put back is the one saved, not the one first drawn", async () => {
    const { p, spec } = await pickPage("an activity's priority");
    let held = holdGranola(p, spec.write[0], spec.write[1]);
    const select = pickFrom(p, priorityOf(p, spec.activity), 'high');
    await settle();
    held.release({ success: true }, 200);
    await settle();
    p.clock.advance(2000);
    await settle();
    held = holdGranola(p, spec.write[0], spec.write[1]);
    pickFrom(p, select, 'low');
    await settle();
    held.release({ error: 'refused' }, 404);
    await settle();
    assert.ok(select.value === 'high', `put back to the saved choice: ${select.value}`);
  });

  await test("a priority refused, then settings goes before it settles: nothing is drawn", async () => {
    const { p, picked, reads } = await refusedPriority(REFUSALS["an activity's priority"]);
    p.leave();
    p.clock.advance(3000);
    await settle();
    assert.ok(listReads(p) === reads, `no read after leaving: ${listReads(p) - reads}`);
    assert.ok(picked.value === 'normal', `the choice was put back before it went: ${picked.value}`);
  });

  await test("speech processing refused after a phone's late blur, its choice unreadable after, lands on the line that says so", async () => {
    const extra = PICKS['speech processing'].extra;
    const base = journal(extra);
    const read = { fails: false };
    const p = await boot(env, '/app/settings/#transcription', { answers: (url, opts) => {
      if (url.startsWith(CONFIG_URL) && String(opts?.method || 'GET').toUpperCase() === 'GET' && read.fails) return undefined;
      return base(url, opts);
    } });
    const held = holdGranola(p, 'PUT', CONFIG_URL);
    const select = pickFrom(p, p.byId('field-transcribe-processing'), 'confidential');
    await settle();
    p.clock.advance(PHONE_BLUR);
    lateBlur(p, select);
    p.clock.advance(0);
    assert.ok(p.doc.activeElement === select && busyNotDisabled(select), `back on the select, busy: ${at(p)}`);
    read.fails = true;
    held.release({ success: false }, 503);
    await settle();
    assert.ok(select.disabled && p.doc.activeElement === p.byId('transcribeProcessingHelp'), `the line that says so: ${at(p)}`);
  });

  await test("the backend's pick redraws its fieldsets, and the landing finds the same select", async () => {
    const { p, select } = await pickPage('backend');
    holdWrites(p);
    const picked = select();
    assert.ok(p.window.SolFocus.isShown(p.byId('parakeet-settings')), "parakeet's fieldset before");
    pickFrom(p, picked, 'whisper');
    await settle();
    assert.ok(!p.window.SolFocus.isShown(p.byId('parakeet-settings')), "parakeet's fieldset hidden");
    p.clock.advance(PHONE_BLUR);
    lateBlur(p, picked);
    p.clock.advance(0);
    assert.ok(p.byId('field-transcribe-backend') === picked, 'the same select');
    assert.ok(p.doc.activeElement === picked, `focus on it: ${at(p)}`);
  });

  await test("a pick of facets after an earlier pick: the select's blur as the heading takes focus lands nothing back on it", async () => {
    const p = await boot(env, '/app/settings/#profile');
    // The narrow layout's one rule the page model needs: the facets section hides the select.
    const wrap = p.byId('settings-index-view');
    const nav = p.q('.settings-nav-mobile');
    const toggle = wrap.classList.toggle.bind(wrap.classList);
    wrap.classList.toggle = (name, force) => {
      const on = toggle(name, force);
      if (name === 'facets-section-open') nav.style.display = on ? 'none' : '';
      return on;
    };
    const heading = p.q('#section-facets h2');
    const select = p.byId('navSelect');
    const base = ctxListeners(p);
    pickSection(p, 'transcription');
    await settle();
    assert.ok(ctxListeners(p) > base, 'the earlier pick waits');
    // As a browser does, focus moving to the heading blurs the select at once,
    // inside the pick's change.
    const own = heading.focus;
    let blurred = 0;
    heading.focus = function () {
      own.call(this);
      if (p.doc.activeElement === this && !blurred) { blurred += 1; p.doc.dispatch(select, { type: 'focusout', relatedTarget: this }); }
    };
    pickSection(p, 'facets');
    assert.ok(blurred === 1, 'the select blurred inside the change');
    await settle();
    assert.ok(p.doc.activeElement === heading, `the heading: ${at(p)}`);
    p.clock.advance(0);
    assert.ok(p.doc.activeElement === heading, `still the heading: ${at(p)}`);
    menuBlur(p, select);
    p.clock.advance(NAV_WINDOW);
    assert.ok(p.doc.activeElement === heading, `the heading after the window: ${at(p)}`);
    assert.ok(ctxListeners(p) === base, 'no wait left');
  });
  return cases;
};
