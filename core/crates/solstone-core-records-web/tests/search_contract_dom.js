// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Search on the view contract, under plain Node: the real fragment, mounted
// through the real view contract and read store, in a stand-in page with a
// form, events, focus, a history and a clock the cases move by hand.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const ORIGIN = 'http://journal.test';
const VOID_TAGS = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);

// ---- a small document

class Target {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, fn, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    const list = this.listeners.get(type) || [];
    if (!list.some((l) => l.fn === fn && l.capture === capture)) list.push({ fn, capture });
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
    for (const { fn } of [...(this.listeners.get(event.type) || [])]) fn.call(this, event);
  }
}

class TextNode {
  constructor(text) { this.nodeType = 3; this.data = String(text); this.parentNode = null; }
  get textContent() { return this.data; }
  set textContent(v) { this.data = String(v); }
}

function simpleMatch(el, part) {
  const re = /([a-zA-Z0-9-]+)|#([\w-]+)|\.([\w-]+)|\[([\w-]+)(?:=["']?([^\]"']*)["']?)?\]/g;
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
    this.open = false;
    const self = this;
    this.classList = {
      list: () => (self.getAttribute('class') || '').split(/\s+/).filter(Boolean),
      add(c) { const l = this.list(); if (!l.includes(c)) l.push(c); self.setAttribute('class', l.join(' ')); },
      remove(c) { self.setAttribute('class', this.list().filter((x) => x !== c).join(' ')); },
      contains(c) { return this.list().includes(c); },
      toggle(c, on) { if (on === undefined ? !this.contains(c) : on) this.add(c); else this.remove(c); },
    };
    this.dataset = new Proxy({}, {
      get: (_, key) => self.getAttribute('data-' + String(key).replace(/[A-Z]/g, (c) => '-' + c.toLowerCase())) ?? undefined,
      set: (_, key, v) => { self.setAttribute('data-' + String(key).replace(/[A-Z]/g, (c) => '-' + c.toLowerCase()), v); return true; },
    });
  }
  get children() { return this.childNodes.filter((n) => n.nodeType === 1); }
  get id() { return this.getAttribute('id') || ''; }
  get className() { return this.getAttribute('class') || ''; }
  set className(v) { this.setAttribute('class', v); }
  get type() { return this.getAttribute('type') || ''; }
  set type(v) { this.setAttribute('type', v); }
  get href() {
    const raw = this.getAttribute('href');
    return raw === null ? '' : new URL(raw, this.ownerDocument.location.href).href;
  }
  set href(v) { this.setAttribute('href', v); }
  get target() { return this.getAttribute('target') || ''; }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(v) { if (v) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  setAttribute(k, v) { this.attrs.set(k, String(v)); }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  hasAttribute(k) { return this.attrs.has(k); }
  removeAttribute(k) { this.attrs.delete(k); }
  get textContent() { return this.childNodes.map((n) => n.textContent).join(''); }
  set textContent(v) { this.replaceChildren(); if (String(v)) this.append(String(v)); }
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
  adopt(node) {
    if (node.parentNode) node.parentNode.removeChild(node);
    node.parentNode = this;
    return node;
  }
  append(...nodes) {
    for (let node of nodes) {
      if (node === null || node === undefined) continue;
      if (typeof node === 'string') node = new TextNode(node);
      if (node.nodeType === 11) {
        const kids = [...node.childNodes];
        node.childNodes = [];
        kids.forEach((k) => { k.parentNode = this; this.childNodes.push(k); });
        continue;
      }
      this.childNodes.push(this.adopt(node));
    }
  }
  appendChild(node) { this.append(node); return node; }
  removeChild(node) {
    const i = this.childNodes.indexOf(node);
    if (i >= 0) this.childNodes.splice(i, 1);
    node.parentNode = null;
    this.ownerDocument.dropFocusFrom(node);
    return node;
  }
  remove() { if (this.parentNode) this.parentNode.removeChild(this); }
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
  focus() { if (this.isConnected) this.ownerDocument.activeElement = this; }
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

// A tiny parser for the fragment's own markup and the excerpts' emphasis.
function parseInto(doc, parent, html) {
  const stack = [parent];
  const re = /<!--[\s\S]*?-->|<\/([a-zA-Z0-9]+)\s*>|<([a-zA-Z0-9]+)((?:\s+[^\s=>]+(?:=(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*\/?>|([^<]+)/g;
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
        el.setAttribute(a[1], v.replace(/&quot;/g, '"').replace(/&amp;/g, '&'));
      }
      top.append(el);
      if (!VOID_TAGS.has(m[2].toLowerCase())) stack.push(el);
      continue;
    }
    if (m[4]) top.append(m[4].replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&amp;/g, '&'));
  }
}

function makeDocument(location) {
  const doc = new Target();
  doc.location = location;
  doc.loads = [];
  doc.title = 'search - journal';
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
    }, init);
    const path = [];
    for (let at = target; at; at = at.parentNode) path.push(at);
    path.forEach((node) => { if (node.fire) node.fire(event); });
    doc.fire(event);
    return event;
  };
  return doc;
}

