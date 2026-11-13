// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Sources on the view contract, under plain Node: the real fragment and
// script, mounted through the real view contract and read store, in a
// stand-in page with focus, a media query and reads the cases answer by hand.
// sources_dom.js runs these after its own cases.
//
// Every compare of two elements is `assert.ok(a === b, …)` with a tag in the
// message: a failing case reports a line, never a diff of the page.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const ORIGIN = 'http://journal.test';
const CLIENTS = '/app/network/api/clients';

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
    for (const entry of [...(this.listeners.get(event.type) || [])]) entry.fn.call(this, event);
  }
}

class TextNode {
  constructor(text) { this.nodeType = 3; this.data = String(text); this.parentNode = null; }
  get textContent() { return this.data; }
}

function simpleMatch(el, part) {
  const re = /([a-zA-Z][a-zA-Z0-9-]*)|#([\w-]+)|\.([\w-]+)|\[([\w-]+)(?:=["']?([^\]"']*)["']?)?\]/g;
  let m;
  let any = false;
  while ((m = re.exec(part))) {
    any = true;
    if (m[1] && el.tagName !== m[1].toUpperCase()) return false;
    if (m[2] && el.getAttribute('id') !== m[2]) return false;
    if (m[3] && !(el.getAttribute('class') || '').split(/\s+/).includes(m[3])) return false;
    if (m[4]) {
      if (!el.hasAttribute(m[4])) return false;
      if (m[5] !== undefined && el.getAttribute(m[4]) !== m[5]) return false;
    }
  }
  return any;
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
  }
  get children() { return this.childNodes.filter((n) => n.nodeType === 1); }
  get className() { return this.getAttribute('class') || ''; }
  set className(v) { this.setAttribute('class', v); }
  get id() { return this.getAttribute('id') || ''; }
  setAttribute(k, v) { this.attrs.set(k, String(v)); }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  hasAttribute(k) { return this.attrs.has(k); }
  removeAttribute(k) { this.attrs.delete(k); }
  get textContent() { return this.childNodes.map((n) => n.textContent).join(''); }
  set textContent(v) {
    [...this.childNodes].forEach((n) => this.removeChild(n));
    if (String(v ?? '')) this.appendChild(new TextNode(v));
  }
  get isConnected() {
    let at = this;
    while (at.parentNode) at = at.parentNode;
    return at === this.ownerDocument.documentElement;
  }
  contains(node) {
    for (let at = node; at; at = at.parentNode) if (at === this) return true;
    return false;
  }
  appendChild(node) {
    if (node.parentNode) node.parentNode.removeChild(node);
    node.parentNode = this;
    this.childNodes.push(node);
    return node;
  }
  removeChild(node) {
    const i = this.childNodes.indexOf(node);
    if (i >= 0) this.childNodes.splice(i, 1);
    node.parentNode = null;
    this.ownerDocument.dropFocusFrom(node);
    return node;
  }
  replaceChildren(...nodes) {
    [...this.childNodes].forEach((n) => this.removeChild(n));
    nodes.forEach((n) => this.appendChild(n));
  }
  querySelectorAll(selector) {
    const out = [];
    const walk = (el) => el.children.forEach((c) => {
      if (selector.split(',').some((one) => simpleMatch(c, one.trim()))) out.push(c);
      walk(c);
    });
    walk(this);
    return out;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  focusable() {
    return ['A', 'BUTTON'].includes(this.tagName) || this.hasAttribute('tabindex');
  }
  focus() {
    if (!this.isConnected || !this.focusable()) return;
    this.ownerDocument.activeElement = this;
  }
  click() {
    const event = { type: 'click', target: this, button: 0, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
    for (let at = this; at; at = at.parentNode) at.fire(event);
    return event;
  }
}

function makeDocument() {
  const doc = new Target();
  doc.title = 'sources - journal';
  doc.createElement = (tag) => new Element(doc, tag);
  doc.documentElement = new Element(doc, 'html');
  doc.body = new Element(doc, 'body');
  doc.documentElement.appendChild(doc.body);
  doc.activeElement = doc.body;
  doc.getElementById = (id) => doc.documentElement.querySelector('#' + id);
  doc.querySelector = (s) => doc.documentElement.querySelector(s);
  // Focus on a node the page takes out goes to the page, as a browser does.
  doc.dropFocusFrom = (node) => {
    for (let at = doc.activeElement; at; at = at.parentNode) {
      if (at === node) { doc.activeElement = doc.body; return; }
    }
  };
  return doc;
}

function abortError() {
  const error = new Error('The operation was aborted');
  error.name = 'AbortError';
  return error;
}

// ---- the journal the page reads

function device(label, minutesAgo) {
  return {
    display_label: label, owner_label: '', device_label: label, kind: 'cert',
    last_accepted_ingest_at: new Date(Date.now() - minutesAgo * 60000).toISOString(),
    source_delivery: { audio: { state: 'current', last_accepted_ingest_at: new Date(Date.now() - minutesAgo * 60000).toISOString() } },
    reported: { platform: 'linux', device_type: 'desktop' },
    capture_state: 'active', unassessed_reason: null,
  };
}

function makePage(env, { wide = true, withApi = false } = {}) {
  const doc = makeDocument();
  const window = new Target();
  const calls = [];
  const errors = [];
  const timers = new Set();
  let serial = 0;
  window.document = doc;
  window.location = { href: ORIGIN + '/app/sources/', pathname: '/app/sources/', search: '', hash: '', origin: ORIGIN };
  window.setTimeout = (fn, ms) => { serial += 1; timers.add(serial); return serial; };
  window.clearTimeout = (id) => { timers.delete(id); };
  window.setInterval = window.setTimeout;
  window.clearInterval = window.clearTimeout;
  const media = new Target();
  media.matches = !wide;
  media.media = '(max-width: 700px)';
  window.matchMedia = (query) => { media.query = query; return media; };
  window.logError = (error, context) => { errors.push({ error, context }); };
  // A read the case answers: `call.answer(status, body)`, or hand the
  // response over now and the body later with `call.respond(status)`.
  window.fetch = (url, init = {}) => new Promise((resolve, reject) => {
    const call = {
      url: String(url), method: String(init.method || 'GET').toUpperCase(), aborted: false, settled: false,
      respond(status) {
        let bodyDone;
        const body = new Promise((done) => { bodyDone = done; });
        call.body = (value) => bodyDone(value);
        call.settled = true;
        resolve({ ok: status === 200, status, redirected: false, headers: jsonHeaders, json: () => body });
      },
      answer(status, value) {
        if (call.settled) return;
        call.settled = true;
        resolve({ ok: status === 200, status, redirected: false, headers: jsonHeaders, json: async () => value });
      },
    };
    if (init.signal) {
      const onAbort = () => { call.aborted = true; if (!call.settled) { call.settled = true; reject(abortError()); } };
      if (init.signal.aborted) onAbort();
      else init.signal.addEventListener('abort', onAbort, { once: true });
    }
    calls.push(call);
  });

  const jsonHeaders = { get: (name) => (String(name).toLowerCase() === 'content-type' ? 'application/json' : null) };

  const shellMain = doc.createElement('main');
  shellMain.setAttribute('id', 'main-content');
  shellMain.setAttribute('tabindex', '-1');
  doc.body.appendChild(shellMain);

  const context = vm.createContext({
    window, document: doc, location: window.location,
    URL, AbortController, AbortSignal, DOMException, Map, Set, Promise, WeakMap,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error, TypeError, RegExp, Symbol,
    setTimeout: window.setTimeout, clearTimeout: window.clearTimeout,
    fetch: window.fetch,
  });
  context.globalThis = window;
  const shell = path.join(env.manifestDir, 'assets/static');
  // The real read bounds and early-read adoption, where a case asks for them.
  if (withApi) vm.runInContext(fs.readFileSync(path.join(shell, 'api.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'views.js'), 'utf8'), context);

  const page = {
    window, doc, media, calls, errors, timers, main: shellMain,
    windowKeys: () => Object.keys(window).sort(),
    // The router's place(): the fragment's markup, scripts taken out.
    place() {
      shellMain.replaceChildren();
      const root = doc.createElement('main');
      root.setAttribute('id', 'sources-root');
      root.setAttribute('class', 'sources-page');
      root.setAttribute('tabindex', '-1');
      shellMain.appendChild(root);
    },
    loadScript() {
      vm.runInContext(env.script, context, { filename: 'sources.js' });
    },
    async mount() {
      page.ready = [];
      page.view = await window.SolViews.mount('sources', { reportReady: (s) => page.ready.push(s) });
      await settle();
      return page.view;
    },
    root: () => doc.getElementById('sources-root'),
    reads: () => calls.filter((c) => c.url === CLIENTS && c.method === 'GET'),
    last: () => calls[calls.length - 1],
    leave() {
      window.SolViews.dispose();
      shellMain.replaceChildren();
      const home = doc.createElement('section');
      home.setAttribute('id', 'home');
      shellMain.appendChild(home);
    },
  };
  return page;
}

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

async function settle() {
  for (let i = 0; i < 8; i += 1) await flush();
}

async function boot(env, options) {
  const p = makePage(env, options);
  p.loadScript();
  p.place();
  await p.mount();
  return p;
}

function tagOf(node) {
  if (!node) return 'none';
  return node.tagName + (node.id ? '#' + node.id : '') + (node.className ? '.' + node.className.split(/\s+/).join('.') : '');
}

function focusIsSomewhere(p, label) {
  const active = p.doc.activeElement;
  assert.ok(active && active !== p.doc.body, `${label}: focus is never on the body (it is on ${tagOf(active)})`);
}

// Run from sources_dom.js: answers with the number of cases.
module.exports = async function sourcesContractCases(manifestDir) {
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/sources/workspace.html'), 'utf8');
  const script = fs.readFileSync(path.join(manifestDir, 'assets/sources/sources.js'), 'utf8');
  const env = { manifestDir, script, workspace };

  await test('the fragment declares the contract and its first read', async () => {
    assert.ok(workspace.includes('<!-- view-contract -->'), 'the view-contract marker');
    assert.ok(workspace.includes(`<!-- first-read: /app/sources/ -> ${CLIENTS} -->`), 'the first read');
  });

  await test('the script only registers: nothing reads, draws, listens or times at load', async () => {
    const p = makePage(env);
    p.place();
    const keys = p.windowKeys();
    p.loadScript();
    assert.strictEqual(p.calls.length, 0, 'no read at load');
    assert.deepStrictEqual(p.windowKeys(), keys, 'no global at load');
    assert.strictEqual(p.media.listenerCount(), 0, 'no media listener at load');
    assert.strictEqual(p.window.listenerCount() + p.doc.listenerCount(), 0, 'no listener at load');
    assert.strictEqual(p.timers.size, 0, 'no timer at load');
    assert.strictEqual(p.root().childNodes.length, 0, 'nothing is drawn');
    assert.strictEqual(p.window.SolViews.registered('sources'), true, 'it registers');
  });

  await test('a mount reads the clients once through the store, draws them and reports content', async () => {
    const p = await boot(env);
    assert.strictEqual(p.reads().length, 1, 'one read');
    assert.ok(p.reads()[0].url === CLIENTS, 'of the clients, by its absolute address');
    assert.ok(p.media.query === '(max-width: 700px)', 'the layout query');
    assert.strictEqual(p.media.listenerCount(), 1, 'one media listener');
    p.reads()[0].answer(200, { clients: [device('studio', 5)] });
    await settle();
    assert.ok(p.root().querySelector('table') !== null, 'the wide table is drawn');
    assert.deepStrictEqual(p.ready, ['content'], 'ready as content');
    assert.strictEqual(p.errors.length, 0, 'no error');
  });

  for (const [label, status, body, state] of [
    ['no devices', 200, { clients: [] }, 'empty'],
    ['a failed read', 503, { reason_code: 'unavailable' }, 'error'],
    ['an unreadable device', 200, { clients: [Object.assign(device('studio', 5), { unassessed_reason: 'activity_unreadable' })] }, 'error'],
  ]) {
    await test(`${label} draws and reports ${state}`, async () => {
      const p = await boot(env);
      p.reads()[0].answer(status, body);
      await settle();
      assert.deepStrictEqual(p.ready, [state]);
      if (state === 'empty') assert.ok(p.root().querySelector('.sources-empty') !== null, 'the empty state');
      if (state === 'error') assert.ok(p.root().querySelector('.sources-retry') !== null, 'the error state');
    });
  }

  await test('the phone layout draws cards, and a layout change redraws in place', async () => {
    const p = await boot(env, { wide: false });
    p.reads()[0].answer(200, { clients: [device('studio', 5), device('phone', 8)] });
    await settle();
    assert.strictEqual(p.root().querySelectorAll('article').length, 2, 'one card each');
    p.media.matches = false;
    p.media.fire({ type: 'change' });
    assert.ok(p.root().querySelector('table') !== null, 'the wide table after the change');
    assert.strictEqual(p.reads().length, 1, 'a redraw reads nothing');
  });

  await test('dispose leaves no listener, timer, read or global, and a second mount in the same document works', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    const keys = p.windowKeys();
    await p.mount();
    p.reads()[0].answer(200, { clients: [device('studio', 5)] });
    await settle();
    const view = p.view;
    p.leave();
    await settle();
    assert.deepStrictEqual(JSON.parse(JSON.stringify(view.ctx.held())), { listeners: 0, timeouts: 0, intervals: 0, observers: 0, undo: 0, bodyClasses: 0, reads: 0 });
    assert.strictEqual(p.media.listenerCount(), 0, 'the media listener is gone');
    assert.strictEqual(p.window.listenerCount() + p.doc.listenerCount(), 0, 'no listener stays');
    assert.strictEqual(p.timers.size, 0, 'no timer stays');
    assert.deepStrictEqual(p.windowKeys(), keys, 'no global stays');
    p.media.fire({ type: 'change' });
    assert.ok(p.doc.getElementById('home') !== null, 'a layout change after leaving draws nothing');
    // Again, in the same document.
    p.place();
    await p.mount();
    assert.strictEqual(p.reads().length, 2, 'the second mount reads its own');
    p.reads()[1].answer(200, { clients: [device('studio', 5)] });
    await settle();
    assert.ok(p.root().querySelector('table') !== null, 'and draws');
    assert.strictEqual(p.media.listenerCount(), 1, 'one media listener, not two');
    assert.deepStrictEqual(p.ready, ['content']);
  });

  await test('leaving while the read is out aborts it, and its late answer draws nothing', async () => {
    const p = await boot(env);
    const call = p.reads()[0];
    const root = p.root();
    p.leave();
    await settle();
    assert.strictEqual(call.aborted, true, 'the read is aborted');
    call.settled = false;
    call.answer(200, { clients: [device('studio', 5)] });
    await settle();
    assert.strictEqual(root.childNodes.length, 0, 'the old root draws nothing');
    assert.ok(p.doc.getElementById('home') !== null, 'the page stays the next view');
    assert.deepStrictEqual(p.ready, [], 'no readiness');
    assert.strictEqual(p.errors.length, 0, 'no error');
  });

  await test('leaving after the answer and before its body draws nothing', async () => {
    const p = await boot(env);
    const call = p.reads()[0];
    const root = p.root();
    call.respond(200);
    await settle();
    p.leave();
    await settle();
    call.body({ clients: [device('studio', 5)] });
    await settle();
    assert.strictEqual(root.childNodes.length, 0, 'the old root draws nothing');
    assert.deepStrictEqual(p.ready, [], 'no readiness');
  });

  await test('a mount that throws part-way takes back what it added, and a second try mounts clean', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    const good = p.window.matchMedia;
    p.window.matchMedia = () => {
      const list = good('(max-width: 700px)');
      list.addEventListener = () => { throw new Error('no listener here'); };
      return list;
    };
    let threw = false;
    try { await p.mount(); } catch (_) { threw = true; }
    assert.ok(threw, 'the mount throws');
    assert.strictEqual(p.window.SolViews.current(), null, 'nothing is mounted');
    assert.strictEqual(p.calls.length, 0, 'it read nothing');
    p.media.addEventListener = Target.prototype.addEventListener;
    p.window.matchMedia = good;
    await p.mount();
    assert.strictEqual(p.reads().length, 1, 'the second try reads');
    assert.strictEqual(p.media.listenerCount(), 1, 'and listens once');
  });

  await test('"try again" makes a read of its own: never the early read that answered, nor one from an earlier arrival', async () => {
    const p = makePage(env, { withApi: true });
    const href = ORIGIN + CLIENTS;
    const unreadable = { clients: [Object.assign(device('studio', 5), { unassessed_reason: 'activity_unreadable' })] };
    const earlyResponse = { ok: true, status: 200, redirected: false, headers: { get: () => 'application/json' }, json: async () => unreadable };
    p.window.__solEarly = { view: 'sources', claimed: true, stats: {}, reads: { [href]: { response: Promise.resolve(earlyResponse), controller: null, startedAt: Date.now(), expect: 'json' } } };
    p.loadScript();
    p.place();
    await p.mount();
    assert.strictEqual(p.calls.length, 0, 'the first load takes the early read and asks nothing');
    assert.strictEqual(p.window.__solEarly.stats.adopted, 1, 'adopted once');
    assert.deepStrictEqual(Object.keys(p.window.__solEarly.reads), [], 'the early read is taken');
    assert.deepStrictEqual(p.ready, ['error'], 'it drew the unreadable device as the error state');
    p.root().querySelector('.sources-retry').click();
    assert.strictEqual(p.reads().length, 1, '"try again" asks the journal');
    p.reads()[0].answer(200, { clients: [device('studio', 5)] });
    await settle();
    assert.ok(p.root().querySelector('table') !== null, 'and draws its own answer');
    // An arrival after that one: no early read is left to take.
    p.leave();
    await settle();
    p.place();
    await p.mount();
    assert.strictEqual(p.reads().length, 2, 'the next arrival reads its own');
    p.reads()[1].answer(503, {});
    await settle();
    p.root().querySelector('.sources-retry').click();
    assert.strictEqual(p.reads().length, 3, 'and its "try again" reads again');
    assert.ok(p.reads()[2] !== p.reads()[1], 'a new request, not the answered one');
  });

  return cases;
};
