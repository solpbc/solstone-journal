// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The view contract, the read store and the in-page router, on a small
// document with a history, a simulated clock and a server the test answers:
// what a view leaves behind when it goes, what a failed mount takes back,
// which moves stay in the page and which load a new one, who owns each
// history entry, and what a screen reader hears.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');
const ORIGIN = 'http://journal.test';

function staticSource(name) {
  return fs.readFileSync(path.join(crateDir, 'assets/static', name), 'utf8');
}

async function flush() {
  for (let i = 0; i < 30; i += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

function makeClock() {
  let now = 0;
  let sequence = 0;
  const timers = new Map();
  const add = (fn, ms, repeat, args) => {
    sequence += 1;
    const delay = Math.max(0, Number(ms) || 0);
    timers.set(sequence, { at: now + delay, fn, args, every: repeat ? Math.max(1, delay) : 0 });
    return sequence;
  };
  return {
    now: () => now,
    count: () => timers.size,
    setTimeout: (fn, ms, ...args) => add(fn, ms, false, args),
    setInterval: (fn, ms, ...args) => add(fn, ms, true, args),
    clearTimeout: (id) => timers.delete(id),
    clearInterval: (id) => timers.delete(id),
    async advance(ms) {
      const end = now + ms;
      for (;;) {
        await flush();
        let next = null;
        for (const [id, timer] of timers) {
          if (timer.at <= end && (!next || timer.at < next[1].at)) next = [id, timer];
        }
        if (!next) break;
        const [id, timer] = next;
        now = timer.at;
        if (timer.every) timer.at += timer.every;
        else timers.delete(id);
        timer.fn(...(timer.args || []));
      }
      now = end;
      await flush();
    },
  };
}

function abortError() {
  return new DOMException('The operation was aborted.', 'AbortError');
}

// Every call is recorded. A route in `answers` is answered at once; anything
// else waits for the test.
function makeServer() {
  const calls = [];
  // Setting an answer also answers the calls already waiting for it.
  const answers = new (class extends Map {
    set(url, fn) {
      super.set(url, fn);
      if (fn) calls.filter((call) => !call.settled && call.url === url).forEach((call) => setImmediate(() => fn(call)));
      return this;
    }
  })();
  function fetch(input, init = {}) {
    const url = new URL(String(input), ORIGIN + '/');
    return new Promise((resolve, reject) => {
      const call = {
        url: url.pathname + url.search,
        method: String(init.method || 'GET').toUpperCase(),
        aborted: false,
        settled: false,
        respond(status, text = '', headers = {}) {
          if (call.settled) return;
          call.settled = true;
          resolve(new Response(text, { status, headers }));
        },
        failTransport() {
          if (call.settled) return;
          call.settled = true;
          reject(new TypeError('Failed to fetch'));
        },
      };
      const signal = init.signal;
      if (signal) {
        const onAbort = () => {
          call.aborted = true;
          if (!call.settled) {
            call.settled = true;
            reject(abortError());
          }
        };
        if (signal.aborted) onAbort();
        else signal.addEventListener('abort', onAbort, { once: true });
      }
      calls.push(call);
      const answer = answers.get(call.url) || answers.get(url.pathname);
      if (answer) setImmediate(() => answer(call));
    });
  }
  const json = (body, status = 200) => (call) => call.respond(status, JSON.stringify(body), { 'content-type': 'application/json' });
  const html = (body) => (call) => call.respond(200, body, { 'content-type': 'text/html; charset=utf-8' });
  return {
    fetch, calls, answers, json, html,
    of: (url) => calls.filter((call) => call.url === url),
    pending: (url) => calls.filter((call) => call.url === url && !call.settled),
  };
}

// ---- a small document

class FakeEventTarget {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, fn, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    const list = this.listeners.get(type) || [];
    if (list.some((entry) => entry.fn === fn && entry.capture === capture)) return;
    list.push({ fn, capture, once: !!options?.once });
    this.listeners.set(type, list);
  }
  removeEventListener(type, fn, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    const list = this.listeners.get(type) || [];
    this.listeners.set(type, list.filter((entry) => !(entry.fn === fn && entry.capture === capture)));
  }
  dispatchEvent(event) {
    event.currentTarget = this;
    (this.listeners.get(event.type) || []).slice().forEach((entry) => {
      if (entry.once) this.removeEventListener(event.type, entry.fn, entry.capture);
      entry.fn.call(this, event);
    });
    return !event.defaultPrevented;
  }
  listenerCount(type) {
    if (type) return (this.listeners.get(type) || []).length;
    let total = 0;
    this.listeners.forEach((list) => { total += list.length; });
    return total;
  }
}

class ClassList {
  constructor(element) { this.element = element; }
  get values() { return new Set((this.element.getAttribute('class') || '').split(/\s+/).filter(Boolean)); }
  write(values) {
    if (values.size) this.element.setAttribute('class', [...values].join(' '));
    else this.element.removeAttribute('class');
  }
  add(...names) { const v = this.values; names.forEach((n) => v.add(n)); this.write(v); }
  remove(...names) { const v = this.values; names.forEach((n) => v.delete(n)); this.write(v); }
  toggle(name, force) {
    const v = this.values;
    const on = force === undefined ? !v.has(name) : !!force;
    if (on) v.add(name); else v.delete(name);
    this.write(v);
    return on;
  }
  contains(name) { return this.values.has(name); }
}

const VOID = new Set(['link', 'meta', 'br', 'img', 'input']);

class FakeNode extends FakeEventTarget {
  constructor(document, tagName) {
    super();
    this.ownerDocument = document;
    this.tagName = tagName ? tagName.toUpperCase() : '#fragment';
    this.attributeMap = new Map();
    this.children = [];
    this.parent = null;
    this.text = '';
    this.classList = new ClassList(this);
    this.disabled = false;
  }
  get attributes() { return Array.from(this.attributeMap, ([name, value]) => ({ name, value })); }
  setAttribute(name, value) { this.attributeMap.set(name, String(value)); }
  getAttribute(name) { return this.attributeMap.has(name) ? this.attributeMap.get(name) : null; }
  hasAttribute(name) { return this.attributeMap.has(name); }
  removeAttribute(name) { this.attributeMap.delete(name); }
  get id() { return this.getAttribute('id') || ''; }
  get href() { return this.hasAttribute('href') ? new URL(this.getAttribute('href'), this.ownerDocument.location.href).href : ''; }
  get src() { return this.hasAttribute('src') ? new URL(this.getAttribute('src'), ORIGIN).href : ''; }
  get target() { return this.getAttribute('target') || ''; }
  get textContent() { return this.text + this.children.map((child) => child.textContent).join(''); }
  set textContent(value) { this.children = []; this.text = String(value); }
  get isConnected() {
    let node = this;
    while (node.parent) node = node.parent;
    return node === this.ownerDocument.root;
  }
  appendChild(child) {
    if (child.tagName === '#fragment') {
      child.children.slice().forEach((node) => this.appendChild(node));
      return child;
    }
    child.remove();
    child.parent = this;
    this.children.push(child);
    if (this.isConnected) this.ownerDocument.inserted?.(child);
    return child;
  }
  replaceChildren(...nodes) {
    this.children.slice().forEach((child) => child.remove());
    this.text = '';
    nodes.forEach((node) => this.appendChild(node));
  }
  remove() {
    if (!this.parent) return;
    this.parent.children = this.parent.children.filter((child) => child !== this);
    this.parent = null;
  }
  replaceWith(other) {
    const parent = this.parent;
    const index = parent.children.indexOf(this);
    other.remove();
    parent.children[index] = other;
    other.parent = parent;
    this.parent = null;
    if (parent.isConnected) this.ownerDocument.inserted?.(other);
  }
  cloneNode(deep) {
    const copy = new FakeNode(this.ownerDocument, this.tagName === '#fragment' ? null : this.tagName);
    this.attributeMap.forEach((value, name) => copy.setAttribute(name, value));
    copy.text = this.text;
    if (deep) this.children.forEach((child) => { const c = child.cloneNode(true); c.parent = copy; copy.children.push(c); });
    return copy;
  }
  get content() { return this._content; }
  set innerHTML(html) {
    this.children.slice().forEach((child) => child.remove());
    this.text = '';
    parseInto(this, String(html));
    if (this.isConnected) this.walk().forEach((node) => node !== this && this.ownerDocument.inserted?.(node));
  }
  get innerHTML() { return this.textContent; }
  walk() { return [this, ...this.children.flatMap((child) => child.walk())]; }
  querySelectorAll(selector) { return this.walk().slice(1).filter((node) => matches(node, selector)); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  closest(selector) {
    let node = this;
    while (node && node.tagName !== '#fragment') {
      if (matches(node, selector)) return node;
      node = node.parent;
    }
    return null;
  }
  focus() { this.ownerDocument.activeElement = this; }
  // A click bubbles from the element to the document.
  click() {
    const event = { type: 'click', target: this, button: 0, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
    for (let node = this; node; node = node.parent) FakeEventTarget.prototype.dispatchEvent.call(node, event);
    this.ownerDocument.dispatchEvent(event);
    return event;
  }
}

// Markup the tests write: elements with quoted attributes, and text.
function parseInto(root, html) {
  const document = root.ownerDocument;
  const stack = [root];
  const tokens = /<!--[\s\S]*?-->|<\/([a-z0-9]+)\s*>|<([a-z0-9]+)((?:\s+[\w-]+(?:="[^"]*")?)*)\s*\/?>|([^<]+)/gi;
  let match;
  while ((match = tokens.exec(html))) {
    const top = stack[stack.length - 1];
    if (match[0].startsWith('<!--')) continue;
    if (match[1]) {
      while (stack.length > 1 && stack.pop().tagName !== match[1].toUpperCase());
      continue;
    }
    if (match[2]) {
      const element = new FakeNode(document, match[2]);
      const attributes = /([\w-]+)(?:="([^"]*)")?/g;
      let attribute;
      while ((attribute = attributes.exec(match[3] || ''))) element.setAttribute(attribute[1], attribute[2] ?? '');
      element.parent = top;
      top.children.push(element);
      if (match[2].toLowerCase() === 'template') {
        element._content = new FakeNode(document, null);
      }
      if (!VOID.has(match[2].toLowerCase()) && !match[0].endsWith('/>')) stack.push(element);
      continue;
    }
    if (match[4] && match[4].trim()) {
      top.text += match[4];
    }
  }
}

function matchesSimple(node, part) {
  const parsed = /^([a-z0-9]+)?(?:#([\w-]+))?(?:\.([\w-]+))?(?:\[([\w-]+)(?:="?([^"\]]*)"?)?\])?$/i.exec(part);
  if (!parsed) return false;
  const [, tag, id, cls, attribute, value] = parsed;
  if (tag && node.tagName !== tag.toUpperCase()) return false;
  if (id && node.id !== id) return false;
  if (cls && !node.classList.contains(cls)) return false;
  if (attribute && !node.hasAttribute(attribute)) return false;
  if (attribute && value !== undefined && node.getAttribute(attribute) !== value) return false;
  return true;
}

function matches(node, selector) {
  if (!node.tagName || node.tagName === '#fragment') return false;
  return selector.split(',').some((group) => {
    const parts = group.trim().split(/\s+/);
    if (!matchesSimple(node, parts[parts.length - 1])) return false;
    let ancestor = node.parent;
    for (let i = parts.length - 2; i >= 0; i -= 1) {
      while (ancestor && !(ancestor.tagName !== '#fragment' && matchesSimple(ancestor, parts[i]))) ancestor = ancestor.parent;
      if (!ancestor) return false;
      ancestor = ancestor.parent;
    }
    return true;
  });
}

// ---- the history: entries of this document, and one before it

function makeHistory(window, location, record) {
  const entries = [{ url: 'http://elsewhere.test/', state: null, other: true }];
  let index = 0;
  const history = {
    scrollRestoration: 'auto',
    get length() { return entries.length; },
    get state() { return entries[index].state; },
    pushState(state, _title, url) {
      entries.splice(index + 1);
      entries.push({ url: new URL(url, location.href).href, state: structuredClone(state) });
      index += 1;
      location.set(entries[index].url);
    },
    replaceState(state, _title, url) {
      entries[index] = { url: url === undefined ? entries[index].url : new URL(url, location.href).href, state: structuredClone(state) };
      location.set(entries[index].url);
    },
    // A move to an entry of another document is the browser's: this page goes.
    async go(delta) {
      const next = index + delta;
      if (next < 0 || next >= entries.length) return;
      index = next;
      if (entries[index].other) {
        record.left.push(entries[index].url);
        return;
      }
      location.set(entries[index].url);
      window.dispatchEvent({ type: 'popstate', state: entries[index].state });
    },
    back() { return history.go(-1); },
    forward() { return history.go(1); },
    entries,
    index: () => index,
  };
  return history;
}

// ---- one page: the shell scripts under test, with two pilot views

const ENTRY_TABLE = {
  views: {
    home: {
      fragment: '/app/home/workspace',
      scripts: [{ src: '/h/home.js' }],
      styles: [],
      reads: [{ route: '/app/home/', urls: ['/app/home/api/pulse'] }],
      contract: true,
    },
    transcripts: {
      fragment: '/app/transcripts/workspace',
      scripts: [{ src: '/t/transcripts.js' }],
      styles: ['/t/date-nav.css'],
      reads: [{ route: '/app/transcripts/{day}', urls: ['/app/transcripts/api/day/{day}'] }],
      openDay: { route: '/app/transcripts/', url: '/app/transcripts/api/open-day' },
      contract: true,
    },
    entities: {
      fragment: '/app/entities/workspace',
      scripts: [],
      styles: [],
      reads: [],
    },
  },
};

const FRAGMENTS = {
  '/app/home/workspace': '<div class="home"><h2>your day</h2><div id="home-root"></div></div><script src="/h/home.js"></script>',
  '/app/transcripts/workspace': '<link rel="stylesheet" href="/t/date-nav.css"><div class="tr"><h2 data-day-heading>day</h2><div id="tr-root"></div></div><script src="/t/transcripts.js"></script>',
  '/app/entities/workspace': '<div><h2>entities</h2></div>',
};

// Each pilot view adds one of everything the contract takes back, and reads
// through the store. `views` counts mounts, updates and draws.
const VIEW_CODE = {
  '/h/home.js': `
    window.SolViews.register('home', {
      mount(ctx) {
        window.__views.home.mounts += 1;
        ctx.on(document, 'keydown', () => {});
        ctx.on(window, 'resize', () => {});
        ctx.setInterval(() => {}, 60000);
        ctx.setTimeout(() => {}, 30000);
        ctx.listen('supervisor', () => {});
        ctx.onConnectionState(() => {});
        ctx.observe(new window.FakeObserver());
        ctx.global('homeGlobal', () => {});
        ctx.bodyClass('home-open', true);
        if (window.__views.home.throwAfterAdding) throw new Error('home failed part-way');
        ctx.read('/app/home/api/pulse').then((pulse) => {
          window.__views.home.draws.push(pulse);
          document.getElementById('home-root').textContent = String(pulse.n);
          ctx.ready('content');
        }, () => ctx.ready('error'));
        return () => { window.__views.home.disposes += 1; };
      }
    });`,
  '/t/transcripts.js': `
    window.SolViews.register('transcripts', {
      mount(ctx) {
        const views = window.__views.transcripts;
        views.mounts += 1;
        let day = ctx.route.day;
        ctx.on(document, 'keydown', () => {});
        ctx.on(window, 'popstate', () => { views.pops += 1; });
        ctx.setInterval(() => {}, 60000);
        ctx.listen('change', () => {});
        const read = () => {
          const asked = day;
          ctx.read('/app/transcripts/api/day/' + asked).then((answer) => {
            if (asked !== day) return;
            views.draws.push(answer.day);
            document.querySelector('[data-day-heading]').textContent = answer.day;
            ctx.ready(answer.empty ? 'empty' : 'content');
          }, () => ctx.ready('error'));
        };
        window.__openSegment = (key) => ctx.history.push(window.location.pathname + '?stream=a#' + key + '/transcript');
        read();
        return {
          update(params) {
            views.updates.push(params.day);
            day = params.day;
            read();
          },
          dispose() { views.disposes += 1; }
        };
      }
    });`,
};

function headScript() {
  const page = fs.readFileSync(path.join(crateDir, 'assets/static/shell.html'), 'utf8');
  return page.match(/<script>\s*\/\/ The journal's front door[\s\S]*?<\/script>/)[0]
    .replace(/^<script>/, '').replace(/<\/script>$/, '')
    .replace('{/*entry-table*/}', JSON.stringify(ENTRY_TABLE));
}

function makePage({ pathname = '/app/home/', transitions = false } = {}) {
  const clock = makeClock();
  const server = makeServer();
  const record = { assigned: [], reloads: [], left: [], announced: [], skipped: 0, transitions: 0 };
  const storage = new Map();
  const writes = [];
  let current = new URL(pathname, ORIGIN);
  const location = {
    get origin() { return ORIGIN; },
    get pathname() { return current.pathname; },
    get search() { return current.search; },
    get hash() { return current.hash; },
    get href() { return current.href; },
    set href(value) { record.assigned.push(new URL(value, current.href).pathname); },
    set(url) { current = new URL(url, current.href); },
    assign(value) { record.assigned.push(new URL(value, current.href).pathname); window.dispatchEvent({ type: 'pageswap', viewTransition: viewTransitionStub(record) }); },
    reload() { record.reloads.push(current.pathname); window.dispatchEvent({ type: 'pageswap', viewTransition: viewTransitionStub(record) }); },
  };
  const window = new FakeEventTarget();
  const document = new FakeEventTarget();
  document.location = location;
  document.title = 'journal';
  document.visibilityState = 'visible';
  document.hidden = false;
  document.readyState = 'complete';
  document.activeElement = null;
  document.root = new FakeNode(document, 'html');
  document.head = document.root.appendChild(new FakeNode(document, 'head'));
  document.body = document.root.appendChild(new FakeNode(document, 'body'));
  document.inserted = (node) => {
    if (node.tagName === 'SCRIPT' && node.hasAttribute('src')) {
      const src = node.getAttribute('src');
      setImmediate(() => {
        try {
          vm.runInContext(VIEW_CODE[src] || '', window, { filename: src });
        } catch (error) {
          window.__scriptErrors.push(error);
        }
        node.dispatchEvent({ type: 'load' });
      });
    }
    if (node.tagName === 'LINK' && node.getAttribute('rel') === 'stylesheet') {
      setImmediate(() => node.dispatchEvent({ type: 'load' }));
    }
  };
  document.createElement = (tag) => {
    const node = new FakeNode(document, tag);
    if (tag === 'template') node._content = new FakeNode(document, null);
    return node;
  };
  // A template's markup is its content.
  const createElement = document.createElement;
  document.createElement = (tag) => {
    const node = createElement(tag);
    if (tag === 'template') {
      Object.defineProperty(node, 'innerHTML', {
        set(html) { node._content = new FakeNode(document, null); parseInto(node._content, String(html)); },
        get() { return ''; },
      });
    }
    return node;
  };
  document.createTextNode = (text) => { const node = new FakeNode(document, 'span'); node.text = text; return node; };
  document.getElementById = (id) => document.root.walk().find((node) => node.id === id) || null;
  document.querySelector = (selector) => document.root.querySelector(selector);
  document.querySelectorAll = (selector) => document.root.querySelectorAll(selector);
  document.body.innerHTML =
    '<nav id="app-rail"><a href="/app/home/" data-app-name="home">home</a><a href="/app/transcripts/" data-app-name="transcripts">transcripts</a>' +
    '<a href="/app/entities/" data-app-name="entities">entities</a><a href="/app/sources/" data-app-name="sources">sources</a>' +
    '<div id="status-instrument"></div></nav><div id="view-announcer" role="status" aria-live="polite"></div>' +
    '<main id="main-content" tabindex="-1"><div class="surface-state surface-state--loading" role="status" aria-busy="true"></div></main>';
  if (transitions) {
    document.startViewTransition = (update) => {
      record.transitions += 1;
      const done = Promise.resolve().then(update);
      return { updateCallbackDone: done, ready: done, finished: done, skipTransition() {} };
    };
  }

  const history = makeHistory(window, location, record);
  history.pushState(null, '', pathname);
  // The event client: listeners and connection handlers, counted.
  const eventListeners = new Map();
  const connectionHandlers = new Set();
  let connected = false;
  let built = false;
  const appEvents = {
    buildChanged: () => built,
    listen(tract, fn) {
      const list = eventListeners.get(tract) || new Set();
      list.add(fn);
      eventListeners.set(tract, list);
      return () => list.delete(fn);
    },
    onConnectionState(fn) {
      connectionHandlers.add(fn);
      fn({ connected });
      return () => connectionHandlers.delete(fn);
    },
  };
  const observers = new Set();
  class FakeObserver {
    constructor() { observers.add(this); }
    observe() {}
    disconnect() { observers.delete(this); }
  }
  const sessionStorage = {
    getItem: (key) => (storage.has(key) ? storage.get(key) : null),
    setItem: (key, value) => { writes.push(key); storage.set(key, String(value)); },
    removeItem: (key) => storage.delete(key),
  };
  Object.assign(window, {
    window, document, location, history, appEvents, FakeObserver, sessionStorage,
    localStorage: { getItem: () => null, setItem: (key) => writes.push('local:' + key), removeItem() {} },
    fetch: server.fetch,
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    scrollY: 0,
    scrollTo(_x, y) { window.scrollY = y; },
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    performance: { mark() {} },
    console: { log() {}, debug() {}, info() {}, warn() {}, error: (...args) => window.__consoleErrors.push(args.map(String).join(' ')) },
    AbortController, AbortSignal, DOMException, Response, ReadableStream, TextDecoder, TextEncoder, URL, URLSearchParams,
    Promise, JSON, Map, Set, WeakMap, Error, TypeError, Math, Number, String, Object, Array, Date, structuredClone,
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    __views: {
      home: { mounts: 0, disposes: 0, draws: [], throwAfterAdding: false },
      transcripts: { mounts: 0, disposes: 0, updates: [], draws: [], pops: 0 },
    },
    __scriptErrors: [],
    __consoleErrors: [],
    solShellData: {
      apps: ['home', 'transcripts', 'entities', 'sources'].map((name) => ({
        name, label: name, workspace_url: `/app/${name}/workspace`,
      })),
    },
    SurfaceState: { error: () => '<div class="surface-state surface-state--error" role="alert"><button class="surface-state-retry">try again</button></div>' },
  });
  vm.createContext(window);
  vm.runInContext(headScript(), window, { filename: 'shell.html' });
  for (const name of ['api.js', 'store.js', 'views.js', 'mount-workspace.js', 'router.js']) {
    vm.runInContext(staticSource(name), window, { filename: name });
  }
  const view = (name) => window.SolViews.current()?.name === name;
  const page = {
    clock, server, window, document, history, record, writes, observers, location,
    eventCount: () => [...eventListeners.values()].reduce((sum, set) => sum + set.size, 0),
    connectionCount: () => connectionHandlers.size,
    setConnected(next) {
      connected = next;
      connectionHandlers.forEach((fn) => fn({ connected }));
    },
    setBuildChanged(next) { built = next; },
    view,
    main: () => document.getElementById('main-content'),
    link: (href) => document.root.walk().find((node) => node.tagName === 'A' && node.getAttribute('href') === href),
    // A click the router doesn't take is a page load, as in a browser.
    click(href) {
      const event = page.link(href).click();
      if (!event.defaultPrevented) record.assigned.push(new URL(href, ORIGIN).pathname);
      return event;
    },
    // Answers every view read and fragment as a plain success.
    answerAll({ pulse = { n: 1 } } = {}) {
      server.answers.set('/app/home/api/pulse', server.json(pulse));
      for (const [url, body] of Object.entries(FRAGMENTS)) server.answers.set(url, server.html(body));
      server.answers.set('/api/shell', server.json({ app_names: [] }));
      server.answers.set('/api/streams/labels', server.json({}));
    },
    async boot(name = 'home') {
      const app = window.solShellData.apps.find((candidate) => candidate.name === name);
      try {
        await window.mountWorkspaceFragment(app.workspace_url, { appName: name });
      } finally {
        window.solRouter.start(name);
      }
      await flush();
    },
  };
  return page;
}

function viewTransitionStub(record) {
  return { skipTransition() { record.skipped += 1; } };
}

function dayAnswer(day, extra = {}) {
  return (call) => call.respond(200, JSON.stringify({ day, ...extra }), { 'content-type': 'application/json' });
}

function answerDays(page) {
  for (const day of ['20260101', '20260102', '20260103']) {
    page.server.answers.set(`/app/transcripts/api/day/${day}`, dayAnswer(day));
  }
}

// What the page holds outside a view's own markup.
function footprint(page) {
  return {
    documentListeners: page.document.listenerCount(),
    windowListeners: page.window.listenerCount(),
    timers: page.clock.count(),
    events: page.eventCount(),
    connections: page.connectionCount(),
    observers: page.observers.size,
    bodyClass: page.document.body.getAttribute('class') || '',
    homeGlobal: typeof page.window.homeGlobal,
  };
}

let cases = 0;
const queue = [];
function asyncCase(name, fn) {
  queue.push(async () => {
    try {
      await fn();
      cases += 1;
    } catch (error) {
      error.message = name + ': ' + error.message;
      throw error;
    }
  });
}

// ---- the contract

asyncCase('contract: dispose leaves no listener, timer, subscription, observer, global or body class behind', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  const before = footprint(page);
  await page.boot('home');
  const mounted = footprint(page);
  assert.ok(mounted.documentListeners > before.documentListeners && mounted.timers > before.timers, 'home added to the page');
  assert.strictEqual(mounted.bodyClass, 'home-open');
  page.window.SolViews.dispose();
  // The shell's own short waits (the still-working watch, the date code's
  // idle fetch) run out first.
  await page.clock.advance(15000);
  const after = footprint(page);
  // The router listens on the document and window for the page's life; it
  // started with home and doesn't grow.
  const router = { documentListeners: 3, windowListeners: 1, connections: 1 };
  assert.deepStrictEqual(after, {
    ...before,
    documentListeners: before.documentListeners + router.documentListeners,
    windowListeners: before.windowListeners + router.windowListeners,
    connections: before.connections + router.connections,
  });
  assert.strictEqual(page.window.__views.home.disposes, 1, 'the view\'s own dispose ran');
});

asyncCase('contract: a mount that throws part-way takes back what it added, and try again mounts it cleanly', async () => {
  const page = makePage();
  page.answerAll();
  page.window.__views.home.throwAfterAdding = true;
  const before = footprint(page);
  await page.boot('home').catch(() => {});
  await page.clock.advance(15000);
  const failed = footprint(page);
  const router = { documentListeners: 3, windowListeners: 1, connections: 1 };
  assert.deepStrictEqual(failed, {
    ...before,
    documentListeners: before.documentListeners + router.documentListeners,
    windowListeners: before.windowListeners + router.windowListeners,
    connections: before.connections + router.connections,
  }, 'nothing the failed mount added is left');
  assert.ok(page.main().querySelector('.surface-state-retry'), 'the view shows its error state');
  page.window.__views.home.throwAfterAdding = false;
  page.main().querySelector('.surface-state-retry').click();
  await page.clock.advance(100);
  assert.deepStrictEqual(page.record.reloads, [], 'no whole page load');
  assert.strictEqual(page.window.__views.home.mounts, 2);
  assert.ok(page.view('home'));
  assert.strictEqual(footprint(page).bodyClass, 'home-open');
});

asyncCase('contract: a transcripts day step is an update, not a remount', async () => {
  const page = makePage({ pathname: '/app/transcripts/20260101' });
  page.answerAll();
  answerDays(page);
  await page.boot('transcripts');
  assert.deepStrictEqual(page.window.__views.transcripts.draws, ['20260101']);
  assert.strictEqual(page.window.solRouter.navigate('/app/transcripts/20260102'), true);
  await page.clock.advance(400);
  const views = page.window.__views.transcripts;
  assert.strictEqual(views.mounts, 1, 'mounted once');
  assert.deepStrictEqual(views.updates, ['20260102']);
  assert.deepStrictEqual(views.draws, ['20260101', '20260102']);
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260102');
  assert.strictEqual(page.server.of('/app/transcripts/workspace').length, 1, 'the fragment is not asked for again');
});

// ---- the router

asyncCase('router: a link between pilot views swaps <main> and keeps the rail, status mark and stream', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260102' }));
  await page.boot('home');
  const rail = page.document.getElementById('app-rail');
  const status = page.document.getElementById('status-instrument');
  const events = page.window.appEvents;
  const event = page.click('/app/transcripts/');
  assert.ok(event.defaultPrevented, 'the router took the click');
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260102', 'the undated root takes its day');
  assert.ok(page.main().querySelector('.tr'), 'the new view is in <main>');
  assert.ok(!page.main().querySelector('.home'), 'the old one is gone');
  assert.strictEqual(page.document.getElementById('app-rail'), rail, 'the rail stays');
  assert.strictEqual(page.document.getElementById('status-instrument'), status, 'the status mark stays');
  assert.strictEqual(page.window.appEvents, events, 'the live stream stays');
  assert.strictEqual(page.connectionCount(), 1, 'the page keeps its own connection watch; home took its own with it');
  assert.deepStrictEqual(page.record.assigned, [], 'no page load');
  assert.deepStrictEqual(page.record.reloads, []);
});

asyncCase('router: to or from another view is a page load, sources included', async () => {
  const page = makePage();
  page.answerAll();
  await page.boot('home');
  for (const href of ['/app/entities/', '/app/sources/']) {
    const event = page.click(href);
    assert.ok(!event.defaultPrevented, `${href} is the browser's`);
  }
  assert.deepStrictEqual(page.record.assigned, ['/app/entities/', '/app/sources/']);
  assert.ok(page.view('home'), 'home stays mounted');

  const other = makePage({ pathname: '/app/entities/' });
  other.answerAll();
  await other.boot('entities');
  assert.strictEqual(other.window.solRouter.isActive(), false, 'a page opened on another view never turns the router on');
  const event = other.click('/app/home/');
  assert.ok(!event.defaultPrevented);
  assert.deepStrictEqual(other.record.assigned, ['/app/home/']);
});

asyncCase('router: a day step on another dated view stays a page load', async () => {
  const page = makePage({ pathname: '/app/entities/' });
  page.answerAll();
  await page.boot('entities');
  assert.strictEqual(page.window.solRouter.navigate('/app/entities/20260102'), false);
  const pilot = makePage();
  pilot.answerAll();
  await pilot.boot('home');
  assert.strictEqual(pilot.window.solRouter.navigate('/app/entities/20260102'), false, 'leaving a pilot view for another view loads the page');
});

asyncCase('router: back and forward between pilot entries restore view and scroll', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  await page.boot('home');
  page.window.scrollY = 300;
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.window.scrollY, 0, 'a new view starts at the top');
  page.window.scrollY = 120;
  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.strictEqual(page.window.scrollY, 300, 'home is where it was left');
  await page.history.forward();
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.window.scrollY, 120);
  assert.deepStrictEqual(page.record.assigned, []);
});

asyncCase('router: back across a segment entry and a day update lands each step, then home', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  const views = page.window.__views.transcripts;
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260101');
  page.window.solRouter.navigate('/app/transcripts/20260102');
  await page.clock.advance(400);
  page.window.__openSegment('120000_300');
  assert.strictEqual(page.location.search + page.location.hash, '?stream=a#120000_300/transcript');
  // back: the segment entry is the view's own; transcripts restores it.
  await page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260102');
  assert.strictEqual(views.pops, 1, 'the view got its own entry');
  assert.strictEqual(views.mounts, 1, 'no remount for a segment entry');
  // back: day A, another path, the router's: an update.
  await page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260101');
  assert.deepStrictEqual(views.updates, ['20260102', '20260101']);
  assert.strictEqual(views.mounts, 1);
  // back: home.
  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('home'), 'back from transcripts reaches home');
  assert.strictEqual(page.window.__views.home.mounts, 2);
  // back once more leaves the page for the entry before it.
  await page.history.back();
  assert.deepStrictEqual(page.record.left, ['http://elsewhere.test/'], 'the browser takes the entry before the page');
  assert.deepStrictEqual(page.record.assigned, []);
});

