// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Support on the view contract, under plain Node: the real fragment and
// script, mounted through the dashboard's real view contract and read store,
// in a stand-in page. The report flow is the shell's; a case stands it in and
// counts what the button asks of it.
//
// Every compare of two elements is `assert.ok(a === b, …)`: a failing case
// reports a line, never a diff of the page.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = path.join(__dirname, '..');
const shellDir = path.join(crateDir, '../solstone-core-convey-shell/assets/static');
const VOID_TAGS = new Set(['br', 'hr', 'img', 'input', 'link', 'meta']);

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

class Target {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, fn) {
    const list = this.listeners.get(type) || [];
    if (!list.includes(fn)) list.push(fn);
    this.listeners.set(type, list);
  }
  removeEventListener(type, fn) {
    this.listeners.set(type, (this.listeners.get(type) || []).filter((f) => f !== fn));
  }
  listenerCount() {
    let n = 0;
    this.listeners.forEach((list) => { n += list.length; });
    return n;
  }
  fire(event) {
    for (const fn of [...(this.listeners.get(event.type) || [])]) fn.call(this, event);
  }
}

class Element extends Target {
  constructor(doc, tag) {
    super();
    this.ownerDocument = doc;
    this.nodeType = 1;
    this.tagName = tag.toUpperCase();
    this.attrs = new Map();
    this.childNodes = [];
    this.parentNode = null;
  }
  get children() { return this.childNodes.filter((n) => n.nodeType === 1); }
  setAttribute(k, v) { this.attrs.set(k, String(v)); }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  hasAttribute(k) { return this.attrs.has(k); }
  get textContent() { return this.childNodes.map((n) => (n.nodeType === 3 ? n.data : n.textContent)).join(''); }
  appendChild(node) { node.parentNode = this; this.childNodes.push(node); return node; }
  replaceChildren() { this.childNodes.forEach((n) => { n.parentNode = null; }); this.childNodes = []; }
  querySelectorAll(tag) {
    const out = [];
    const walk = (el) => el.children.forEach((c) => { if (c.tagName === tag.toUpperCase()) out.push(c); walk(c); });
    walk(this);
    return out;
  }
  find(id) {
    for (const child of this.children) {
      if (child.getAttribute('id') === id) return child;
      const deeper = child.find(id);
      if (deeper) return deeper;
    }
    return null;
  }
  click() {
    const event = { type: 'click', target: this, button: 0 };
    for (let at = this; at; at = at.parentNode) at.fire(event);
  }
}