class DOMParser {
  constructor(doc) { this.doc = doc; }
  parseFromString(html) {
    const body = this.doc.createElement('body');
    parseInto(this.doc, body, String(html));
    return { body };
  }
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

// ---- one page with search in it

function makePage({ manifestDir, searchHtml, dateFormatSource }, address = '/app/search/', { coarse = false } = {}) {
  let current = new URL(address, ORIGIN);
  const location = {
    get origin() { return ORIGIN; },
    get href() { return current.href; },
    get pathname() { return current.pathname; },
    get search() { return current.search; },
    get hash() { return current.hash; },
    set(url) { current = new URL(url, current.href); },
  };
  const doc = makeDocument(location);
  const clock = makeClock();
  const calls = [];
  const marks = [];
  const window = new Target();
  const entries = [{ url: current.href, state: { solRoute: { id: 'e0', owner: 'router' } } }];
  let index = 0;
  let serial = 0;
  window.history = {
    get state() { return entries[index].state; },
    get length() { return entries.length; },
    pushState(state, _t, url) {
      entries.splice(index + 1);
      entries.push({ url: new URL(url, current.href).href, state });
      index += 1;
      location.set(entries[index].url);
    },
    replaceState(state, _t, url) {
      entries[index] = { url: url === undefined ? entries[index].url : new URL(url, current.href).href, state };
      location.set(entries[index].url);
    },
    go(delta) {
      const next = index + delta;
      if (next < 0 || next >= entries.length) return;
      index = next;
      location.set(entries[index].url);
      window.fire({ type: 'popstate', state: entries[index].state });
    },
    back() { this.go(-1); },
    forward() { this.go(1); },
  };
  // The router's part of a view's own entry: an entry id of its own.
  window.solRouter = {
    viewEntry(kind, url) {
      if (kind === 'push') {
        serial += 1;
        window.history.pushState({ solRoute: { id: 'v' + serial, owner: 'view' } }, '', url);
      } else {
        window.history.replaceState(window.history.state, '', url);
      }
    },
  };
  window.matchMedia = (query) => ({ matches: coarse && query === '(pointer: coarse)' });
  window.scrollY = 0;
  window.scrollTo = (_x, y) => { window.scrollY = y; };
  window.location = location;
  window.document = doc;
  window.setTimeout = clock.setTimeout;
  window.clearTimeout = clock.clearTimeout;
  window.setInterval = clock.setInterval;
  window.clearInterval = clock.clearInterval;
  window.beginViewReady = (view, detail) => {
    let done = false;
    return (state) => { if (!done) { done = true; marks.push(Object.assign({ view, state }, detail)); } };
  };
  window.apiJson = (url, opts = {}) => new Promise((resolve, reject) => {
    const call = { url, opts, aborted: false, settled: false,
      answer(body) { if (!call.settled) { call.settled = true; resolve(body); } },
      fail(error) { if (!call.settled) { call.settled = true; reject(error); } } };
    if (opts.signal) {
      const onAbort = () => { call.aborted = true; call.fail(abortError()); };
      if (opts.signal.aborted) onAbort();
      else opts.signal.addEventListener('abort', onAbort, { once: true });
    }
    calls.push(call);
  });
  window.logError = (error) => { throw error; };

  // The shell around the view: a rail with search and home, and <main>.
  parseInto(doc, doc.body, '<nav id="app-rail"><a href="/app/home/" data-app-name="home">home</a><a href="/app/search/" data-app-name="search">search</a></nav><main id="main-content" tabindex="-1"></main>');
  const main = doc.getElementById('main-content');
  const markup = searchHtml.replace(/<script>[\s\S]*?<\/script>/, '').replace(/<style>[\s\S]*?<\/style>/, '');
  parseInto(doc, main, markup);

  const context = vm.createContext({
    window, document: doc, location, history: window.history,
    DOMParser: function () { return new DOMParser(doc); },
    URL, URLSearchParams, AbortController, DOMException, WeakMap, Map, Set, Promise,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error,
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
  });
  context.self = window;
  const shell = path.resolve(manifestDir, '../solstone-core-convey-shell/assets/static');
  vm.runInContext(dateFormatSource, context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'views.js'), 'utf8'), context);
  const before = { calls: calls.length, html: main.textContent };
  vm.runInContext(searchHtml.match(/<script>([\s\S]*?)<\/script>/)[1], context);
  const loadedQuietly = calls.length === before.calls && main.textContent === before.html;

  const ready = [];
  const page = {
    window, doc, main, clock, calls, marks, entries, location, ready, loadedQuietly,
    index: () => index,
    root: () => doc.getElementById('journal-search'),
    q: (s) => doc.querySelector(s),
    searches: () => calls.filter((c) => c.url.startsWith('/app/search/api/search?')),
    async mount() {
      page.view = await window.SolViews.mount('search', { reportReady: (s) => ready.push(s) });
      await flush();
      return page.view;
    },
    // Another view takes the page, as the shell's place() does.
    leave(next = '/app/home/') {
      window.history.pushState({ solRoute: { id: 'r' + (++serial), owner: 'router' } }, '', next);
    },
    replaceView() {
      window.SolViews.dispose();
      main.replaceChildren();
      parseInto(doc, main, '<section id="home"><h1>home</h1></section>');
    },
    submit() {
      const form = doc.querySelector('form');
      const event = doc.dispatch(form, { type: 'submit' });
      if (!event.defaultPrevented) doc.loads.push('form');
      return event;
    },
    type(name, value) {
      const field = doc.querySelector(`[name=${name}]`);
      field.value = value;
      doc.dispatch(field, { type: 'input' });
    },
  };
  return page;
}

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