asyncCase('router: a page that learned a new build loads the destination as a new page', async () => {
  const page = makePage();
  page.answerAll();
  await page.boot('home');
  page.setBuildChanged(true);
  const event = page.click('/app/transcripts/');
  assert.ok(event.defaultPrevented);
  assert.deepStrictEqual(page.record.assigned, ['/app/transcripts/']);
  assert.ok(page.view('home'), 'no new code mounted into this page');
  assert.ok(page.writes.includes('solstone.serverBuildReloadAt'));
  assert.ok(page.record.skipped >= 1, 'the outgoing cross-document transition is skipped');
  // A view this page's code doesn't know is a page load too.
  const unknown = makePage();
  unknown.answerAll();
  await unknown.boot('home');
  unknown.window.solShellData.apps = unknown.window.solShellData.apps.filter((app) => app.name !== 'transcripts');
  unknown.click('/app/transcripts/');
  assert.deepStrictEqual(unknown.record.assigned, ['/app/transcripts/']);
});

asyncCase('router: a fragment naming code from another build loads the whole page, never runs it here', async () => {
  const page = makePage();
  page.answerAll();
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  page.server.answers.set('/app/transcripts/workspace', page.server.html('<div class="tr"><h2>day</h2></div><script src="/app/transcripts/static/0123456789abcdef/transcripts.js"></script>'));
  await page.boot('home');
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  assert.deepStrictEqual(page.record.reloads, ['/app/transcripts/20260101'], 'one whole page load of the destination');
  assert.strictEqual(page.window.__views.transcripts.mounts, 0, 'the other build\'s code never ran');
  assert.ok(page.writes.includes('solstone.serverBuildReloadAt'));
});