// Enough of a parser for the fragment's own markup.
function parseInto(doc, parent, html) {
  const stack = [parent];
  const re = /<!--[\s\S]*?-->|<(style|script)\b[^>]*>[\s\S]*?<\/\1>|<\/([a-zA-Z0-9]+)\s*>|<([a-zA-Z][a-zA-Z0-9-]*)((?:\s+[^\s=>]+(?:=(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*\/?>|([^<]+)/g;
  let m;
  while ((m = re.exec(html))) {
    const top = stack[stack.length - 1];
    if (m[0].startsWith('<!--') || m[1]) continue;
    if (m[2]) {
      while (stack.length > 1 && stack.pop().tagName !== m[2].toUpperCase()) {}
      continue;
    }
    if (m[3]) {
      const el = new Element(doc, m[3]);
      const attrRe = /([^\s=>]+)(?:=("[^"]*"|'[^']*'|[^\s>]+))?/g;
      let a;
      while ((a = attrRe.exec(m[4] || ''))) {
        let v = a[2] === undefined ? '' : a[2];
        if (/^["']/.test(v)) v = v.slice(1, -1);
        el.setAttribute(a[1], v);
      }
      top.appendChild(el);
      if (!VOID_TAGS.has(m[3].toLowerCase())) stack.push(el);
      continue;
    }
    if (m[5] && m[5].trim()) top.appendChild({ nodeType: 3, data: m[5] });
  }
}

function makePage(env) {
  const doc = new Target();
  doc.documentElement = new Element(doc, 'html');
  doc.body = new Element(doc, 'body');
  doc.documentElement.appendChild(doc.body);
  doc.title = 'support - journal';
  doc.getElementById = (id) => doc.documentElement.find(id);
  const window = new Target();
  const errors = [];
  window.document = doc;
  window.location = { href: 'http://journal.test/app/support/', pathname: '/app/support/', search: '', hash: '' };
  window.logError = (error, context) => { errors.push({ error, context }); };
  const main = new Element(doc, 'main');
  main.setAttribute('id', 'main-content');
  doc.body.appendChild(main);
  const context = vm.createContext({
    window, document: doc, location: window.location,
    URL, AbortController, AbortSignal, DOMException, Map, Set, Promise, WeakMap,
    console, Math, Number, String, Object, Array, Boolean, Date, JSON, Error, TypeError, RegExp, Symbol,
    setTimeout, clearTimeout,
  });
  vm.runInContext(fs.readFileSync(path.join(shellDir, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shellDir, 'views.js'), 'utf8'), context);
  const page = {
    window, doc, main, errors,
    reports: [],
    windowKeys: () => Object.keys(window).sort(),
    // The shell's report flow, once its part of the shell has loaded.
    loadReportFlow() {
      window.convey = window.convey || {};
      window.convey.reportError = (context) => { page.reports.push(context); };
    },
    loadScript() { vm.runInContext(env.script, context, { filename: 'support.js' }); },
    // The router's place(): the fragment's markup, scripts and styles out.
    place() {
      main.replaceChildren();
      parseInto(doc, main, env.workspace);
    },
    async mount(name = 'support') {
      page.ready = [];
      page.view = await window.SolViews.mount(name, { reportReady: (s) => page.ready.push(s) });
      return page.view;
    },
    button: () => doc.getElementById('make-report'),
  };
  return page;
}

(async () => {
  const workspace = fs.readFileSync(path.join(crateDir, 'assets/workspace.html'), 'utf8');
  const script = fs.readFileSync(path.join(crateDir, 'assets/static/support.js'), 'utf8');
  const env = { workspace, script };

  await test('the fragment declares the contract and starts no read', async () => {
    assert.ok(workspace.includes('<!-- view-contract -->'), 'the view-contract marker');
    assert.ok(!workspace.includes('first-read:'), 'no first read');
  });

  await test('the script only registers: nothing listens, reports or is added to the page at load', async () => {
    const p = makePage(env);
    p.loadReportFlow();
    p.place();
    const keys = p.windowKeys();
    p.loadScript();
    assert.deepStrictEqual(p.windowKeys(), keys, 'no global at load');
    assert.strictEqual(p.button().listenerCount(), 0, 'no listener on the button at load');
    assert.strictEqual(p.window.listenerCount() + p.doc.listenerCount(), 0, 'no page listener at load');
    assert.strictEqual(p.window.SolViews.registered('support'), true, 'it registers');
    p.button().click();
    assert.strictEqual(p.reports.length, 0, 'the button does nothing before a mount');
  });

  await test('after an in-page mount the button opens the report flow, once a press', async () => {
    const p = makePage(env);
    p.loadReportFlow();
    p.loadScript();
    // Another contract view was on the page first.
    p.window.SolViews.register('home', { mount() {} });
    await p.mount('home');
    p.place();
    await p.mount();
    assert.deepStrictEqual(p.ready, ['content'], 'ready as content at mount');
    p.button().click();
    assert.strictEqual(p.reports.length, 1, 'one report flow');
    assert.strictEqual(p.reports[0].source, 'support-page', 'as the support page');
    assert.deepStrictEqual(Object.keys(p.reports[0]), ['source'], 'and nothing else');
    assert.strictEqual(p.errors.length, 0, 'no error');
  });

  await test('dispose takes the listener back, and a second mount in the same document listens once', async () => {
    const p = makePage(env);
    p.loadReportFlow();
    p.loadScript();
    p.place();
    const keys = p.windowKeys();
    await p.mount();
    const first = p.button();
    const view = p.view;
    p.window.SolViews.dispose();
    assert.deepStrictEqual(JSON.parse(JSON.stringify(view.ctx.held())), { listeners: 0, timeouts: 0, intervals: 0, observers: 0, undo: 0, bodyClasses: 0, reads: 0 });
    assert.strictEqual(first.listenerCount(), 0, 'the button keeps no listener');
    assert.deepStrictEqual(p.windowKeys(), keys, 'no global stays');
    p.place();
    await p.mount();
    assert.ok(p.button() !== first, 'the new mount has its own markup');
    p.button().click();
    assert.strictEqual(p.reports.length, 1, 'one press, one report flow');
  });

  await test('a press before the report flow has loaded does nothing and throws nothing; the next press after it loads opens it', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    await p.mount();
    p.button().click();
    assert.strictEqual(p.reports.length, 0, 'nothing opens');
    assert.strictEqual(p.errors.length, 0, 'no error');
    p.loadReportFlow();
    p.button().click();
    assert.strictEqual(p.reports.length, 1, 'it opens once the flow is there');
  });

  await test('a mount with no button throws, takes back what it added, and a second try mounts clean', async () => {
    const p = makePage(env);
    p.loadReportFlow();
    p.loadScript();
    let threw = false;
    try { await p.mount(); } catch (_) { threw = true; }
    assert.ok(threw, 'the mount throws');
    assert.strictEqual(p.window.SolViews.current(), null, 'nothing is mounted');
    p.place();
    await p.mount();
    p.button().click();
    assert.strictEqual(p.reports.length, 1, 'the second try works');
  });

  console.log('DOM CASES: ' + cases + ' passed');
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
