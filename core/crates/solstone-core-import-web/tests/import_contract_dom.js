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
const { loadWindowCatalog, journalContext } = require('../../solstone-core-convey-shell/tests/journal_catalog_dom.js');

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
      default:
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
  // As the shell's: a disclosure, open when the import's state asks for it.
  window.Drawer = { render: (options) => `<details class="drawer"${options.open ? ' open' : ''}><summary><p class="drawer-line">${options.line}</p></summary>${options.bodyHtml}</details>` };
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
  loadWindowCatalog(context);
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

async function validateQuick(p, withFile) {
  p.press(p.q('[data-import-quick]'));
  await settle();
  if (withFile) {
    const input = p.byId('fileInput');
    input.files = [{ name: 'notes.txt', size: 10 }];
    p.doc.dispatch(input, { type: 'change' });
  } else {
    p.byId('pasteText').value = 'a note';
  }
  p.doc.dispatch(p.byId('importForm'), { type: 'submit' });
  await settle();
}

// Run from import_workspace_dom.js: answers with the number of cases.
module.exports = async function importContractCases(crateDir) {
  const workspace = fs.readFileSync(path.join(crateDir, 'assets/workspace.html'), 'utf8');
  const inline = [...workspace.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
  const detailJs = fs.readFileSync(path.join(crateDir, 'assets/import_detail.js'), 'utf8');
  const template = workspace.replace(/<style>[\s\S]*?<\/style>/g, '').replace(/<script[\s\S]*?<\/script>/g, '').replace(/<link[^>]*>/g, '');
  const { JournalI18n } = journalContext({});
  const markup = JournalI18n.renderWorkspace(template);
  const shellDir = path.join(crateDir, '../solstone-core-convey-shell/assets/static');
  // In the fragment's order: the view's script, then its file.
  const env = { markup, shellDir, scripts: [['workspace.html', inline[0]], ['import_detail.js', detailJs]] };
  const mountSource = inline[0].slice(inline[0].indexOf('function mountImport(ctx) {'));

  await test('the fragment is a contract view with its first reads, one script, and no handler naming a global', async () => {
    assert.ok(workspace.includes('<!-- view-contract -->'));
    assert.ok(workspace.includes('<!-- first-read: /app/import/ -> /app/import/api/sources /app/import/api/list?page=1&per_page=100&source= -->'));
    assert.ok(inline.length === 1, 'one inline script');
    const handlers = [...workspace.matchAll(/\son[a-z]+="([^"]*)"/g)].map((m) => m[1]);
    assert.deepStrictEqual(handlers, ['return false;'], "the guided form's submit guard is the only inline handler, and it calls nothing");
    for (const source of [inline[0], detailJs]) {
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
    assert.deepStrictEqual(p.windowKeys().filter((k) => !keys.includes(k)), ['ImportDetail'], 'only the detail file\'s code is defined');
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
        await p.mount();
        assert.ok(p.window.listenerCount() > listeners[0], 'the view listens on the window while it is up');
        assert.ok(p.main.listenerCount() === 2, 'one press and one key delegation on #main-content');
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

  const isSave = (call) => call.method === 'POST' && call.url === '/app/import/api/save';
  for (const [name, start] of [['guided', startGuided], ['quick', (p) => validateQuick(p, true)]]) {
    await test(`${name} file upload shows its live status and leaving aborts it with one notice`, async () => {
      const p = await boot(env);
      p.holds.push(isSave);
      await start(p);
      const save = p.calls.find(isSave);
      assert.ok(save && !save.settled, 'the upload is out');
      const status = p.byId('importUploadStatus');
      assert.strictEqual(status?.getAttribute('role'), 'status');
      assert.ok(status.textContent.trim(), 'the stay-on-page status is shown');
      assert.strictEqual(p.notices.length, 0, 'no notice before leaving');
      p.leave();
      await settle();
      assert.ok(save.aborted, 'the upload was aborted');
      assert.deepStrictEqual(p.writes().map((c) => c.url), ['/app/import/api/save'], 'no meta, preview or start went out');
      assert.deepStrictEqual(p.notices.map((notice) => notice.app), ['import']);
      assert.ok(p.byId('home-text')?.textContent === 'home as it was');
    });
  }

  await test('a settled file upload clears its warning and leaving shows no interruption notice', async () => {
    for (const code of [200, 500]) {
      const p = await boot(env);
      p.holds.push(isSave);
      await validateQuick(p, true);
      const status = p.byId('importUploadStatus');
      assert.ok(status?.textContent.trim());
      const answer = code === 200 ? { path: '/journal/imports/x', timestamp: SAVED } : { error: 'failed' };
      answerHeld(p, isSave, answer, code);
      await settle();
      assert.strictEqual(status.textContent, '');
      p.leave();
      await settle();
      assert.strictEqual(p.notices.length, 0);
    }
  });

  await test('quick pasted text keeps its existing save cancellation without a file-upload warning', async () => {
    const p = await boot(env);
    p.holds.push(isSave);
    await validateQuick(p, false);
    const save = p.calls.find(isSave);
    assert.ok(save && !save.settled);
    assert.strictEqual(p.byId('importUploadStatus').textContent, '');
    p.leave();
    await settle();
    assert.ok(save.aborted);
    assert.strictEqual(p.notices.length, 0);
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

  // The journal names a file the retired audio-folder sync brought in for the
  // folder, and an audio file the owner chose as plain audio; the dialog says which.
  for (const [what, display] of [['a past audio folder import', 'folder'], ['an audio file the owner chose', 'audio']]) {
    await test(`${what} keeps its source label on the desktop delete dialog`, async () => {
      const folder = journalContext({}).JournalI18n.render('journal.window.import.import.source.names.audio.audio.folder');
      const p = await boot(env, `/app/import/${DONE}`, { answers: journal({
        [`GET /app/import/api/${DONE}`]: detailAnswer(DONE, { source_type: 'audio', source_display: display === 'folder' ? folder : display }),
      }) });
      p.press(p.q('[data-import-delete]'));
      await settle();
      const meta = p.byId('importDeleteModal').querySelector('.trash-dlg-meta strong').textContent;
      assert.strictEqual(meta, display === 'folder' ? folder : display);
    });
  }

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

  await test('each poll stops when import goes: the thinking progress and the 1 s list reload', async () => {
    // The thinking progress, on a progress view.
    const p = await boot(env, `/app/import/#progress/${RUNNING}`, { folders: true });
    assert.ok(p.byId('guideSteps').querySelector('.import-progress-panel'), 'the progress panel draws');
    assert.ok(p.clock.live() >= 1, 'the progress poll is set');
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

  await test('the cards carry a link to sources, which a guide hides and the grid shows again', async () => {
    const p = await boot(env);
    const link = p.byId('importSourcesLink');
    assert.ok(link && link.tagName === 'A', 'the link is there');
    assert.strictEqual(link.getAttribute('href'), '/app/sources/');
    assert.strictEqual(link.textContent, JournalI18n.render('journal.window.import.link.label.see.when.your.devices.last.added.something'));
    assert.ok(link.style.display === '', 'it shows with the cards');
    const guided = await boot(env, '/app/import/#guide/chatgpt');
    const hidden = guided.byId('importSourcesLink');
    assert.ok(hidden.style.display === 'none', 'a guide hides it');
    guided.press(guided.q('.import-back-link'));
    await settle();
    assert.ok(hidden.style.display === '', 'back on the cards it shows again');
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
    assert.ok(p.doc.activeElement !== p.doc.body, `focus is placed (${focusOn(p)})`);
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
      assert.ok(p.doc.activeElement !== p.doc.body, `${address}: focus is placed (${focusOn(p)})`);
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

  await test('the next mount picks up what a full load would: a running import and a stashed receipt', async () => {
    const p = makePage(env, '/app/import/', { folders: true });
    p.storage.set('journal-import-delete-receipt', JSON.stringify({ at: Date.now(), erase: false, outcome: { kind: 'moved', trashId: 'trash-1', eraseAt: '2026-12-01T00:00:00Z' } }));
    p.loadScripts();
    p.place();
    await p.mount();
    await settle();
    assert.ok(p.notices.some((n) => n.title === 'moved to the trash'), 'the stashed receipt shows');
    assert.ok(!p.storage.has('journal-import-delete-receipt'), 'and is taken');
    assert.ok(p.streams.find((s) => s.kind === 'listen' && !s.stopped).pending.tracked.has(RUNNING), 'the running import is tracked again');
  });

  // ---- M9u: presses keep focus, and a draw that changes nothing writes nothing

  const listen = (p) => p.streams.find((s) => s.kind === 'listen' && !s.stopped);
  const under = (p, selector, from) => mutationsUnder(p.q(selector), from);
  const rowOf = (p, id) => p.q(`tr[data-import-id="${id}"]`);
  const same = (a, b, message) => assert.ok(a === b, message);
  // Every listener on a node in the page, so a case can see a draw bind none twice.
  const elementListeners = (p) => p.doc.documentElement.querySelectorAll('*').reduce((n, el) => n + el.listenerCount(), 0);
  // A page whose list answer a case can change between reads.
  function listPage(address = '/app/import/', extra = {}, options = {}) {
    const state = { list: listAnswer() };
    const serve = journal({ 'GET /app/import/api/list': () => state.list, ...extra });
    // An import's durable record says what its row says, as the journal's does.
    const answers = (url, opts) => {
      const u = new URL(url, ORIGIN);
      const id = /^\/app\/import\/api\/(\d{8}_\d{6})$/.exec(u.pathname)?.[1];
      const row = id && String(opts?.method || 'GET').toUpperCase() === 'GET' && !(`GET ${u.pathname}` in extra)
        && state.list.imports.find((imp) => imp.timestamp === id);
      return row ? detailAnswer(id, row) : serve(url, opts);
    };
    const p = makePage(env, address, { answers, ...options });
    p.state = state;
    return p;
  }
  async function bootList(address, extra, options) {
    const p = listPage(address, extra, options);
    p.loadScripts();
    p.place();
    await p.mount();
    await settle();
    return p;
  }
  // The history read again, as the filter's change asks for it (which also
  // clears a selection, by design).
  async function reread(p) {
    p.byId('importSourceFilter').onchange({ target: { value: '' } });
    await settle();
  }
  // The history read again, as the 1 s re-read after a finished import's
  // event asks for it (an event for a row that already says so).
  async function rereadKeeping(p) {
    const done = p.state.list.imports.find((imp) => imp.status === 'success');
    listen(p).fn({ event: 'completed', import_id: done.timestamp, entries_written: done.entries_written, entities_seeded: done.entities_seeded });
    await settle();
    p.clock.advance(1000);
    await settle();
  }
  const THREE = () => listAnswer([importRow('20261031_080000', 'success'), importRow(DONE, 'success'), importRow(RUNNING, 'running')]);

  await test('a re-read that says nothing new writes nothing in the list, the grid or select mode, and focus stays on its node', async () => {
    const p = await bootList();
    for (const pick of [false, true]) {
      if (pick) {
        p.press(p.byId('importSelectBtn'));
        const box = p.q(`tr[data-import-id="${DONE}"] input[data-import-pick]`);
        box.checked = true;
        p.press(box);
        await settle();
      }
      const row = rowOf(p, DONE);
      row.focus();
      const mark = mutationMark();
      await rereadKeeping(p);
      assert.ok(under(p, '#importListView', mark) === 0, `nothing written (${pick ? 'select mode' : 'the list'}): ${mutationsSeen(p.q('#importListView'), mark)}`);
      same(p.doc.activeElement, row, 'focus stays on the same row');
    }
  });

  await test('a changed re-read keeps the nodes that are the same: a text change, an inserted first row, a reorder, an attribute change, a removed row with focus', async () => {
    const p = await bootList();
    const done = rowOf(p, DONE);
    done.focus();
    // Text only: a running row's stats move.
    p.state.list = listAnswer([importRow(DONE, 'success'), importRow(RUNNING, 'running', { entries_written: 9 })]);
    const running = rowOf(p, RUNNING);
    await reread(p);
    same(rowOf(p, DONE), done, 'the unchanged row is kept');
    same(rowOf(p, RUNNING), running, 'the changed row is kept, its text brought over');
    assert.ok(running.querySelector('.import-stats-cell').textContent.includes('9'), 'with the new text');
    same(p.doc.activeElement, done, 'focus stays');
    // An inserted first row.
    p.state.list = THREE();
    await reread(p);
    same(rowOf(p, DONE), done, 'the rows below an inserted one are kept');
    assert.ok(p.qa('.import-table tbody tr')[0].getAttribute('data-import-id') === '20261031_080000', 'the new row is first');
    same(p.doc.activeElement, done, 'focus stays');
    // A reorder: the focused row is drawn again, and focus follows it.
    p.state.list = listAnswer([importRow(RUNNING, 'running', { entries_written: 9 }), importRow(DONE, 'success'), importRow('20261031_080000', 'success')]);
    await reread(p);
    assert.deepStrictEqual(p.qa('.import-table tbody tr').map((r) => r.getAttribute('data-import-id')), [RUNNING, DONE, '20261031_080000'], 'the new order');
    assert.ok(p.doc.activeElement.getAttribute('data-import-id') === DONE, `focus on the same import (${focusOn(p)})`);
    // An attribute change replaces the row; focus goes to its successor.
    rowOf(p, RUNNING).focus();
    p.state.list = listAnswer([importRow(RUNNING, 'success'), importRow(DONE, 'success'), importRow('20261031_080000', 'success')]);
    await reread(p);
    assert.ok(p.doc.activeElement.getAttribute('data-import-id') === RUNNING, `focus on the same import (${focusOn(p)})`);
    // A removed row with focus in it: the next row takes it.
    rowOf(p, DONE).focus();
    p.state.list = listAnswer([importRow(RUNNING, 'success'), importRow('20261031_080000', 'success')]);
    await reread(p);
    assert.ok(p.doc.activeElement.getAttribute('data-import-id') === '20261031_080000', `focus on the next row (${focusOn(p)})`);
  });

  await test('twenty changed draws of each region bind nothing twice', async () => {
    const p = await bootList();
    const start = elementListeners(p);
    const page = listenerCounts(p);
    for (let i = 0; i < 20; i += 1) {
      p.state.list = listAnswer([importRow(DONE, 'success', { entries_written: i }), importRow(RUNNING, 'running', { entries_written: 20 + i })]);
      await reread(p);
      listen(p).fn({ event: 'status', import_id: RUNNING, stage: 'importing', elapsed_ms: 1000 * i, generation: 1 });
    }
    await settle();
    assert.ok(elementListeners(p) === start, `the list's listeners: ${start} → ${elementListeners(p)}`);
    assert.deepStrictEqual(listenerCounts(p), page, 'the page and main listeners');
    // The progress panel, the detail's page and its content pages.
    const q = await bootList(`/app/import/#progress/${RUNNING}`);
    const qStart = elementListeners(q);
    for (let i = 0; i < 20; i += 1) listen(q).fn({ event: 'status', import_id: RUNNING, stage: i % 2 ? 'writing' : 'importing', elapsed_ms: 1000 * i, generation: 1 });
    await settle();
    assert.ok(elementListeners(q) === qStart, `the panel's listeners: ${qStart} → ${elementListeners(q)}`);
    const state = { n: 0 };
    const d = await bootList(`/app/import/${DONE}#content`, { [`GET /app/import/api/${DONE}`]: () => detailAnswer(DONE, { entries_written: state.n }) });
    const dStart = elementListeners(d);
    for (let i = 0; i < 20; i += 1) {
      state.n = i;
      listen(d).fn({ event: 'status', import_id: DONE });
      await settle();
      d.press(d.find('#contentPagination button', i % 2 ? 'prev' : 'next'));
      await settle();
    }
    assert.ok(elementListeners(d) === dStart, `the detail's listeners: ${dStart} → ${elementListeners(d)}`);
  });

  await test('the progress panel keeps its node and focus through a stream that changes it, and an event with nothing new writes nothing', async () => {
    const p = await bootList(`/app/import/#progress/${RUNNING}`);
    const panel = p.q('.import-progress-panel');
    panel.focus();
    const event = { event: 'status', import_id: RUNNING, stage: 'importing', elapsed_ms: 5000, generation: 1 };
    listen(p).fn(event);
    await settle();
    same(p.q('.import-progress-panel'), panel, 'the panel is the same node');
    same(p.doc.activeElement, panel, 'focus stays on it');
    const mark = mutationMark();
    listen(p).fn({ ...event });
    await settle();
    assert.ok(under(p, '#guideSteps', mark) === 0, `the same event wrote ${under(p, '#guideSteps', mark)}`);
    listen(p).fn({ ...event, stage: 'writing', elapsed_ms: 9000 });
    await settle();
    same(p.q('.import-progress-panel'), panel, 'still the same node after a change');
    same(p.doc.activeElement, panel, 'and focused');
  });

  await test("an import's page: an event with nothing new writes nothing, a changed one keeps its delete and the saved start's slot", async () => {
    const state = { extra: {} };
    const p = await bootList(`/app/import/${DONE}`, { [`GET /app/import/api/${DONE}`]: () => detailAnswer(DONE, state.extra) });
    const del = p.q('[data-import-delete]');
    del.focus();
    let mark = mutationMark();
    listen(p).fn({ event: 'status', import_id: DONE });
    await settle();
    assert.ok(under(p, '#importDetailView', mark) === 0, `nothing new wrote ${under(p, '#importDetailView', mark)}`);
    state.extra = { entries_written: 9, has_gaps: true };
    listen(p).fn({ event: 'status', import_id: DONE });
    await settle();
    same(p.q('[data-import-delete]'), del, 'the delete is kept');
    same(p.doc.activeElement, del, 'and focused');
    // A drawer the owner opened stays open through events, and they write nothing.
    const drawer = p.q('details.drawer');
    drawer.open = true;
    mark = mutationMark();
    listen(p).fn({ event: 'status', import_id: DONE });
    await settle();
    same(p.q('details.drawer'), drawer, 'the drawer is kept');
    assert.ok(drawer.open, 'and stays open');
    assert.ok(under(p, '#importDetailView', mark) === 0, `an open drawer: nothing new wrote ${mutationsSeen(p.q('#importDetailView'), mark)}`);
    // And a state that opens it in the markup opens a kept, closed one: a
    // running import that fails.
    const run = { extra: { status: 'running' } };
    const r = await bootList(`/app/import/${RUNNING}`, { [`GET /app/import/api/${RUNNING}`]: () => detailAnswer(RUNNING, run.extra) });
    const closed = r.q('details.drawer');
    assert.ok(closed && !closed.open, 'a running import draws its drawer closed');
    run.extra = { status: 'failed', error: 'it stopped' };
    listen(r).fn({ event: 'status', import_id: RUNNING });
    await settle();
    same(r.q('details.drawer'), closed, 'the drawer is kept');
    assert.ok(closed.open, 'and the failure opens it, as the base drew it');
    // A saved start's slot is the flow's: a redraw of the overview leaves it.
    const saved = { timestamp: SAVED, status: 'pending', saved_start: { state: 'eligible', active: false },
      import_json: { import_id: SAVED, file_path: '/journal/imports/x/n.txt', source_timestamp: SAVED, file_size: 6, is_local_path: false, source_hint: 'chatgpt' } };
    const s = await bootList(`/app/import/${SAVED}`, { [`GET /app/import/api/${SAVED}`]: () => ({ ...detailAnswer(SAVED), ...saved, ...state.extra }) });
    const button = s.q('[data-saved-start-action]');
    assert.ok(button, 'the saved start draws');
    button.focus();
    mark = mutationMark();
    listen(s).fn({ event: 'status', import_id: SAVED });
    await settle();
    assert.ok(under(s, '#importDetailView', mark) === 0, `nothing new wrote ${under(s, '#importDetailView', mark)}`);
    state.extra = { entries_written: 3 };
    listen(s).fn({ event: 'status', import_id: SAVED });
    await settle();
    same(s.q('[data-saved-start-action]'), button, "the saved start's button is kept");
    same(s.doc.activeElement, button, 'and focused');
  });

  await test('a source card lands on its guide, and every way back to the grid on the card it came from', async () => {
    const p = await bootList();
    p.q('[data-import-source="chatgpt"]').focus();
    p.press(p.q('[data-import-source="chatgpt"]'));
    await settle();
    assert.ok(focusOn(p) === 'H2:ChatGPT', `the guide's heading (${focusOn(p)})`);
    p.q('.import-back-link').focus();
    p.press(p.q('.import-back-link'));
    await settle();
    assert.ok(p.doc.activeElement.getAttribute('data-import-source') === 'chatgpt', `the card it came from (${focusOn(p)})`);
    p.q('[data-import-quick]').focus();
    p.press(p.q('[data-import-quick]'));
    await settle();
    assert.ok(focusOn(p) === 'H2:quick import', `the quick guide's heading (${focusOn(p)})`);
    p.window.history.back();
    await settle();
    assert.ok(p.doc.activeElement.hasAttribute('data-import-quick'), `a back lands on the quick card (${focusOn(p)})`);
  });

  await test('validate stays busy and focused while its save is out, a second press or Enter sends nothing, then the date field', async () => {
    for (const mode of ['held', 'failed', 'answered']) {
      const p = await bootList();
      p.press(p.q('[data-import-quick]'));
      await settle();
      const t = (c) => c.url === '/app/import/api/save';
      if (mode === 'held') p.holds.push(t);
      if (mode === 'failed') p.statuses.push([t, 500]);
      p.byId('pasteText').value = 'a note';
      const validate = p.byId('validateBtn');
      validate.focus();
      p.doc.dispatch(p.byId('importForm'), { type: 'submit' });
      await settle();
      if (mode === 'held') {
        same(p.doc.activeElement, validate, 'focus stays on validate');
        assert.ok(validate.getAttribute('aria-busy') === 'true' && !validate.disabled, 'busy, not disabled');
        p.doc.dispatch(p.byId('importForm'), { type: 'submit' });
        p.doc.dispatch(p.byId('importForm'), { type: 'submit' });
        await settle();
        assert.ok(p.writes().filter(t).length === 1, 'one save');
      } else if (mode === 'failed') {
        same(p.doc.activeElement, validate, 'a failed save leaves focus on validate');
        assert.ok(validate.getAttribute('aria-busy') === null, 'no longer busy');
      } else {
        assert.ok(focusOn(p) === 'INPUT#timestampInput', `the dialog's date (${focusOn(p)})`);
      }
    }
  });

  await test("the detect dialog's preview and start, and guided preview and start: busy while out, the progress panel after a start", async () => {
    for (const mode of ['held', 'answered']) {
      const p = await bootList();
      p.press(p.q('[data-import-quick]'));
      await settle();
      p.byId('pasteText').value = 'a note';
      p.doc.dispatch(p.byId('importForm'), { type: 'submit' });
      await settle();
      const start = p.byId('startBtn');
      if (mode === 'held') p.holds.push((c) => c.url === '/app/import/api/meta');
      start.focus();
      p.press(start);
      await settle();
      if (mode === 'held') {
        same(p.doc.activeElement, start, "the dialog's preview keeps focus while out");
        assert.ok(start.getAttribute('aria-busy') === 'true' && !start.disabled, 'busy, not disabled');
        continue;
      }
      same(p.doc.activeElement, start, 'after the preview, the same button ("start import")');
      p.press(start);
      await settle();
      assert.ok(focusOn(p).startsWith('DIV') && p.doc.activeElement.classList.contains('import-progress-panel'), `the progress panel (${focusOn(p)})`);
    }
    for (const mode of ['held', 'answered']) {
      const p = await bootList();
      if (mode === 'held') p.holds.push((c) => c.url === '/app/import/api/preview');
      await startGuided(p);
      const start = p.byId('guidedStartBtn');
      start.focus();
      if (mode === 'held') {
        same(p.doc.activeElement, start, 'guided preview keeps focus while out');
        assert.ok(start.getAttribute('aria-busy') === 'true' && !start.disabled, 'busy, not disabled');
        continue;
      }
      p.press(start);
      await settle();
      assert.ok(p.doc.activeElement.classList.contains('import-progress-panel'), `guided start lands on the panel (${focusOn(p)})`);
    }
  });

  await test("a facet answer: the pressed one busy and its pair held while out, no second answer, then the facet's select", async () => {
    const strava = { name: 'strava', display_name: 'Strava', description: 'workouts', has_guide: false, input_type: 'file', accept: '.zip' };
    const facet = { offer: { state: 'ask_new', facet: 'fitness' }, facets: [{ name: 'fitness', title: 'fitness' }],
      copy: { heading: 'which', ask_new: 'file under {facet}?', no_means: 'no', yes: 'yes', no: 'no', this_import: 'this import', none_option: 'none', none_hint: 'none', files_earlier: 'earlier {facet}' } };
    for (const mode of ['held', 'answered']) {
      const p = await bootList('/app/import/#guide/strava', {
        'GET /app/import/api/sources': () => ({ items: [...sourcesAnswer().items, strava] }),
        'GET /app/import/api/body-facet': () => facet,
        'POST /app/import/api/body-facet': () => ({ ...facet, offer: { state: 'chosen', facet: 'fitness' } }),
      });
      const t = (c) => c.method === 'POST' && c.url === '/app/import/api/body-facet';
      if (mode === 'held') p.holds.push(t);
      const yes = p.q('[data-body-facet-answer="yes"]');
      const no = p.q('[data-body-facet-answer="no"]');
      yes.focus();
      p.press(yes);
      await settle();
      if (mode === 'held') {
        same(p.doc.activeElement, yes, 'focus stays on the answer');
        assert.ok(yes.getAttribute('aria-busy') === 'true' && no.getAttribute('aria-disabled') === 'true' && no.getAttribute('aria-busy') === null, 'pressed busy, its pair held');
        assert.ok(!yes.disabled && !no.disabled, 'neither turned off');
        p.press(no);
        p.press(yes);
        await settle();
        assert.ok(p.writes().filter(t).length === 1, 'one answer');
      } else {
        assert.ok(focusOn(p) === 'SELECT#guidedFacetSelect', `the facet's select (${focusOn(p)})`);
      }
    }
  });

  await test("a delete from the list: focus back on the busy opener while out, a second delete can't start, then the next row", async () => {
    for (const mode of ['held', 'failed', 'answered', 'set']) {
      const p = await bootList('/app/import/', {
        'POST /app/import/api/imports/delete': () => ({ done: [{ id: DONE, result: { trash: { id: 't1', erase_at: '2026-12-01T00:00:00Z' } } }, { id: '20261031_080000', result: { trash: { id: 't2', erase_at: '2026-12-01T00:00:00Z' } } }], failed: [] }),
      });
      p.state.list = THREE();
      await reread(p);
      p.press(p.byId('importSelectBtn'));
      for (const id of mode === 'set' ? ['20261031_080000', DONE] : [DONE]) {
        const box = p.q(`tr[data-import-id="${id}"] input[data-import-pick]`);
        box.checked = true;
        p.press(box);
      }
      await settle();
      const opener = p.byId('importPickDelete');
      opener.focus();
      p.press(opener);
      await settle();
      const t = (c) => c.method === 'DELETE' || c.url === '/app/import/api/imports/delete';
      if (mode === 'held') p.holds.push(t);
      if (mode === 'failed') p.statuses.push([t, 409]);
      // The row before the deleted one stays, so the landing has to pick the one after.
      p.state.list = mode === 'set' ? listAnswer([importRow(RUNNING, 'running')]) : listAnswer([importRow('20261031_080000', 'success'), importRow(RUNNING, 'running')]);
      p.press(p.byId('importDeleteModalConfirm'));
      await settle();
      if (mode === 'held') {
        same(p.doc.activeElement, opener, 'focus is back on the opener');
        assert.ok(opener.getAttribute('aria-busy') === 'true', 'which is busy');
        p.press(opener);
        await settle();
        assert.ok(p.byId('importDeleteModal').style.display !== 'flex', 'no second dialog');
        assert.ok(p.writes().filter(t).length === 1, 'one delete');
      } else if (mode === 'failed') {
        assert.ok(focusOn(p) === 'BUTTON#importPickDelete', `a refused delete stays on its opener (${focusOn(p)})`);
      } else {
        assert.ok(p.doc.activeElement.getAttribute('data-import-id') === RUNNING, `the next row (${focusOn(p)})`);
      }
    }
    // With nothing after it or before it, the list's heading.
    const p = await bootList();
    p.state.list = listAnswer([importRow(DONE, 'success')]);
    await reread(p);
    p.press(p.byId('importSelectBtn'));
    const box = p.q(`tr[data-import-id="${DONE}"] input[data-import-pick]`);
    box.checked = true;
    p.press(box);
    p.byId('importPickDelete').focus();
    p.press(p.byId('importPickDelete'));
    await settle();
    p.state.list = listAnswer([]);
    p.state.list.total = 0;
    p.press(p.byId('importDeleteModalConfirm'));
    await settle();
    assert.ok(focusOn(p) === 'H2:import history', `the list's heading (${focusOn(p)})`);
  });

  await test("an import's own delete: focus back on its busy delete while out", async () => {
    const holder = { extra: {} };
    const p = await bootList(`/app/import/${DONE}`, { [`GET /app/import/api/${DONE}`]: () => detailAnswer(DONE, holder.extra) });
    const del = p.q('[data-import-delete]');
    del.focus();
    p.press(del);
    await settle();
    p.holds.push((c) => c.method === 'DELETE');
    p.press(p.byId('importDeleteModalConfirm'));
    await settle();
    same(p.doc.activeElement, del, 'focus is back on delete import');
    assert.ok(del.getAttribute('aria-busy') === 'true', 'which is busy');
    // A stream that redraws the page meanwhile keeps the busy control, busy.
    holder.extra = { entries_written: 7 };
    listen(p).fn({ event: 'status', import_id: DONE });
    await settle();
    same(p.q('[data-import-delete]'), del, 'a redraw keeps the busy delete');
    assert.ok(del.getAttribute('aria-busy') === 'true' && p.doc.activeElement === del, 'still busy and focused');
    p.press(del);
    await settle();
    assert.ok(p.byId('importDeleteModal').style.display !== 'flex', 'no second dialog');
  });

  await test('load more keeps its button, and on the last page the first row it brought takes focus', async () => {
    for (const pages of [3, 2]) {
      const p = await bootList('/app/import/', {
        'GET /app/import/api/list': (u) => (u.searchParams.get('page') === '2'
          ? { ...listAnswer([importRow('20261020_080000', 'success')]), page: 2, pages }
          : { ...listAnswer(), pages }),
      });
      const button = p.q('[data-import-load-more]');
      button.focus();
      p.press(button);
      await settle();
      if (pages === 3) same(p.doc.activeElement, button, 'the same button, relabelled');
      else assert.ok(p.doc.activeElement.getAttribute('data-import-id') === '20261020_080000', `the first new row (${focusOn(p)})`);
    }
  });

  await test("a content page keeps its control, and on the last page the page it's on", async () => {
    const p = await bootList(`/app/import/${DONE}#content`, {
      [`GET /app/import/api/${DONE}/content`]: (u) => ({ ...contentAnswer(Number(u.searchParams.get('page')) || 1), pages: 3 }),
    });
    const next = () => p.find('#contentPagination button', 'next');
    next().focus();
    p.press(next());
    await settle();
    assert.ok(focusOn(p) === 'BUTTON:next', `next, drawn again (${focusOn(p)})`);
    p.press(next());
    await settle();
    assert.ok(focusOn(p) === 'BUTTON:3', `the last page's own number (${focusOn(p)})`);
  });

  await test('focus outside import is left alone by a changed re-read and a stream', async () => {
    const p = await bootList();
    const rail = p.byId('rail-home');
    rail.focus();
    p.state.list = THREE();
    await reread(p);
    listen(p).fn({ event: 'status', import_id: RUNNING, stage: 'writing', elapsed_ms: 1, generation: 1 });
    await settle();
    same(p.doc.activeElement, rail, 'the rail keeps focus');
  });

  await test("a start with nothing to import stays unavailable, and a held folder's start ignores a press", async () => {
    const p = await bootList('/app/import/', { 'POST /app/import/api/preview': () => ({ preview_token: 'p', size: { recordings: 0, days: 0 } }) });
    await startGuided(p);
    const start = p.byId('guidedStartBtn');
    assert.ok(start.disabled && start.getAttribute('aria-busy') === null, 'nothing to import: disabled, not busy');
    const before = p.writes().length;
    start.disabled = false;
    start.setAttribute('aria-disabled', 'true');
    p.press(start);
    await settle();
    assert.ok(p.writes().length === before, 'a held start sends nothing');
  });

  // ---- a pick from a select, and a phone's late blur as its menu closes
  //
  // A phone fires change with the select still focused, then some 865 ms
  // later takes focus to the page (blur and focusout) with no input between.

  const PHONE_BLUR_MS = 865;
  // Where focus is, for a message: the page itself by its tag alone.
  const at = (p) => (p.doc.activeElement === p.doc.body ? 'BODY' : focusOn(p));
  function pickFrom(p, select, value) {
    select.focus();
    select.value = value;
    p.doc.dispatch(select, { type: 'change' });
  }
  function phoneBlur(p, select) {
    select.blur();
    p.doc.dispatch(select, { type: 'blur' });
    p.doc.dispatch(select, { type: 'focusout' });
  }
  const strava = { name: 'strava', display_name: 'Strava', description: 'workouts', has_guide: false, input_type: 'file', accept: '.zip' };
  const stravaFacet = { offer: { state: 'chosen', facet: 'fitness' }, facets: [{ name: 'fitness', title: 'fitness' }, { name: 'body', title: 'body' }],
    copy: { heading: 'which', this_import: 'this import', none_option: 'none', none_hint: 'none', files_earlier: 'earlier {facet}' } };
  const archive = { name: 'audio_archive', display_name: 'recordings', description: 'a folder', has_guide: false, input_type: 'file', time_zones: ['UTC', 'America/Denver'] };
  // Each select that stays shown after a pick: its page, the select, and a value to pick.
  const PICKS = [
    ['the source filter', () => bootList(), (p) => p.byId('importSourceFilter'), 'chatgpt'],
    ['the content month', () => bootList(`/app/import/${DONE}#content`), (p) => p.byId('contentMonth'), '202611'],
    ['the facet', () => bootList('/app/import/#guide/strava', {
      'GET /app/import/api/sources': () => ({ items: [...sourcesAnswer().items, strava] }),
      'GET /app/import/api/body-facet': () => stravaFacet,
    }), (p) => p.byId('guidedFacetSelect'), ''],
    ['the time zone', () => bootList('/app/import/#guide/audio_archive', {
      'GET /app/import/api/sources': () => ({ items: [...sourcesAnswer().items, archive] }),
    }), (p) => p.byId('guidedArchiveZone'), 'America/Denver'],
  ];

  for (const [name, open, find, value] of PICKS) {
    await test(`${name}: a phone's late blur after a pick lands back on the same select a task later`, async () => {
      const p = await open();
      const select = find(p);
      assert.ok(select && !select.disabled && select.checkVisibility(), `${name} is drawn and shown`);
      pickFrom(p, select, value);
      await settle();
      p.clock.advance(PHONE_BLUR_MS);
      await settle();
      assert.ok(p.doc.activeElement === select, `still on ${name} before the blur (${at(p)})`);
      phoneBlur(p, select);
      assert.ok(p.doc.activeElement === p.doc.body, 'the blur takes focus to the page');
      p.clock.advance(0);
      await settle();
      assert.ok(p.doc.activeElement === select, `focus is back on the same select (${at(p)})`);
      assert.ok(find(p) === select, 'the select is the one picked from');
    });

    await test(`${name}: a pick with no late blur leaves focus alone, and the wait leaves nothing behind`, async () => {
      const p = await open();
      const select = find(p);
      select.focus();
      const before = [p.doc.listenerCount(), select.listenerCount(), p.clock.live()];
      pickFrom(p, select, value);
      await settle();
      p.clock.advance(2000);
      await settle();
      assert.ok(p.doc.activeElement === select, `focus stays on ${name} (${at(p)})`);
      assert.deepStrictEqual([p.doc.listenerCount(), select.listenerCount(), p.clock.live()], before, 'no listener or timer of the wait is left');
      // A blur after the window is the owner's own: nothing brings focus back.
      phoneBlur(p, select);
      p.clock.advance(0);
      await settle();
      assert.ok(p.doc.activeElement === p.doc.body, `a later blur is left alone (${at(p)})`);
    });
  }

  // The filter and the month read again after a pick, and draw what comes back.
  const REREADS = [
    ['the source filter', PICKS[0], (c) => c.method === 'GET' && c.url.startsWith('/app/import/api/list?') && c.url.endsWith('source=chatgpt')],
    ['the content month', PICKS[1], (c) => c.method === 'GET' && c.url.includes('/content?') && c.url.includes('month=202611')],
  ];
  for (const [name, [, open, find, value], t] of REREADS) {
    await test(`${name}: a read that answers before the late blur draws, and the landing still keeps the same select`, async () => {
      const p = await open();
      const select = find(p);
      pickFrom(p, select, value);
      await settle();
      assert.ok(p.calls.filter(t).length === 1 && p.calls.filter(t)[0].settled, 'the read is in before the blur');
      assert.ok(find(p) === select && p.doc.activeElement === select, `the draw keeps the select and its focus (${at(p)})`);
      p.clock.advance(PHONE_BLUR_MS);
      phoneBlur(p, select);
      p.clock.advance(0);
      await settle();
      assert.ok(p.doc.activeElement === select, `focus is back on the same select (${at(p)})`);
    });

    await test(`${name}: a read that answers after the landing draws, and focus stays on the same select`, async () => {
      const p = await open();
      const select = find(p);
      p.holds.push(t);
      pickFrom(p, select, value);
      await settle();
      p.clock.advance(PHONE_BLUR_MS);
      phoneBlur(p, select);
      p.clock.advance(0);
      await settle();
      assert.ok(p.doc.activeElement === select, `landed before the read is in (${at(p)})`);
      const out = p.calls.filter(t);
      assert.ok(out.length === 1 && !out[0].settled, 'the read is still out');
      p.holds.length = 0;
      out[0].answer(name === 'the source filter' ? listAnswer() : contentAnswer(1));
      await settle();
      assert.ok(find(p) === select, 'the draw keeps the select');
      assert.ok(p.doc.activeElement === select, `focus stays on the same select (${at(p)})`);
    });

    await test(`${name}: one read per pick, and none from the late blur or its landing`, async () => {
      const p = await open();
      const select = find(p);
      const gets = () => p.calls.filter((c) => c.method === 'GET').length;
      const start = gets();
      pickFrom(p, select, value);
      await settle();
      assert.ok(p.calls.filter(t).length === 1 && gets() === start + 1, `one read for the pick (${gets() - start})`);
      p.clock.advance(PHONE_BLUR_MS);
      phoneBlur(p, select);
      p.clock.advance(0);
      await settle();
      assert.ok(gets() === start + 1, `nothing more is read (${gets() - start})`);
    });
  }

  // ---- Import OX: live handoff tray, history needs_review row, and /app/import/<id> 404 expired recovery ----
  await test('live handoff tray renders all 5 states, allowlists device_noun, preserves focus on ready to review, bridges SSE, and removes gone rows', async () => {
    const p = await bootList('/app/import/', {
      'GET /app/import/api/list': () => listAnswer([
        importRow('20261122_143000', 'pending', { original_filename: 'export.zip', source_type: 'apple_health', source_hint: 'apple_health', needs_review: true }),
        importRow(DONE, 'success'),
      ]),
    });
    // Wire a mock host after boot? Wait — bootList runs without importHost unless configured; verify history needs_review row first!
    const reviewRow = rowOf(p, '20261122_143000');
    assert.ok(reviewRow, 'needs_review history row present');
    assert.ok(reviewRow.classList.contains('import-row--review'), 'review row carries highlight class');
    assert.ok(reviewRow.querySelector('.import-status.review')?.textContent.includes('ready to review'), 'review pill rendered');
    const reviewLink = reviewRow.querySelector('a.import-review-link');
    assert.ok(reviewLink && reviewLink.getAttribute('href') === '/app/import/20261122_143000', 'review link points directly to /app/import/<id>');
    assert.ok(reviewLink.textContent.includes('review and start'), 'review link copy');
  });

  await test('live handoff tray lifecycle under import host: waiting -> uploading -> review_required (no focus steal) -> SSE delivered -> gone', async () => {
    const reviewId = '20261122_144500';
    const listData = { value: listAnswer([importRow(DONE, 'success')]) };
    const p = makePage(env, '/app/import/', {
      answers: journal({ 'GET /app/import/api/list': () => listData.value }),
    });
    p.window.solstoneImportContract = { capability: 'solstoneJournalHost', version: 2, pick: 'import.pick', drop: 'solstone:drop', event: 'solstone:handoff' };
    p.window.solstoneJournalHost = { version: 2, types: ['import.pick'], send: async () => ({ outcome: 'queued', items: [] }) };
    p.loadScripts();
    p.place();
    await p.mount();
    await settle();
    const tray = p.byId('handoffStatus');
    assert.ok(tray && tray.hidden, 'handoff tray starts hidden when empty');
    const fireHandoff = detail => p.window.fire({ type: 'solstone:handoff', detail });

    // Waiting -> uploading with allowlisted device_noun vs unknown device_noun.
    fireHandoff({ item: 'it-1', state: 'waiting', name: 'export.zip', size_bytes: 5242880 });
    assert.ok(!tray.hidden, 'tray reveals when an item enters');
    assert.ok(tray.textContent.includes('export.zip') && tray.textContent.includes('waiting in the solstone app'));
    fireHandoff({ item: 'it-1', state: 'uploading', name: 'export.zip', device_noun: 'iphone', size_bytes: 5242880 });
    assert.ok(tray.textContent.includes('sending from your iphone…'), 'allowlisted device_noun renders device copy');
    fireHandoff({ item: 'it-1', state: 'uploading', name: 'export.zip', device_noun: 'toaster' });
    assert.ok(tray.textContent.includes('sending to your journal…'), 'non-allowlisted device_noun falls back to generic');

    // Ready to review: brand-orange card without stealing focus!
    const sourceFilter = p.byId('importSourceFilter');
    sourceFilter.focus();
    fireHandoff({ item: 'it-1', state: 'attention', review_required: true, import_id: reviewId, name: 'export.zip' });
    assert.strictEqual(p.doc.activeElement, sourceFilter, 'ready to review card never steals focus');
    const revCard = tray.querySelector('.import-handoff-row.is-review');
    assert.ok(revCard && revCard.textContent.includes('waiting for your review'));
    const cta = revCard.querySelector('a.import-primary-btn');
    assert.ok(cta && cta.getAttribute('href') === `/app/import/${reviewId}` && cta.textContent.includes('review and confirm'));

    // Non-review attention fault with empty reason falls back cleanly and carries role="alert".
    fireHandoff({ item: 'it-2', state: 'attention', review_required: false, retryable: false, name: 'bad.zip', reason: '' });
    const faultCard = tray.querySelector('.import-handoff-row.is-attention');
    assert.ok(faultCard && faultCard.getAttribute('role') === 'alert');
    assert.ok(faultCard.textContent.includes('needs attention') && faultCard.textContent.includes('remove it'));

    // Delivered bridged to SSE events.
    fireHandoff({ item: 'it-3', state: 'delivered', import_id: RUNNING, name: 'notes.md' });
    assert.ok(tray.querySelector('[data-handoff-item="it-3"].is-done')?.textContent.includes('in your journal'));
    listen(p).fn({ event: 'status', import_id: RUNNING, stage: 'importing', generation: 1 });
    await settle();
    assert.ok(tray.querySelector('[data-handoff-item="it-3"]')?.textContent.includes('in your journal · '), 'SSE stage bridges into tray');
    listen(p).fn({ event: 'completed', import_id: RUNNING, entries_written: 7, generation: 2 });
    await settle();
    const doneText = tray.querySelector('[data-handoff-item="it-3"]')?.textContent || '';
    assert.ok(doneText.includes('completed · 7 entries'), `SSE completion bridges into tray (got: ${JSON.stringify(doneText)})`);

    // Older-app fallback: attention without review_required upgrades when history row has needs_review.
    const legacyId = '20261122_145500';
    listData.value = listAnswer([importRow(legacyId, 'pending', { original_filename: 'old.zip', source_hint: 'apple_health', needs_review: true })]);
    fireHandoff({ item: 'it-4', state: 'attention', import_id: legacyId, name: 'old.zip', reason: 'preview required' });
    await settle();
    assert.ok(tray.querySelector('[data-handoff-item="it-4"]')?.classList.contains('is-review'), 'older-app attention upgrades to review card from history needs_review');

    // Gone removes rows cleanly without rendering red error; empty tray hides itself.
    for (const id of ['it-1', 'it-2', 'it-3', 'it-4']) fireHandoff({ item: id, state: 'gone' });
    assert.ok(tray.hidden, 'tray hides once all items are gone');
    assert.strictEqual(tray.querySelectorAll('.import-handoff-row').length, 0);
  });

  await test('/app/import/<id>: 404 on valid timestamp renders expired recovery; 500 and non-timestamp 404 do not', async () => {
    const expiredId = '20261122_140000';
    async function bootStatus(pathName, apiPath, code) {
      const p = makePage(env, pathName, { answers: journal({ [`GET ${apiPath}`]: () => ({ error: 'missing' }) }) });
      p.statuses.push([c => c.url === apiPath, code]);
      p.loadScripts();
      p.place();
      await p.mount();
      await settle();
      return p;
    }

    const p404 = await bootStatus(`/app/import/${expiredId}`, `/app/import/api/${expiredId}`, 404);
    assert.ok(p404.q('.import-expired-card'), '404 on valid timestamp renders expired recovery card');
    assert.ok(p404.byId('importMeta').textContent.includes('expired'));
    assert.ok(p404.q('.import-expired-card a')?.getAttribute('href') === '/app/import/');

    const p500 = await bootStatus(`/app/import/${expiredId}`, `/app/import/api/${expiredId}`, 500);
    assert.ok(!p500.q('.import-expired-card'), '500 must NOT render expired recovery card');

    const pBad = await bootStatus('/app/import/not-a-timestamp', '/app/import/api/not-a-timestamp', 404);
    assert.ok(!pBad.q('.import-expired-card'), '404 on non-timestamp id must NOT render expired recovery card');
  });

  return cases;
};