asyncCase('router: a quick second move ends the first one\'s reads, and the second view adopts its own', async () => {
  const page = makePage();
  page.answerAll();
  page.server.answers.delete('/app/home/api/pulse');
  const firstPulse = () => page.server.of('/app/home/api/pulse');
  // The first pulse answers at once; later ones wait for the test.
  page.server.answers.set('/app/home/api/pulse', (call) => { if (firstPulse().length === 1) page.server.json({ n: 1 })(call); });
  await page.boot('home');
  page.click('/app/transcripts/');
  await flush();
  const dayRead = () => page.server.of('/app/transcripts/api/day/20260101');
  page.click('/app/home/');
  await flush();
  const transcriptsEarly = page.server.calls.filter((call) => call.url.startsWith('/app/transcripts/api/'));
  assert.ok(transcriptsEarly.length > 0, 'the first move started its reads');
  assert.ok(transcriptsEarly.every((call) => call.aborted), 'the first move\'s reads were aborted');
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  assert.strictEqual(page.window.__views.transcripts.mounts, 0, 'transcripts never mounted');
  const pulses = page.server.pending('/app/home/api/pulse');
  assert.strictEqual(pulses.length, 1, 'one pulse read for the second move');
  pulses[0].respond(200, JSON.stringify({ n: 2 }), { 'content-type': 'application/json' });
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.deepStrictEqual(page.window.__views.home.draws, [{ n: 1 }, { n: 2 }]);
  assert.strictEqual(page.server.of('/app/home/api/pulse').length, 2, 'home adopted the read its move started');
  assert.strictEqual(dayRead().length <= 1, true);
});