async function settle() {
  for (let i = 0; i < 5; i += 1) await flush();
}

function answer(days, extra = {}) {
  return Object.assign({
    total: days.reduce((n, d) => n + d.total, 0),
    total_days: days.length,
    days,
  }, extra);
}

function day(d, hits = 1, total = hits) {
  return {
    day: d,
    total,
    showing: hits,
    results: Array.from({ length: hits }, (_, i) => ({
      path: `${d}/ja1r/0900${i}0_300/audio.jsonl`, stream: 'ja1r', day: d,
      text: `hit <strong>${i}</strong>`, entry_id: `e${i}`, idx: i,
    })),
  };
}

const ALPHA = answer([day('20260403', 2, 9), day('20260402', 1)], { total_days: 25 });
const BETA = answer([day('20260310', 1)]);

module.exports = async function searchContractCases(env) {
  const { test } = env;
  const page = (address, options) => makePage(env, address, options);

  await test('the script only registers: nothing reads or draws at load', async () => {
    const p = page('/app/search/?q=alpha');
    assert.ok(p.loadedQuietly, 'no read and no drawing when the script loads');
    assert.strictEqual(p.window.SolViews.registered('search'), true);
  });

  await test('a mount reads through the store and draws its answer', async () => {
    const p = page('/app/search/?q=alpha&from=&to=');
    await p.mount();
    assert.strictEqual(p.q('[name=q]').value, 'alpha');
    const [call] = p.searches();
    assert.ok(call.url.includes('q=alpha') && call.url.includes('limit=5') && call.url.includes('offset=0'), call.url);
    assert.strictEqual(p.view.ctx.held().reads, 1, 'the read is the mount\'s');
    assert.strictEqual(p.root().getAttribute('aria-busy'), 'true');
    call.answer(ALPHA);
    await settle();
    assert.strictEqual(p.q('.search-results').querySelectorAll('section').length, 2);
    assert.strictEqual(p.root().hasAttribute('aria-busy'), false);
    assert.deepStrictEqual(p.ready, ['content']);
  });

  await test('an empty address reads nothing and is ready at once', async () => {
    const p = page('/app/search/');
    await p.mount();
    assert.strictEqual(p.searches().length, 0);
    assert.deepStrictEqual(p.ready, ['content']);
  });

  await test('dispose mid-search leaves no timer, no read, no busy mark, and draws nothing', async () => {
    const p = page('/app/search/?q=alpha');
    const timersBefore = p.clock.live();
    await p.mount();
    p.clock.advance(3000);
    assert.ok(p.q('.search-status').textContent.startsWith('still searching'), 'the slow read says so');
    const held = p.view.ctx.held();
    assert.strictEqual(held.reads, 1);
    assert.ok(held.timeouts + held.intervals >= 1, 'a progress timer is the mount\'s');
    const call = p.searches()[0];
    const statusText = p.q('.search-status').textContent;
    p.leave();
    p.replaceView();
    assert.strictEqual(call.aborted, true, 'the read is aborted when the view goes');
    assert.strictEqual(p.clock.live(), timersBefore, 'no timer outlives the view');
    const after = p.view.ctx.held();
    assert.strictEqual(after.timeouts + after.intervals + after.reads + after.listeners, 0);
    call.answer(ALPHA);
    await settle();
    p.clock.advance(20000);
    assert.strictEqual(p.doc.querySelectorAll('[aria-busy]').length, 0, 'no busy mark on the next view');
    assert.ok(!p.main.textContent.includes('still searching'), 'no still-searching text on the next view');
    assert.ok(statusText.startsWith('still searching'));
    assert.strictEqual(p.doc.querySelectorAll('.search-day').length, 0, 'nothing drawn');
  });

  await test('an answer that arrives after the address has moved on draws nothing', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    p.leave('/app/transcripts/20260403');
    p.searches()[0].answer(ALPHA);
    await settle();
    assert.strictEqual(p.q('.search-results').querySelectorAll('section').length, 0);
    p.clock.advance(10000);
    assert.ok(!p.q('.search-status').textContent.startsWith('still searching'), 'no tick on a view being left');
  });

  await test('a submit is one in-page entry, aborts the old read, and a late old answer is not drawn', async () => {
    const p = page('/app/search/?q=alpha&from=&to=');
    await p.mount();
    const old = p.searches()[0];
    const length = p.entries.length;
    p.q('[name=q]').focus();
    p.q('[name=q]').value = 'beta';
    const event = p.submit();
    assert.strictEqual(event.defaultPrevented, true, 'no page load');
    assert.strictEqual(p.doc.loads.length, 0);
    assert.strictEqual(p.entries.length, length + 1, 'one entry');
    assert.strictEqual(p.location.search, '?q=beta&from=&to=', 'the address the form builds');
    assert.strictEqual(old.aborted, true, 'the old read is aborted');
    old.answer(ALPHA);
    const fresh = p.searches()[1];
    assert.ok(fresh.url.includes('q=beta'));
    fresh.answer(BETA);
    await settle();
    const days = p.q('.search-results').querySelectorAll('h2').map((h) => h.textContent);
    assert.strictEqual(days.length, 1, 'only the new answer is drawn');
    assert.strictEqual(p.doc.activeElement, p.q('[name=q]'), 'focus stays in the field');
    assert.deepStrictEqual(p.marks.map((m) => [m.view, m.state, m.update]), [['search', 'content', true]]);
  });

  await test('submitting the same address again reads again without a new entry', async () => {
    const p = page('/app/search/?q=alpha&from=&to=');
    await p.mount();
    p.searches()[0].answer(ALPHA);
    await settle();
    const length = p.entries.length;
    p.submit();
    assert.strictEqual(p.entries.length, length);
    assert.strictEqual(p.searches().length, 2, 'a fresh read');
  });

  await test('a paging link is one entry; focus moves to the first new heading, never to the page', async () => {
    const p = page('/app/search/?q=alpha&from=&to=');
    await p.mount();
    p.searches()[0].answer(ALPHA);
    await settle();
    const older = p.q('.search-paging').querySelector('a');
    assert.strictEqual(older.textContent, 'older matching days');
    older.focus();
    const length = p.entries.length;
    const event = older.click();
    assert.strictEqual(event.defaultPrevented, true);
    assert.strictEqual(p.doc.loads.length, 0, 'no page load');
    assert.strictEqual(p.entries.length, length + 1);
    assert.ok(p.location.search.includes('offset=20'), p.location.search);
    assert.strictEqual(p.doc.activeElement, p.main, 'focus waits on <main> while the page reads');
    const next = p.searches()[1];
    assert.ok(next.url.includes('offset=20'));
    next.answer(answer([day('20260301', 1)], { total_days: 25 }));
    await settle();
    const heading = p.q('.search-results').querySelector('h2');
    assert.strictEqual(p.doc.activeElement, heading, 'focus is on the first day heading');
    assert.notStrictEqual(p.doc.activeElement, p.doc.body);
  });

  await test('"more from this day" is one entry for that day', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    p.searches()[0].answer(ALPHA);
    await settle();
    const more = p.q('.search-results').querySelectorAll('a').find((a) => a.textContent === 'more from this day');
    more.click();
    assert.strictEqual(p.location.search, '?q=alpha&from=2026-04-03&to=2026-04-03');
    assert.ok(p.searches()[1].url.includes('limit=100'), 'a day\'s page');
  });

  await test('a page with no results keeps focus on <main>', async () => {
    const p = page('/app/search/?q=alpha&offset=40');
    await p.mount();
    p.searches()[0].answer(answer([], { total: 12, total_days: 25 }));
    await settle();
    const back = p.q('.search-paging').querySelectorAll('a').find((a) => a.textContent === 'back to first results');
    back.focus();
    back.click();
    p.searches()[1].answer(answer([]));
    await settle();
    assert.strictEqual(p.doc.activeElement, p.main);
  });

  await test('back and forward between two searches restore the form, the results and the scroll, with no new read', async () => {
    const p = page('/app/search/?q=alpha&from=&to=');
    await p.mount();
    p.searches()[0].answer(ALPHA);
    await settle();
    p.window.scrollY = 640;
    p.q('[name=q]').value = 'beta';
    p.submit();
    p.searches()[1].answer(BETA);
    await settle();
    assert.strictEqual(p.window.scrollY, 0, 'a new search starts at the top');
    p.window.scrollY = 80;
    const reads = p.searches().length;
    p.window.history.back();
    await settle();
    assert.strictEqual(p.q('[name=q]').value, 'alpha', 'the form shows the entry\'s query');
    assert.strictEqual(p.q('.search-results').querySelectorAll('section').length, 2, 'the entry\'s results');
    assert.strictEqual(p.window.scrollY, 640, 'the entry\'s scroll');
    assert.strictEqual(p.searches().length, reads, 'restored from what the page already had');
    p.window.history.forward();
    await settle();
    assert.strictEqual(p.q('[name=q]').value, 'beta');
    assert.strictEqual(p.q('.search-results').querySelectorAll('section').length, 1);
    assert.strictEqual(p.window.scrollY, 80);
    assert.strictEqual(p.searches().length, reads);
    assert.strictEqual(p.marks.length, 3, 'each restore is marked ready');
  });

  await test('back to an entry whose answer is not held reads it again', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    const first = p.searches()[0];
    p.q('[name=q]').value = 'beta';
    p.submit();
    assert.strictEqual(first.aborted, true);
    p.searches()[1].answer(BETA);
    await settle();
    p.window.history.back();
    await settle();
    assert.strictEqual(p.searches().length, 3, 'alpha was never answered, so it is read');
  });

  await test('the rail\'s search on a results page is one entry for the empty search', async () => {
    const p = page('/app/search/?q=alpha');
    const shipped = p.q('.search-status').textContent;
    await p.mount();
    p.searches()[0].answer(ALPHA);
    await settle();
    const rail = p.q('#app-rail a[data-app-name="search"]');
    rail.focus();
    const length = p.entries.length;
    const event = rail.click();
    assert.strictEqual(event.defaultPrevented, true, 'no page load');
    assert.strictEqual(p.entries.length, length + 1);
    assert.strictEqual(p.location.href, ORIGIN + '/app/search/');
    assert.strictEqual(p.q('[name=q]').value, '');
    assert.strictEqual(p.q('.search-results').querySelectorAll('section').length, 0);
    assert.strictEqual(p.q('.search-status').textContent, shipped, 'the status the fragment ships with');
    assert.strictEqual(p.searches().length, 1, 'nothing to read');
    assert.strictEqual(p.doc.activeElement, rail);
  });

  await test('links search does not take: the same address, another view, a modified click', async () => {
    const p = page('/app/search/');
    await p.mount();
    assert.strictEqual(p.q('#app-rail a[data-app-name="search"]').click().defaultPrevented, false, 'the same address is the router\'s');
    assert.strictEqual(p.q('#app-rail a[data-app-name="home"]').click().defaultPrevented, false, 'another view is the router\'s');
    p.location.set('/app/search/?q=alpha');
    assert.strictEqual(p.q('#app-rail a[data-app-name="search"]').click({ metaKey: true }).defaultPrevented, false);
    assert.strictEqual(p.q('#app-rail a[data-app-name="search"]').click({ defaultPrevented: true }).defaultPrevented, true);
  });

  await test('a result opens its transcripts segment, map day and week through the router', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    p.searches()[0].answer(answer([{ day: '20260403', total: 3, showing: 3, results: [
      { path: '20260403/ja1r/090000_300/audio.jsonl', stream: 'ja1r', day: '20260403', text: 'a' },
      { path: '20260403/location.md', day: '20260403', text: 'b' },
      { path: 'reflections/weekly/20260403.md', week_url: '/app/home/week/20260403', text: 'c' },
    ] }]));
    await settle();
    const hrefs = p.q('.search-results').querySelectorAll('a.search-link').map((a) => a.getAttribute('href'));
    assert.deepStrictEqual(hrefs, ['/app/transcripts/20260403?stream=ja1r#090000_300', '/app/map/20260403', '/app/home/week/20260403']);
    for (const a of p.q('.search-results').querySelectorAll('a')) {
      assert.strictEqual(a.click().defaultPrevented, false, `${a.getAttribute('href')} is left to the router`);
    }
  });

  await test('an entry read in flight across a move, or a new search, draws nothing', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    p.searches()[0].answer(ALPHA);
    await settle();
    const button = () => p.q('.search-results').querySelector('button');
    p.doc.dispatch(button(), { type: 'click', button: 0 });
    const entry = p.calls.find((c) => c.url.startsWith('/app/search/api/entry?'));
    assert.ok(entry, 'the entry read');
    assert.strictEqual(p.view.ctx.held().reads, 1, 'it is the mount\'s');
    const content = p.q('.search-results').querySelector('pre');
    assert.strictEqual(content.textContent, 'loading entry…');
    p.q('[name=q]').value = 'beta';
    p.submit();
    entry.answer({ content: 'LATE', reindexed_since_search: false });
    await settle();
    assert.ok(!p.main.textContent.includes('LATE'), 'a new search keeps the old entry out');

    const q = page('/app/search/?q=alpha');
    await q.mount();
    q.searches()[0].answer(ALPHA);
    await settle();
    q.doc.dispatch(q.q('.search-results').querySelector('button'), { type: 'click', button: 0 });
    const read = q.calls.find((c) => c.url.startsWith('/app/search/api/entry?'));
    const pre = q.q('.search-results').querySelector('pre');
    q.leave('/app/transcripts/20260403');
    q.replaceView();
    assert.strictEqual(read.aborted, true, 'aborted with the view');
    read.answer({ content: 'LATE' });
    await settle();
    assert.strictEqual(pre.textContent, 'loading entry…', 'nothing drawn after the move');
  });

  await test('an entry read opens, closes and reads once', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    p.searches()[0].answer(ALPHA);
    await settle();
    const button = p.q('.search-results').querySelector('button');
    p.doc.dispatch(button, { type: 'click', button: 0 });
    p.calls.find((c) => c.url.startsWith('/app/search/api/entry?')).answer({ content: 'THE ENTRY', reindexed_since_search: false });
    await settle();
    const pre = p.q('.search-results').querySelector('pre');
    assert.ok(pre.textContent.includes('THE ENTRY'));
    p.doc.dispatch(button, { type: 'click', button: 0 });
    assert.strictEqual(pre.hidden, true);
    p.doc.dispatch(button, { type: 'click', button: 0 });
    assert.strictEqual(p.calls.filter((c) => c.url.startsWith('/app/search/api/entry?')).length, 1);
  });

  await test('try again reads again and puts focus on the first heading', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    p.searches()[0].fail(Object.assign(new Error('boom'), { reasonCode: 'index_locked' }));
    await settle();
    assert.deepStrictEqual(p.ready, ['error']);
    const retry = p.q('.search-status').querySelector('button');
    retry.focus();
    p.doc.dispatch(retry, { type: 'click', button: 0 });
    assert.strictEqual(p.doc.activeElement, p.main);
    p.searches()[1].answer(ALPHA);
    await settle();
    assert.strictEqual(p.doc.activeElement, p.q('.search-results').querySelector('h2'));
  });

  await test('typing while a search runs stops it', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    p.type('q', 'alph');
    assert.strictEqual(p.searches()[0].aborted, true);
    await settle();
    assert.strictEqual(p.root().hasAttribute('aria-busy'), false);
    assert.strictEqual(p.clock.live(), 0);
  });

  await test('a mount that throws part-way takes back what it added', async () => {
    const p = page('/app/search/?q=alpha');
    p.q('#search-date-options').remove();
    const doc = p.doc.listenerCount();
    const win = p.window.listenerCount();
    await assert.rejects(p.window.SolViews.mount('search', {}));
    assert.strictEqual(p.doc.listenerCount(), doc, 'no document listener left');
    assert.strictEqual(p.window.listenerCount(), win, 'no window listener left');
    assert.strictEqual(p.window.SolViews.current(), null);
  });

  await test('on a touch screen a submit lets go of the field, so the keyboard closes', async () => {
    const p = page('/app/search/?q=alpha', { coarse: true });
    await p.mount();
    p.q('[name=q]').focus();
    p.q('[name=q]').value = 'beta';
    p.submit();
    assert.strictEqual(p.doc.activeElement, p.main, 'focus waits on <main>');
    p.searches()[1].answer(BETA);
    await settle();
    assert.strictEqual(p.doc.activeElement, p.main, 'and stays there; the status says what was found');
  });

  await test('held answers outlive the view: back from another view draws at once, a forward arrival reads', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    p.searches()[0].answer(ALPHA);
    await settle();
    // A result's transcripts day, as the router moves: its entry, then the swap.
    p.leave('/app/transcripts/20260403');
    p.replaceView();
    const main = p.main;
    // Back: the router's pop, and search mounted again into fresh markup.
    p.window.history.back();
    main.replaceChildren();
    parseInto(p.doc, main, env.searchHtml.replace(/<script>[\s\S]*?<\/script>/, '').replace(/<style>[\s\S]*?<\/style>/, ''));
    await p.mount();
    assert.strictEqual(p.searches().length, 1, 'no read on back');
    assert.strictEqual(p.q('.search-results').querySelectorAll('section').length, 2, 'the held answer is drawn');
    assert.deepStrictEqual(p.ready.slice(-1), ['content'], 'ready at once, so the router can restore the scroll');
    // A forward arrival at the same address is a new entry: it reads.
    p.leave('/app/home/');
    p.replaceView();
    p.leave('/app/search/?q=alpha');
    main.replaceChildren();
    parseInto(p.doc, main, env.searchHtml.replace(/<script>[\s\S]*?<\/script>/, '').replace(/<style>[\s\S]*?<\/style>/, ''));
    await p.mount();
    assert.strictEqual(p.searches().length, 2, 'a forward arrival reads');
  });

  await test('on a first load the answer is held under the id the router gives the entry after the mount', async () => {
    const p = page('/app/search/?q=alpha');
    p.entries[0].state = null;
    await p.mount();
    p.entries[0].state = { solRoute: { id: 'started', owner: 'router' } };
    p.searches()[0].answer(ALPHA);
    await settle();
    p.window.scrollY = 500;
    p.q('[name=q]').value = 'beta';
    p.submit();
    p.searches()[1].answer(BETA);
    await settle();
    p.window.history.back();
    await settle();
    assert.strictEqual(p.searches().length, 2, 'the first answer was held');
    assert.strictEqual(p.window.scrollY, 500, 'and the first entry\'s scroll');
  });

  await test('a pop to another view\'s address is left to the router', async () => {
    const p = page('/app/search/?q=alpha');
    await p.mount();
    p.searches()[0].answer(ALPHA);
    await settle();
    p.leave('/app/transcripts/20260403');
    p.window.history.back();
    p.window.history.forward();
    await settle();
    assert.strictEqual(p.searches().length, 1, 'search reads nothing for another view\'s entry');
  });
};