// ---- the store

asyncCase('store: one read in flight per key; a refresh asks of its own', async () => {
  const page = makePage({ pathname: '/app/entities/' });
  const abort = page.window.SolStore.createAbort();
  const scope = page.window.SolStore.scope(abort.signal);
  page.window.apiJson = (url, opts) => page.window.fetch(url, opts).then((r) => r.json());
  const a = scope.read('/app/x/api/state');
  const b = scope.read('/app/x/api/state');
  await flush();
  assert.strictEqual(a, b);
  assert.strictEqual(page.server.of('/app/x/api/state').length, 1);
  scope.read('/app/x/api/state', { fresh: true });
  await flush();
  assert.strictEqual(page.server.of('/app/x/api/state').length, 2);
});

asyncCase('store: an ended mount\'s reads are aborted and never answer', async () => {
  const page = makePage({ pathname: '/app/entities/' });
  const abort = page.window.SolStore.createAbort();
  const scope = page.window.SolStore.scope(abort.signal);
  page.window.apiJson = (url, opts) => page.window.fetch(url, opts).then((r) => r.json(), (e) => { throw e; });
  let answered = false;
  scope.read('/app/x/api/state').then(() => { answered = true; }, () => { answered = true; });
  await flush();
  abort.abort();
  page.server.of('/app/x/api/state')[0].respond(200, '{}', { 'content-type': 'application/json' });
  await flush();
  assert.strictEqual(page.server.of('/app/x/api/state')[0].aborted, true);
  assert.strictEqual(answered, false, 'never drawn, never an error');
  scope.read('/app/x/api/state');
  await flush();
  assert.strictEqual(page.server.of('/app/x/api/state').length, 1, 'nothing more is asked after the end');
});

asyncCase('store: moving between views writes nothing to web storage', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  for (let i = 0; i < 3; i += 1) {
    page.click('/app/transcripts/');
    await page.clock.advance(400);
    page.window.solRouter.navigate('/app/transcripts/20260102');
    await page.clock.advance(400);
    page.click('/app/home/');
    await page.clock.advance(400);
  }
  assert.deepStrictEqual(page.writes, []);
});

asyncCase('store: a view whose first read failed reads again on the stream\'s connect, once per connect', async () => {
  const page = makePage({ pathname: '/app/transcripts/20260101' });
  page.answerAll();
  let down = true;
  page.server.answers.set('/app/transcripts/api/day/20260101', (call) => (down ? call.respond(500, '{}', { 'content-type': 'application/json' }) : dayAnswer('20260101')(call)));
  await page.boot('transcripts');
  await page.clock.advance(5000);
  assert.strictEqual(page.window.SolViews.current().state, 'error');
  const reads = () => page.server.of('/app/transcripts/api/day/20260101').length;
  const first = reads();
  page.setConnected(true);
  await page.clock.advance(5000);
  assert.strictEqual(reads(), first + 1, 'one read on the connect');
  assert.strictEqual(page.window.SolViews.current().state, 'error', 'still down');
  page.setConnected(true);
  await page.clock.advance(5000);
  assert.strictEqual(reads(), first + 1, 'the same connect never reads twice');
  down = false;
  page.setConnected(false);
  page.setConnected(true);
  await page.clock.advance(5000);
  assert.strictEqual(reads(), first + 2, 'the next connect reads again');
  assert.strictEqual(page.window.SolViews.current().state, 'content');
  assert.deepStrictEqual(page.window.__views.transcripts.draws, ['20260101']);
  // After a day step whose read fails, the next connect still reads again.
  page.server.answers.set('/app/transcripts/api/day/20260102', (call) => call.respond(500, '{}', { 'content-type': 'application/json' }));
  page.window.solRouter.navigate('/app/transcripts/20260102');
  await page.clock.advance(5000);
  assert.strictEqual(page.window.SolViews.current().state, 'error');
  const stepped = page.server.of('/app/transcripts/api/day/20260102').length;
  page.server.answers.set('/app/transcripts/api/day/20260102', dayAnswer('20260102'));
  page.setConnected(false);
  page.setConnected(true);
  await page.clock.advance(5000);
  assert.ok(page.server.of('/app/transcripts/api/day/20260102').length > stepped, 'a failed day step reads again on the connect');
});

// ---- what a screen reader hears

asyncCase('accessibility: focus moves to the new view\'s heading and the view is announced by name', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  const heading = page.main().querySelector('h2');
  assert.strictEqual(page.document.activeElement, heading, 'focus is on the heading');
  assert.strictEqual(heading.getAttribute('tabindex'), '-1');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'transcripts');
  // A day step keeps focus where it was.
  const step = page.link('/app/home/');
  step.focus();
  page.window.solRouter.navigate('/app/transcripts/20260102');
  await page.clock.advance(400);
  assert.strictEqual(page.document.activeElement, step);
});

asyncCase('accessibility: a view still loading at ten seconds is announced as still working, once', async () => {
  const page = makePage();
  page.answerAll();
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  const announcer = page.document.getElementById('view-announcer');
  page.click('/app/transcripts/');
  // The swap waits 300 ms for the read, and the name follows 300 ms later.
  await page.clock.advance(1000);
  assert.strictEqual(announcer.textContent, 'transcripts');
  await page.clock.advance(8500);
  assert.strictEqual(announcer.textContent, 'transcripts');
  await page.clock.advance(1000);
  assert.strictEqual(announcer.textContent, 'still working…');
  announcer.textContent = '';
  await page.clock.advance(20000);
  assert.strictEqual(announcer.textContent, '', 'said once');
  // A view ready before ten seconds is never announced as still working.
  page.server.pending('/app/transcripts/api/day/20260101').forEach((call) => dayAnswer('20260101')(call));
  page.click('/app/home/');
  await page.clock.advance(15000);
  assert.strictEqual(announcer.textContent, 'home');
});

// ---- transitions and prefetch

asyncCase('transition: the swap waits for the next view\'s reads, at most 300 ms', async () => {
  const page = makePage({ transitions: true });
  page.answerAll();
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  page.click('/app/transcripts/');
  await page.clock.advance(200);
  assert.ok(page.main().querySelector('.home'), 'the old view stays while the read is out');
  await page.clock.advance(150);
  assert.ok(page.main().querySelector('.tr'), 'swapped at 300 ms with the read still out');
  assert.strictEqual(page.record.transitions, 1);
  // A read that answers sooner swaps sooner.
  answerDays(page);
  page.click('/app/home/');
  await page.clock.advance(50);
  assert.ok(page.main().querySelector('.home'));
});

asyncCase('prefetch: pressing or hovering on a rail entry fetches its code, never its data', async () => {
  const page = makePage();
  page.answerAll();
  page.server.answers.set('/t/transcripts.js', (call) => call.respond(200, ''));
  page.server.answers.set('/t/date-nav.css', (call) => call.respond(200, ''));
  await page.boot('home');
  const before = page.server.calls.length;
  const rail = page.link('/app/transcripts/');
  page.document.dispatchEvent({ type: 'pointerover', target: rail });
  await page.clock.advance(50);
  assert.strictEqual(page.server.calls.length, before, 'a pass over the rail asks for nothing');
  await page.clock.advance(100);
  const asked = page.server.calls.slice(before).map((call) => call.url);
  assert.deepStrictEqual(asked.sort(), ['/app/transcripts/workspace', '/t/date-nav.css', '/t/transcripts.js']);
  page.document.dispatchEvent({ type: 'pointerdown', target: rail });
  await page.clock.advance(150);
  assert.strictEqual(page.server.calls.length, before + 3, 'once per view');
  page.document.dispatchEvent({ type: 'pointerdown', target: page.link('/app/entities/') });
  await page.clock.advance(150);
  assert.strictEqual(page.server.calls.length, before + 3, 'another view\'s code is its own page\'s');
  assert.ok(!page.server.calls.some((call) => call.url.includes('/api/') && call.url !== '/api/shell' && call.url !== '/api/streams/labels' && call.url !== '/app/home/api/pulse'));
  assert.ok(page.window.solRouter.held().fragments <= 2, 'one fragment per pilot view at most');
});

asyncCase('leaks: a hundred switches and fifty day steps leave the page as the first switch did', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  const cycle = async () => {
    page.click('/app/transcripts/');
    await page.clock.advance(400);
    page.window.solRouter.navigate('/app/transcripts/20260102');
    await page.clock.advance(400);
    page.click('/app/home/');
    await page.clock.advance(400);
  };
  await cycle();
  await page.clock.advance(15000);
  const first = { ...footprint(page), router: page.window.solRouter.held().fragments, head: page.document.head.children.length };
  for (let i = 0; i < 49; i += 1) await cycle();
  await page.clock.advance(15000);
  const last = { ...footprint(page), router: page.window.solRouter.held().fragments, head: page.document.head.children.length };
  assert.deepStrictEqual(last, first);
  assert.ok(page.window.solRouter.held().scrolls <= 50, 'the scroll memory is bounded');
  assert.deepStrictEqual(page.window.__scriptErrors, []);
});

(async () => {
  for (const run of queue) await run();
  console.log('DOM CASES: ' + cases + ' passed');
})().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
