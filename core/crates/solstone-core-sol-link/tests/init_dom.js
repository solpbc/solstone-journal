// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const nodeAssert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

let glyphs;
let executedCases = 0;
let passedCases = 0;
function recordCase(assertion) {
  executedCases += 1;
  const result = assertion();
  passedCases += 1;
  return result;
}
const assert = new Proxy(nodeAssert, {
  apply(target, thisArg, args) {
    return recordCase(() => Reflect.apply(target, thisArg, args));
  },
  get(target, property) {
    const value = Reflect.get(target, property);
    if (typeof value !== 'function') return value;
    return (...args) => recordCase(() => Reflect.apply(value, target, args));
  },
});

class Element {
  constructor(id = '', dataset = {}, tag = '', doc = null) {
    this.tag = String(tag || '').toLowerCase();
    this.id = id;
    this.dataset = { ...dataset };
    this.hidden = false;
    this.disabled = false;
    this.checked = false;
    this.value = '';
    this.type = '';
    this.name = '';
    this.href = '';
    this.tabIndex = 0;
    this._textContent = '';
    this.classList = {
      values: new Set(),
      setFromString(v) { this.values = new Set(String(v || '').split(/\s+/).filter(Boolean)); },
      add(...names) { names.forEach(n => this.values.add(n)); },
      remove(...names) { names.forEach(n => this.values.delete(n)); },
      contains(n) { return this.values.has(n); },
      toString() { return Array.from(this.values).join(' '); }
    };
    this.children = [];
    this.listeners = {};
    this.attributes = {};
    this.style = {};
    this.parent = null;
    this.document = doc;
  }
  get className() { return this.classList.toString(); }
  set className(v) { this.classList.setFromString(v); }
  get textContent() {
    if (this.children.length === 0) return this._textContent;
    return this.children.map(c => c.textContent).join('');
  }
  set textContent(v) {
    this.children = [];
    this._textContent = String(v ?? '');
  }
  get innerHTML() { return ''; }
  set innerHTML(v) {
    this.children = [];
    this._textContent = '';
    if (v) {
      const parsed = parseHTML(v, this.document);
      for (const c of parsed.children) this.appendChild(c);
    }
  }
  appendChild(child) {
    child.parent = this;
    child.document = this.document;
    this.children.push(child);
    return child;
  }
  append(...children) {
    children.forEach(c => this.appendChild(c));
  }
  addEventListener(name, fn) {
    (this.listeners[name] ||= []).push(fn);
  }
  click() {
    if (this.type === 'radio' && this.name) {
      if (this.document) {
        const all = this.document.querySelectorAll(`input[name="${this.name}"]`);
        for (const r of all) { r.checked = false; }
      }
      this.checked = true;
    }
    for (const fn of this.listeners['click'] || []) {
      fn({ target: this, preventDefault() {}, stopPropagation() {} });
    }
    if (this.type === 'radio') {
      for (const fn of this.listeners['change'] || []) {
        fn({ target: this, preventDefault() {}, stopPropagation() {} });
      }
    }
    if (this.type === 'submit') {
      let p = this.parent;
      while (p && p.tag !== 'form') {
        p = p.parent;
      }
      if (p) {
        for (const fn of p.listeners['submit'] || []) {
          fn({ target: p, preventDefault() {}, stopPropagation() {} });
        }
      }
    }
  }
  keydown(key) {
    const event = { key, target: this, preventDefault() {}, stopPropagation() {} };
    for (const fn of this.listeners['keydown'] || []) {
      fn(event);
    }
  }
  focus() {
    if (this.document) this.document.activeElement = this;
  }
  setAttribute(n, v) {
    this.attributes[n] = String(v);
    if (n === 'class') this.classList.setFromString(v);
  }
  getAttribute(n) { return this.attributes[n] ?? null; }
  querySelector(sel) {
    return this.querySelectorAll(sel)[0] || null;
  }
  querySelectorAll(sel) {
    const parts = (sel || '').trim().split(/\s+/).filter(Boolean);
    if (parts.length > 1) {
      let current = [this];
      for (const part of parts) {
        const next = [];
        for (const el of current) {
          next.push(...el.querySelectorAll(part));
        }
        current = next;
      }
      return current;
    }
    const single = parts[0] || '';
    const res = [];
    const walk = (node) => {
      for (const c of node.children) {
        let match = false;
        if (single.startsWith('#') && c.id === single.slice(1)) match = true;
        else if (single === 'span:not(.mark-sep)' && c.tag === 'span' && !c.classList.contains('mark-sep')) match = true;
        else if (single.includes('.')) {
          const [t, cls] = single.split('.');
          if ((!t || c.tag === t) && c.classList.contains(cls)) match = true;
        }
        else if (single === c.tag) match = true;
        else if (single === 'input[name="thinking-choice"]' && c.tag === 'input' && c.name === 'thinking-choice') match = true;
        else if (single === 'input[name="thinking-choice"]:checked' && c.tag === 'input' && c.name === 'thinking-choice' && c.checked) match = true;
        else if (single.startsWith('input[name="') && c.tag === 'input' && c.name === single.slice(12, -2)) match = true;
        else if (single.startsWith('input[value="') && c.tag === 'input' && c.value === single.slice(13, -2)) match = true;
        if (match) res.push(c);
        walk(c);
      }
    };
    walk(this);
    return res;
  }
}

function parseAttributes(attrString) {
  const attrs = {};
  const regex = /([a-zA-Z0-9-]+)(?:=(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
  let m;
  while ((m = regex.exec(attrString)) !== null) {
    const key = m[1].toLowerCase();
    const val = m[2] !== undefined ? m[2] : (m[3] !== undefined ? m[3] : (m[4] !== undefined ? m[4] : ''));
    attrs[key] = val;
  }
  return attrs;
}

function parseHTML(htmlString, doc) {
  const root = new Element('', {}, '#root', doc);
  let current = root;
  const voidTags = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr']);

  let idx = 0;
  while (idx < htmlString.length) {
    const nextLt = htmlString.indexOf('<', idx);
    if (nextLt === -1) {
      const text = htmlString.slice(idx);
      if (text.trim()) {
        const textNode = new Element('', {}, '#text', doc);
        textNode.textContent = text;
        current.appendChild(textNode);
      }
      break;
    }
    if (nextLt > idx) {
      const text = htmlString.slice(idx, nextLt);
      if (text.trim()) {
        const textNode = new Element('', {}, '#text', doc);
        textNode.textContent = text;
        current.appendChild(textNode);
      }
    }
    const nextGt = htmlString.indexOf('>', nextLt);
    if (nextGt === -1) break;

    const tagContent = htmlString.slice(nextLt + 1, nextGt);
    idx = nextGt + 1;

    if (tagContent.startsWith('!--')) {
      const commentEnd = htmlString.indexOf('-->', nextLt);
      if (commentEnd !== -1) {
        idx = commentEnd + 3;
      }
      continue;
    }

    if (tagContent.startsWith('/')) {
      const tagName = tagContent.slice(1).trim().toLowerCase();
      let p = current;
      while (p && p !== root && p.tag !== tagName) {
        p = p.parent;
      }
      if (p && p !== root) {
        current = p.parent || root;
      }
      continue;
    }

    const selfClosing = tagContent.endsWith('/');
    const contentTrimmed = selfClosing ? tagContent.slice(0, -1).trim() : tagContent.trim();
    const spaceIdx = contentTrimmed.search(/\s/);
    const tagName = (spaceIdx === -1 ? contentTrimmed : contentTrimmed.slice(0, spaceIdx)).toLowerCase();
    const attrString = spaceIdx === -1 ? '' : contentTrimmed.slice(spaceIdx);

    const el = new Element('', {}, tagName, doc);
    const attrs = parseAttributes(attrString);
    for (const [key, val] of Object.entries(attrs)) {
      el.setAttribute(key, val);
      if (key === 'id') el.id = val;
      if (key === 'class') el.classList.setFromString(val);
      if (key === 'hidden') el.hidden = true;
      if (key === 'disabled') el.disabled = true;
      if (key === 'checked') el.checked = true;
      if (key === 'value') el.value = val;
      if (key === 'type') el.type = val;
      if (key === 'name') el.name = val;
      if (key === 'href') el.href = val;
      if (key === 'tabindex') el.tabIndex = parseInt(val, 10);
    }

    current.appendChild(el);
    if (!selfClosing && !voidTags.has(tagName)) {
      current = el;
    }
  }
  return root;
}

const stubCopy = {
  schema: "solstone.setup-copy.v1",
  heading: "stub-heading",
  welcome: {
    title: "stub-welcome-title",
    body: "stub-welcome-body",
    how_it_fits: ["stub-beat-1", "stub-beat-2", "stub-beat-3"]
  },
  fits: {
    title: "stub-fits-title",
    journal_term: "stub-journal-term",
    journal: "stub-journal-desc",
    app_term: "stub-app-term",
    app: "stub-app-desc",
    thinking_term: "stub-thinking-term",
    thinking: "stub-thinking-desc"
  },
  login: { label: "stub-login", help: "stub-login-help" },
  folder: { title: "stub-folder", help: "stub-folder-help", change: "stub-folder-change", continue: "stub-folder-continue" },
  mark: {
    title: "stub-mark-title",
    error: "stub-mark-error",
    help: "stub-mark-help",
    permanence: "stub-permanence",
    try_another: "stub-try-another",
    continue: "stub-mark-continue"
  },
  thinking: {
    title: "stub-thinking-title",
    choices: [
      { id: "local", label: "stub-local-label", sub: "stub-local-sub", description: "stub-local-desc" },
      { id: "spp", label: "stub-spp-label", sub: "stub-spp-sub", description: "stub-spp-desc" },
      { id: "byo", label: "stub-byo-label", sub: "stub-byo-sub", description: "stub-byo-desc" }
    ],
    default_line: "stub-default-line {size}",
    size_unknown: "stub-size-unknown",
    choose_later: "stub-choose-later",
    change: "stub-thinking-change",
    later: "stub-later-choice",
    continue: "stub-thinking-continue",
    check_unknown: "stub-check-unknown"
  },
  downloads: {
    speech: "stub-speech-name",
    thinking: "stub-thinking-name",
    progress: "stub-downloading-progress",
    cancel: "stub-cancel-btn",
    retry: "stub-retry-btn",
    error: "stub-download-error",
    idle: "stub-idle",
    complete: "stub-download-complete",
    cancelled: "stub-download-cancelled"
  },
  footer: "stub-footer-line",
  help: { text: "stub-help-text", link: "stub-help-link", href: "https://help.invalid" }
};

function defaultState() {
  return {
    schema: "solstone.setup-state.v1",
    done: false,
    redirect: "/app/home",
    mark: { render: null, state: "unavailable", proposal_id: null },
    thinking: { choice: null, saved: false, capability: "ok", download_bytes: null },
    downloads: { speech: { state: "idle" }, thinking: { state: "idle" } },
    remaining_required_steps: ["mark", "thinking"],
    copy: JSON.parse(JSON.stringify(stubCopy))
  };
}

function makeMark(words = ['alpha', 'beta'], hex1 = '#ff5500', hex2 = '#00aa55', rot = 0) {
  return {
    icon1: { name: 'icon1', svg: glyphs.anchor, color: { name: 'c1', hex: hex1 }, rot: rot },
    icon2: { name: 'icon2', svg: glyphs.bike, color: { name: 'c2', hex: hex2 }, rot: rot },
    words
  };
}

function settle() {
  return new Promise(resolve => setImmediate(resolve));
}

function createPageEnvironment(markup, scriptCode) {
  let currentTime = 0;
  let timerId = 1;
  let timers = [];

  function fakeSetTimeout(fn, delay) {
    const id = timerId++;
    timers.push({ id, fn, time: currentTime + Math.max(0, delay || 0), isInterval: false });
    return id;
  }
  function fakeClearTimeout(id) {
    timers = timers.filter(t => t.id !== id);
  }
  function fakeSetInterval(fn, interval) {
    const id = timerId++;
    timers.push({ id, fn, time: currentTime + Math.max(0, interval || 0), interval: Math.max(1, interval || 1), isInterval: true });
    return id;
  }
  function fakeClearInterval(id) {
    timers = timers.filter(t => t.id !== id);
  }

  function advanceTime(ms) {
    const target = currentTime + ms;
    while (true) {
      timers.sort((a, b) => a.time - b.time || a.id - b.id);
      const nextTimer = timers.find(t => t.time <= target);
      if (!nextTimer) {
        currentTime = target;
        break;
      }
      currentTime = nextTimer.time;
      if (nextTimer.isInterval) {
        nextTimer.time += nextTimer.interval;
      } else {
        fakeClearTimeout(nextTimer.id);
      }
      nextTimer.fn();
    }
  }

  const domListeners = {};
  const doc = {
    title: '',
    querySelectorAll(sel) { return root.querySelectorAll(sel); },
    querySelector(sel) { return root.querySelector(sel); },
    getElementById(id) { return root.querySelector('#' + id); },
    createElement(tag) { return new Element('', {}, tag, doc); },
    addEventListener(name, fn) { (domListeners[name] ||= []).push(fn); }
  };
  const root = parseHTML(markup, doc);

  const pendingRequests = [];
  const allRequests = [];

  const windowObj = {
    location: { href: '/init' },
    apiJson(url, opts = {}) {
      return new Promise((resolve, reject) => {
        const req = {
          url,
          opts,
          resolve: (val) => {
            req.isPending = false;
            resolve(val);
          },
          reject: (err) => {
            req.isPending = false;
            reject(err);
          },
          isPending: true
        };
        if (opts.signal) {
          if (opts.signal.aborted) {
            req.reject(new Error('AbortError: The operation was aborted'));
            return;
          }
          opts.signal.addEventListener('abort', () => {
            if (req.isPending) {
              req.reject(new Error('AbortError: The operation was aborted'));
            }
          });
        }
        pendingRequests.push(req);
        allRequests.push(req);
      });
    }
  };

  const context = vm.createContext({
    window: windowObj,
    document: doc,
    setTimeout: fakeSetTimeout,
    clearTimeout: fakeClearTimeout,
    setInterval: fakeSetInterval,
    clearInterval: fakeClearInterval,
    AbortController: globalThis.AbortController,
    console
  });

  vm.runInContext(scriptCode, context);

  function boot() {
    for (const fn of domListeners['DOMContentLoaded'] || []) {
      fn();
    }
  }

  function nextPending(predicate) {
    if (typeof predicate === 'function') {
      return pendingRequests.find(r => r.isPending && predicate(r)) || null;
    }
    if (typeof predicate === 'string') {
      return pendingRequests.find(r => r.isPending && r.url === predicate) || null;
    }
    return pendingRequests.find(r => r.isPending) || null;
  }

  return {
    doc,
    window: windowObj,
    pendingRequests,
    allRequests,
    nextPending,
    advanceTime,
    boot
  };
}

async function runTests(manifestDir) {
  glyphs = JSON.parse(fs.readFileSync(path.join(manifestDir, 'assets/mark_assets/glyphs.json'), 'utf8'));
  const html = fs.readFileSync(path.join(manifestDir, 'assets/init.html'), 'utf8');
  const lastScriptOpen = html.lastIndexOf('<script>');
  const lastScriptClose = html.lastIndexOf('</script>');
  const scriptCode = html.slice(lastScriptOpen + '<script>'.length, lastScriptClose);
  const markup = html.slice(0, lastScriptOpen);

  // CASE 0A: Initial state read reject, bootstrap copy check, and retry
  {
    const env = createPageEnvironment(markup, scriptCode);
    const bootstrapEl = env.doc.getElementById('setup-copy-bootstrap');
    bootstrapEl.textContent = JSON.stringify(stubCopy);
    env.boot();
    await settle();

    assert.strictEqual(env.doc.getElementById('setup-heading').textContent, stubCopy.heading);
    const retryBtn = env.doc.getElementById('setup-retry');
    assert.strictEqual(retryBtn.textContent, stubCopy.downloads.retry);
    assert.strictEqual(retryBtn.hidden, true);

    const req1 = env.nextPending('/init/api/state');
    assert.ok(req1);
    req1.reject(new Error('initial-state-failure'));
    await settle();

    assert.ok(env.doc.getElementById('mark-error').textContent.includes('initial-state-failure'));
    assert.strictEqual(env.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'regenerate_mark'), false);

    retryBtn.click();
    await settle();

    const req2 = env.nextPending('/init/api/state');
    assert.ok(req2);
    req2.resolve({
      ...defaultState(),
      mark: { state: 'unavailable' }
    });
    await settle();

    const regenCalls = env.allRequests.filter(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'regenerate_mark');
    assert.strictEqual(regenCalls.length, 1);
  }

  // CASE 0B: Initial state read deadline abort (8000ms), activeElement, role alert, retry without regenerate
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    await settle();

    const req1 = env.nextPending('/init/api/state');
    assert.ok(req1);

    env.advanceTime(8000);
    await settle();

    const markErr = env.doc.getElementById('mark-error');
    assert.ok(markErr.textContent.length > 0);
    assert.strictEqual(env.doc.activeElement, markErr);
    assert.strictEqual(markErr.getAttribute('role'), 'alert');

    const reqCountBefore = env.allRequests.filter(r => r.url === '/init/api/state').length;
    env.advanceTime(8000);
    await settle();
    const reqCountAfter = env.allRequests.filter(r => r.url === '/init/api/state').length;
    assert.strictEqual(reqCountBefore, reqCountAfter);

    const retryBtn = env.doc.getElementById('setup-retry');
    retryBtn.click();
    await settle();

    const req2 = env.nextPending('/init/api/state');
    assert.ok(req2);
    req2.resolve({
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'P_retry', render: makeMark(['prop1', 'prop2']) }
    });
    await settle();

    assert.strictEqual(retryBtn.hidden, true);
    assert.ok(env.doc.getElementById('journal-mark-display').textContent.includes('prop1'));
    const anyRegen = env.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'regenerate_mark');
    assert.strictEqual(anyRegen, false);
  }

  // CASE 0C: Mark snapshot validation and omitted mark
  {
    // Boot proposed with proposal_id and no render
    const env1 = createPageEnvironment(markup, scriptCode);
    env1.boot();
    await settle();
    env1.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'P_no_render', render: null }
    });
    await settle();

    const contBtn1 = env1.doc.getElementById('mark-continue');
    assert.strictEqual(contBtn1.disabled, true);
    assert.strictEqual(env1.doc.getElementById('journal-mark-display').textContent, '');
    assert.strictEqual(env1.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'regenerate_mark'), false);

    // Boot with mark omitted
    const env2 = createPageEnvironment(markup, scriptCode);
    env2.boot();
    await settle();
    env2.nextPending().resolve({
      ...defaultState(),
      mark: null
    });
    await settle();

    assert.ok(env2.doc.getElementById('mark-error').textContent.length > 0);
    assert.strictEqual(env2.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'regenerate_mark'), false);
  }

  // CASE 1: Fresh unavailable state, try another, continue, locked state advance
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    await settle();

    const req1 = env.nextPending();
    assert.ok(req1 && req1.url === '/init/api/state');
    req1.resolve(defaultState());
    await settle();

    const req2 = env.nextPending();
    assert.ok(req2 && req2.url === '/init/api/action');
    const p1 = JSON.parse(req2.opts.body);
    assert.strictEqual(p1.action, 'regenerate_mark');

    req2.resolve({
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'P1', render: makeMark(['alpha', 'beta']) }
    });
    await settle();

    const display = env.doc.getElementById('journal-mark-display');
    assert.ok(display.textContent.includes('alpha') && display.textContent.includes('beta'));

    const tryAnother = env.doc.getElementById('mark-try-another');
    tryAnother.click();
    await settle();

    const req3 = env.nextPending();
    assert.ok(req3 && req3.url === '/init/api/action');
    const p2 = JSON.parse(req3.opts.body);
    assert.strictEqual(p2.action, 'regenerate_mark');

    req3.resolve({
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'P2', render: makeMark(['gamma', 'delta']) }
    });
    await settle();

    const contBtn = env.doc.getElementById('mark-continue');
    contBtn.click();
    await settle();

    const req4 = env.nextPending();
    assert.ok(req4 && req4.url === '/init/api/action');
    const p3 = JSON.parse(req4.opts.body);
    assert.strictEqual(p3.action, 'commit_mark');
    assert.strictEqual(p3.proposal_id, 'P2');

    // Twin page for proposed commit resolve
    {
      const twin = createPageEnvironment(markup, scriptCode);
      twin.boot();
      await settle();
      twin.nextPending().resolve(defaultState());
      await settle();
      twin.nextPending().resolve({ ...defaultState(), mark: { state: 'proposed', proposal_id: 'P2', render: makeMark(['gamma', 'delta']) } });
      await settle();
      twin.doc.getElementById('mark-continue').click();
      await settle();
      twin.nextPending().resolve({ ...defaultState(), mark: { state: 'proposed', proposal_id: 'P2', render: makeMark(['gamma', 'delta']) } });
      await settle();
      assert.strictEqual(twin.doc.getElementById('setup-thinking').hidden, true);
    }

    // Main page resolves locked
    req4.resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'P2', render: makeMark(['gamma', 'delta']) }
    });
    await settle();

    assert.strictEqual(env.doc.getElementById('setup-thinking').hidden, false);
    assert.strictEqual(env.doc.getElementById('setup-mark').hidden, true);

    // Twin page for commit throw
    {
      const twinThrow = createPageEnvironment(markup, scriptCode);
      twinThrow.boot();
      await settle();
      twinThrow.nextPending().resolve(defaultState());
      await settle();
      twinThrow.nextPending().resolve({ ...defaultState(), mark: { state: 'proposed', proposal_id: 'P2', render: makeMark() } });
      await settle();
      twinThrow.doc.getElementById('mark-continue').click();
      await settle();
      twinThrow.nextPending().reject({ rawDetail: 'err-commit-raw', serverMessage: 'err-server', message: 'err' });
      await settle();
      assert.strictEqual(twinThrow.doc.getElementById('setup-mark').hidden, false);
      assert.ok(twinThrow.doc.getElementById('mark-error').textContent.includes('err-commit-raw'));
    }

    // Twin page for already locked boot
    {
      const twinLocked = createPageEnvironment(markup, scriptCode);
      twinLocked.boot();
      await settle();
      twinLocked.nextPending().resolve({
        ...defaultState(),
        mark: { state: 'locked', proposal_id: 'P_locked', render: makeMark() }
      });
      await settle();
      const anyRegen = twinLocked.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'regenerate_mark');
      assert.strictEqual(anyRegen, false);
    }
  }

  // CASE 2: Boot proposed X, poll in flight held, try another held, click continue twice -> no commit, poll resolve Z ignored, Y adopted, commit Y locked, stale pre-lock poll ignored
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    await settle();

    const bootState = {
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'X', render: makeMark(['wordX1', 'wordX2']) },
      downloads: { thinking: { state: 'downloading', attempt_id: 't-0', bytes_done: 1, bytes_total: 9 }, speech: { state: 'idle' } }
    };
    env.nextPending().resolve(bootState);
    await settle();

    env.advanceTime(1000);
    const heldPoll = env.nextPending();
    assert.ok(heldPoll && heldPoll.url === '/init/api/state');

    env.doc.getElementById('mark-try-another').click();
    await settle();
    const heldRegen = env.nextPending('/init/api/action');
    assert.ok(heldRegen && heldRegen.url === '/init/api/action');

    env.doc.getElementById('mark-continue').click();
    env.doc.getElementById('mark-continue').click();
    await settle();

    const commitPosted = env.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'commit_mark');
    assert.strictEqual(commitPosted, false);

    heldPoll.resolve({
      ...defaultState(),
      downloads: bootState.downloads,
      mark: { state: 'proposed', proposal_id: 'Z', render: makeMark(['wordZ1', 'wordZ2']) }
    });
    await settle();

    const display = env.doc.getElementById('journal-mark-display');
    assert.ok(display.textContent.includes('wordX1') && display.textContent.includes('wordX2'));
    assert.strictEqual(env.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).proposal_id === 'Z'), false);

    heldRegen.resolve({
      ...defaultState(),
      downloads: bootState.downloads,
      mark: { state: 'proposed', proposal_id: 'Y', render: makeMark(['wordY1', 'wordY2']) }
    });
    await settle();

    assert.ok(display.textContent.includes('wordY1') && display.textContent.includes('wordY2'));

    env.doc.getElementById('mark-continue').click();
    await settle();

    const commitReq = env.nextPending('/init/api/action');
    assert.ok(commitReq && commitReq.url === '/init/api/action');
    const commitBody = JSON.parse(commitReq.opts.body);
    assert.strictEqual(commitBody.action, 'commit_mark');
    assert.strictEqual(commitBody.proposal_id, 'Y');

    commitReq.resolve({
      ...defaultState(),
      downloads: bootState.downloads,
      mark: { state: 'locked', proposal_id: 'Y', render: makeMark(['wordY1', 'wordY2']) }
    });
    await settle();

    env.advanceTime(1000);
    const postLockPoll = env.nextPending('/init/api/state');
    assert.ok(postLockPoll && postLockPoll.url === '/init/api/state');

    postLockPoll.resolve({
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'X', render: makeMark(['wordX1', 'wordX2']) }
    });
    await settle();

    assert.ok(display.textContent.includes('wordY1') && display.textContent.includes('wordY2'));
    assert.ok(env.doc.getElementById('mark-try-another').hidden || env.doc.getElementById('setup-mark').hidden);
    const commitCalls = env.allRequests.filter(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'commit_mark');
    for (const c of commitCalls) {
      assert.strictEqual(JSON.parse(c.opts.body).proposal_id, 'Y');
    }
  }

  // CASE 3: Proposal conflict 409, clicks during held refresh, resolution of Y, 8000ms abort on second conflict, delayed resolution of X ignored
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    await settle();

    env.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'X', render: makeMark(['wordX1', 'wordX2']) }
    });
    await settle();

    const display = env.doc.getElementById('journal-mark-display');
    assert.ok(display.textContent.includes('wordX1'));

    // Commit returns 409 proposal_conflict
    env.doc.getElementById('mark-continue').click();
    await settle();

    const commitReq = env.nextPending();
    commitReq.reject({
      status: 409,
      reasonCode: 'proposal_conflict',
      rawDetail: 'detail-conflict'
    });
    await settle();

    assert.strictEqual(env.doc.getElementById('mark-error').textContent, 'detail-conflict');

    const stateReq1 = env.nextPending('/init/api/state');
    assert.ok(stateReq1);

    // While the refresh GET is held, click continue and try another. No further action is posted.
    const actionCountBefore = env.allRequests.filter(r => r.url === '/init/api/action').length;
    env.doc.getElementById('mark-continue').click();
    env.doc.getElementById('mark-try-another').click();
    await settle();
    const actionCountAfter = env.allRequests.filter(r => r.url === '/init/api/action').length;
    assert.strictEqual(actionCountBefore, actionCountAfter);

    // Resolve the refresh as valid Y. Y's words show. Y is not posted until a later continue click.
    stateReq1.resolve({
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'Y', render: makeMark(['wordY1', 'wordY2']) }
    });
    await settle();

    assert.ok(display.textContent.includes('wordY1') && display.textContent.includes('wordY2'));
    assert.strictEqual(env.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).proposal_id === 'Y'), false);

    // After Y is shown, continue again, 409, hold that second refresh. Advance 8000 so it aborts. Words stay Y.
    env.doc.getElementById('mark-continue').click();
    await settle();
    const commitReq2 = env.nextPending('/init/api/action');
    assert.strictEqual(JSON.parse(commitReq2.opts.body).proposal_id, 'Y');
    commitReq2.reject({
      status: 409,
      reasonCode: 'proposal_conflict',
      rawDetail: 'detail-conflict-2'
    });
    await settle();

    const stateReq2 = env.nextPending('/init/api/state');
    assert.ok(stateReq2);

    env.advanceTime(8000);
    await settle();

    assert.ok(display.textContent.includes('wordY1') && display.textContent.includes('wordY2'));

    // Then resolve that aborted request with valid X. Words stay Y. No regenerate_mark.
    stateReq2.resolve({
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'X', render: makeMark(['wordX1', 'wordX2']) }
    });
    await settle();

    assert.ok(display.textContent.includes('wordY1') && display.textContent.includes('wordY2'));
    assert.strictEqual(env.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'regenerate_mark'), false);
  }

  // CASE 4: Capability & Size variations
  {
    // Twin A (1000 bytes)
    const envA = createPageEnvironment(markup, scriptCode);
    envA.boot();
    await settle();
    envA.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok', download_bytes: 1000 }
    });
    await settle();
    const capReqA = envA.nextPending();
    if (capReqA && capReqA.url === '/init/api/local-capability') {
      capReqA.resolve({ overall: 'ok', checks: [] });
      await settle();
    }
    const summaryA = envA.doc.getElementById('thinking-summary').textContent;
    assert.strictEqual(summaryA, stubCopy.thinking.default_line.replace('{size}', '1 KB'));

    // Twin B (2000000 bytes)
    const envB = createPageEnvironment(markup, scriptCode);
    envB.boot();
    await settle();
    envB.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok', download_bytes: 2000000 }
    });
    await settle();
    const capReqB = envB.nextPending();
    if (capReqB && capReqB.url === '/init/api/local-capability') {
      capReqB.resolve({ overall: 'ok', checks: [] });
      await settle();
    }
    const summaryB = envB.doc.getElementById('thinking-summary').textContent;
    assert.strictEqual(summaryB, stubCopy.thinking.default_line.replace('{size}', '2 MB'));
    assert.notStrictEqual(summaryA, summaryB);

    // Twin C (null bytes)
    const envC = createPageEnvironment(markup, scriptCode);
    envC.boot();
    await settle();
    envC.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok', download_bytes: null }
    });
    await settle();
    const capReqC = envC.nextPending();
    if (capReqC && capReqC.url === '/init/api/local-capability') {
      capReqC.resolve({ overall: 'ok', checks: [] });
      await settle();
    }
    const summaryC = envC.doc.getElementById('thinking-summary').textContent;
    assert.strictEqual(summaryC, stubCopy.thinking.default_line.replace('{size}', stubCopy.thinking.size_unknown));
    assert.strictEqual(summaryC.includes('1 KB') || summaryC.includes('2 MB') || summaryC.includes('1000') || summaryC.includes('2000000'), false);

    // Twin D (blocked capability)
    const envD = createPageEnvironment(markup, scriptCode);
    envD.boot();
    await settle();
    envD.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'blocked', download_bytes: null }
    });
    await settle();
    const capReqD = envD.nextPending();
    assert.ok(capReqD && capReqD.url === '/init/api/local-capability');
    capReqD.resolve({
      overall: 'blocked',
      checks: [{ name: 'local', severity: 'blocked', detail: 'detail-blocked' }]
    });
    await settle();

    const localInputD = envD.doc.querySelector('input[value="local"]');
    assert.strictEqual(localInputD.disabled, true);
    assert.strictEqual(envD.doc.querySelector('input[value="byo"]').disabled, false);
    assert.strictEqual(envD.doc.querySelector('input[value="spp"]').disabled, false);
    assert.strictEqual(envD.doc.querySelector('input[value="later"]').disabled, false);
    assert.strictEqual(localInputD.parent.classList.contains('is-unavailable'), true);
    // A computer that can't run local gets no greyed choice: the row is gone and its reason stays.
    assert.strictEqual(localInputD.parent.hidden, true);

    const reasonSpanD = envD.doc.getElementById('thinking-local-reason');
    assert.ok(reasonSpanD && reasonSpanD.textContent.includes('detail-blocked'));
    assert.strictEqual(reasonSpanD.classList.contains('error-message'), false);
    const errMsgsD = envD.doc.querySelectorAll('.error-message');
    assert.ok(errMsgsD.every(el => !el.textContent.includes('detail-blocked')));

    assert.strictEqual(envD.doc.getElementById('thinking-summary').hidden, false);
    assert.strictEqual(envD.doc.getElementById('thinking-summary').textContent, stubCopy.thinking.choose_later);
    assert.strictEqual(envD.doc.getElementById('thinking-choose-later').hidden, true);
    const checkedD = envD.doc.querySelectorAll('input[name="thinking-choice"]:checked');
    assert.strictEqual(checkedD.length, 0);

    envD.doc.getElementById('thinking-continue').click();
    await settle();
    const saveReqD = envD.nextPending();
    assert.ok(saveReqD && saveReqD.url === '/init/api/action');
    assert.strictEqual(JSON.parse(saveReqD.opts.body).action, 'save_thinking');
    assert.strictEqual(JSON.parse(saveReqD.opts.body).choice, 'later');

    // Twin E (unknown capability)
    const envE = createPageEnvironment(markup, scriptCode);
    envE.boot();
    await settle();
    envE.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'unknown', download_bytes: null }
    });
    await settle();
    const capReqE = envE.nextPending();
    if (capReqE) {
      capReqE.resolve({ overall: 'unknown', checks: [] });
      await settle();
    }
    const localInputE = envE.doc.querySelector('input[value="local"]');
    assert.strictEqual(localInputE.disabled, true);
    assert.strictEqual(localInputE.parent.classList.contains('is-unavailable'), true);
    const reasonSpanE = envE.doc.getElementById('thinking-local-reason');
    assert.ok(reasonSpanE && reasonSpanE.textContent.length > 0);
    assert.strictEqual(reasonSpanE.hidden, false);
    assert.strictEqual(envE.doc.getElementById('thinking-summary').hidden, false);
    assert.strictEqual(envE.doc.getElementById('thinking-summary').textContent, stubCopy.thinking.choose_later);
    assert.strictEqual(envE.doc.getElementById('thinking-choose-later').hidden, true);
    assert.strictEqual(envE.doc.querySelectorAll('input[name="thinking-choice"]:checked').length, 0);

    envE.doc.getElementById('thinking-continue').click();
    await settle();
    const saveReqE = envE.nextPending();
    assert.ok(saveReqE && saveReqE.url === '/init/api/action');
    assert.strictEqual(JSON.parse(saveReqE.opts.body).action, 'save_thinking');
    assert.notStrictEqual(JSON.parse(saveReqE.opts.body).choice, 'local');

    // Twin E2 (unknown capability with one check detail)
    const envE2 = createPageEnvironment(markup, scriptCode);
    envE2.boot();
    await settle();
    envE2.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'unknown', download_bytes: null }
    });
    await settle();
    const capReqE2 = envE2.nextPending();
    if (capReqE2) {
      capReqE2.resolve({
        overall: 'unknown',
        checks: [{ name: 'local', severity: 'unknown', detail: 'detail-unknown' }]
      });
      await settle();
    }
    const localInputE2 = envE2.doc.querySelector('input[value="local"]');
    assert.strictEqual(localInputE2.disabled, true);
    assert.strictEqual(localInputE2.parent.classList.contains('is-unavailable'), true);
    const reasonSpanE2 = envE2.doc.getElementById('thinking-local-reason');
    assert.ok(reasonSpanE2 && reasonSpanE2.textContent.includes('detail-unknown'));
    assert.strictEqual(reasonSpanE2.hidden, false);
    assert.strictEqual(envE2.doc.querySelectorAll('input[name="thinking-choice"]:checked').length, 0);

    // Twin E3 (unknown capability + local_admitted: false)
    const envE3 = createPageEnvironment(markup, scriptCode);
    envE3.boot();
    await settle();
    envE3.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'unknown', download_bytes: null }
    });
    await settle();
    const capReqE3 = envE3.nextPending();
    if (capReqE3) {
      capReqE3.resolve({ overall: 'unknown', local_admitted: false, checks: [] });
      await settle();
    }
    const localInputE3 = envE3.doc.querySelector('input[value="local"]');
    assert.strictEqual(localInputE3.disabled, true);
    assert.strictEqual(localInputE3.parent.classList.contains('is-unavailable'), true);
    const reasonSpanE3 = envE3.doc.getElementById('thinking-local-reason');
    assert.ok(reasonSpanE3 && reasonSpanE3.textContent.length > 0);
    assert.strictEqual(reasonSpanE3.hidden, false);
    assert.strictEqual(envE3.doc.querySelectorAll('input[name="thinking-choice"]:checked').length, 0);

    envE3.doc.getElementById('thinking-continue').click();
    await settle();
    const saveReqE3 = envE3.nextPending();
    assert.ok(saveReqE3 && saveReqE3.url === '/init/api/action');
    assert.strictEqual(JSON.parse(saveReqE3.opts.body).action, 'save_thinking');
    assert.notStrictEqual(JSON.parse(saveReqE3.opts.body).choice, 'local');

    // Twin E4 (unknown capability + local_admitted: true)
    const envE4 = createPageEnvironment(markup, scriptCode);
    envE4.boot();
    await settle();
    envE4.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'unknown', download_bytes: null }
    });
    await settle();
    const capReqE4 = envE4.nextPending();
    if (capReqE4) {
      capReqE4.resolve({ overall: 'unknown', local_admitted: true, checks: [] });
      await settle();
    }
    const localInputE4 = envE4.doc.querySelector('input[value="local"]');
    assert.strictEqual(localInputE4.disabled, false);
    assert.strictEqual(localInputE4.parent.classList.contains('is-unavailable'), false);
    const reasonSpanE4 = envE4.doc.getElementById('thinking-local-reason');
    assert.ok(reasonSpanE4 && reasonSpanE4.textContent.length > 0);
    assert.strictEqual(reasonSpanE4.hidden, false);
    assert.strictEqual(reasonSpanE4.classList.contains('error-message'), false);
    assert.strictEqual(envE4.doc.querySelectorAll('input[name="thinking-choice"]:checked').length, 0);

    localInputE4.click();
    await settle();
    assert.strictEqual(envE4.doc.querySelectorAll('input[name="thinking-choice"]:checked').length, 1);
    assert.strictEqual(localInputE4.checked, true);

    envE4.doc.getElementById('thinking-continue').click();
    await settle();
    const saveReqE4 = envE4.nextPending();
    assert.ok(saveReqE4 && saveReqE4.url === '/init/api/action');
    assert.strictEqual(JSON.parse(saveReqE4.opts.body).action, 'save_thinking');
    assert.strictEqual(JSON.parse(saveReqE4.opts.body).choice, 'local');

    // Twin F (capability failure)
    const envF = createPageEnvironment(markup, scriptCode);
    envF.boot();
    await settle();
    envF.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok', download_bytes: null }
    });
    await settle();
    const capReqF = envF.nextPending();
    if (capReqF) {
      capReqF.reject(new Error('probe failed'));
      await settle();
    }
    const localInputF = envF.doc.querySelector('input[value="local"]');
    assert.strictEqual(localInputF.disabled, false);
    const reasonSpanF = envF.doc.getElementById('thinking-local-reason');
    assert.strictEqual(reasonSpanF ? reasonSpanF.textContent : '', '');

    // Twin G (state capability ok, local-capability warning)
    const envG = createPageEnvironment(markup, scriptCode);
    envG.boot();
    await settle();
    envG.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok', download_bytes: 4000 }
    });
    await settle();
    const capReqG = envG.nextPending();
    assert.ok(capReqG && capReqG.url === '/init/api/local-capability');
    capReqG.resolve({
      overall: 'warning',
      checks: [{ name: 'gpu', severity: 'warning', detail: 'gpu slow' }]
    });
    await settle();
    const localInputG = envG.doc.querySelector('input[value="local"]');
    assert.strictEqual(localInputG.disabled, true);
    assert.strictEqual(localInputG.parent.classList.contains('is-unavailable'), true);
    assert.strictEqual(envG.doc.querySelectorAll('input[name="thinking-choice"]:checked').length, 0);
    const summaryG = envG.doc.getElementById('thinking-summary').textContent;
    assert.strictEqual(summaryG.includes('4 KB'), false);
    assert.strictEqual(summaryG, stubCopy.thinking.choose_later);

    envG.doc.getElementById('thinking-continue').click();
    await settle();
    const saveReqG = envG.nextPending();
    assert.ok(saveReqG && saveReqG.url === '/init/api/action');
    assert.strictEqual(JSON.parse(saveReqG.opts.body).action, 'save_thinking');
    assert.notStrictEqual(JSON.parse(saveReqG.opts.body).choice, 'local');

    // Before the capability request resolves, setup state capability unknown: local enabled, not is-unavailable
    const envBefore = createPageEnvironment(markup, scriptCode);
    envBefore.boot();
    await settle();
    envBefore.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'unknown', download_bytes: null }
    });
    await settle();
    const localInputBefore = envBefore.doc.querySelector('input[value="local"]');
    assert.strictEqual(localInputBefore.disabled, false);
    assert.strictEqual(localInputBefore.parent.classList.contains('is-unavailable'), false);

    // Show a non-empty #thinking-error, then fire choice change listener -> #thinking-error is hidden and empty
    const errorEl = envBefore.doc.getElementById('thinking-error');
    errorEl.textContent = 'some error';
    errorEl.hidden = false;
    const laterInput = envBefore.doc.querySelector('input[value="later"]');
    laterInput.click();
    await settle();
    assert.strictEqual(errorEl.hidden, true);
    assert.strictEqual(errorEl.textContent, '');
  }

  // CASE 5: Options revealed, poll does not clear user pick, continue paths, done:false leaves /init
  {
    const choices = ['byo', 'local', 'spp', 'later'];
    for (const targetChoice of choices) {
      const env = createPageEnvironment(markup, scriptCode);
      env.boot();
      await settle();

      env.nextPending().resolve({
        ...defaultState(),
        mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
        thinking: { choice: null, saved: false, capability: 'ok', download_bytes: null },
        downloads: { speech: { state: 'downloading', bytes_done: 1, bytes_total: 10 }, thinking: { state: 'idle' } }
      });
      await settle();

      const cap = env.nextPending();
      if (cap && cap.url === '/init/api/local-capability') {
        cap.resolve({ overall: 'ok', checks: [] });
        await settle();
      }

      env.doc.getElementById('thinking-change').click();
      assert.strictEqual(env.doc.getElementById('thinking-options').hidden, false);

      const allLabels = env.doc.querySelectorAll('label.radio-option');
      const localL = allLabels.find(l => l.querySelector('input[value="local"]'));
      const sppL = allLabels.find(l => l.querySelector('input[value="spp"]'));
      const byoL = allLabels.find(l => l.querySelector('input[value="byo"]'));
      const laterL = allLabels.find(l => l.querySelector('input[value="later"]'));
      assert.ok(localL.textContent.includes(stubCopy.thinking.choices[0].label));
      assert.ok(localL.textContent.includes(stubCopy.thinking.choices[0].sub));
      assert.ok(localL.textContent.includes(stubCopy.thinking.choices[0].description));
      assert.ok(sppL.textContent.includes(stubCopy.thinking.choices[1].label));
      assert.ok(sppL.textContent.includes(stubCopy.thinking.choices[1].sub));
      assert.ok(sppL.textContent.includes(stubCopy.thinking.choices[1].description));
      assert.ok(byoL.textContent.includes(stubCopy.thinking.choices[2].label));
      assert.ok(byoL.textContent.includes(stubCopy.thinking.choices[2].sub));
      assert.ok(byoL.textContent.includes(stubCopy.thinking.choices[2].description));
      assert.strictEqual(laterL.textContent, stubCopy.thinking.later);
      assert.strictEqual(laterL.textContent.includes(stubCopy.thinking.choices[0].description), false);

      if (targetChoice !== 'local') {
        const radio = env.doc.querySelectorAll('input[name="thinking-choice"]').find(r => r.value === targetChoice);
        assert.ok(radio);
        radio.click();
        await settle();
      }

      env.advanceTime(1000);
      const pollReq = env.nextPending();
      if (pollReq && pollReq.url === '/init/api/state') {
        pollReq.resolve({
          ...defaultState(),
          thinking: { choice: null, saved: false, capability: 'ok' },
          done: false,
          redirect: '/app/home',
          downloads: { speech: { state: 'downloading', bytes_done: 2, bytes_total: 10 }, thinking: { state: 'idle' } }
        });
        await settle();
      }

      assert.strictEqual(env.window.location.href, '/init');

      if (targetChoice !== 'local') {
        const radioAfter = env.doc.querySelectorAll('input[name="thinking-choice"]').find(r => r.value === targetChoice);
        assert.strictEqual(radioAfter.checked, true);
      }

      env.doc.getElementById('thinking-continue').click();
      await settle();

      const saveReq = env.nextPending();
      assert.ok(saveReq && saveReq.url === '/init/api/action');
      assert.strictEqual(JSON.parse(saveReq.opts.body).action, 'save_thinking');
      assert.strictEqual(JSON.parse(saveReq.opts.body).choice, targetChoice);

      saveReq.resolve({
        ...defaultState(),
        thinking: { choice: targetChoice, saved: true, capability: 'ok' }
      });
      await settle();

      const finReq = env.nextPending();
      assert.ok(finReq && finReq.url === '/init/api/action');
      assert.strictEqual(JSON.parse(finReq.opts.body).action, 'finalize');

      const expectedRedirect = `/app/redirect-${targetChoice}`;
      finReq.resolve({
        ...defaultState(),
        done: true,
        redirect: expectedRedirect
      });
      await settle();

      assert.strictEqual(env.window.location.href, expectedRedirect);
    }

    // Done: false leaves /init
    {
      const envDoneFalse = createPageEnvironment(markup, scriptCode);
      envDoneFalse.boot();
      await settle();
      envDoneFalse.nextPending().resolve({
        ...defaultState(),
        mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
        thinking: { choice: 'later', saved: true }
      });
      await settle();
      const cap = envDoneFalse.nextPending();
      if (cap) { cap.resolve({ overall: 'ok', checks: [] }); await settle(); }

      envDoneFalse.doc.getElementById('thinking-continue').click();
      await settle();
      const finReq = envDoneFalse.nextPending();
      finReq.resolve({ ...defaultState(), done: false, redirect: '/app/should-not-nav' });
      await settle();
      assert.strictEqual(envDoneFalse.window.location.href, '/init');
    }
  }

  // CASE 6: Stub copy text checks and request payload hygiene
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    await settle();

    env.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok' },
      downloads: { thinking: { state: 'downloading', attempt_id: 't-1', bytes_done: 5, bytes_total: 10 }, speech: { state: 'idle' } }
    });
    await settle();
    const cap = env.nextPending();
    if (cap) { cap.resolve({ overall: 'ok', checks: [] }); await settle(); }

    env.doc.getElementById('thinking-change').click();

    // Check heading
    assert.strictEqual(env.doc.getElementById('setup-heading').textContent, stubCopy.heading);
    assert.strictEqual(env.doc.title, stubCopy.heading);

    // Permanence on proposed page
    {
      const envP = createPageEnvironment(markup, scriptCode);
      envP.boot();
      await settle();
      envP.nextPending().resolve({
        ...defaultState(),
        mark: { state: 'proposed', proposal_id: 'X', render: makeMark() }
      });
      await settle();
      assert.strictEqual(envP.doc.getElementById('mark-permanence').textContent, stubCopy.mark.permanence);
      assert.strictEqual(envP.doc.getElementById('mark-try-another').textContent, stubCopy.mark.try_another);
      assert.strictEqual(envP.doc.getElementById('mark-continue').textContent, stubCopy.mark.continue);
    }

    assert.strictEqual(env.doc.getElementById('thinking-title').textContent, stubCopy.thinking.title);
    assert.strictEqual(env.doc.getElementById('thinking-change').textContent, stubCopy.thinking.change);
    assert.strictEqual(env.doc.getElementById('thinking-continue').textContent, stubCopy.thinking.continue);
    assert.strictEqual(env.doc.getElementById('setup-footer').textContent, stubCopy.footer);
    assert.strictEqual(env.doc.getElementById('setup-help').hidden, false);
    assert.strictEqual(env.doc.getElementById('setup-help-link').href, stubCopy.help.href);

    assert.strictEqual(env.doc.getElementById('fits-journal-term').textContent, stubCopy.fits.journal_term);
    assert.strictEqual(env.doc.getElementById('fits-journal').textContent, stubCopy.fits.journal);
    assert.strictEqual(env.doc.getElementById('fits-app-term').textContent, stubCopy.fits.app_term);
    assert.strictEqual(env.doc.getElementById('fits-app').textContent, stubCopy.fits.app);
    assert.strictEqual(env.doc.getElementById('fits-thinking-term').textContent, stubCopy.fits.thinking_term);
    assert.strictEqual(env.doc.getElementById('fits-thinking').textContent, stubCopy.fits.thinking);
    assert.notStrictEqual(env.doc.getElementById('section-machine-card').textContent, stubCopy.welcome.how_it_fits[0]);

    const labels = env.doc.querySelectorAll('label.radio-option span').map(s => s.textContent);
    assert.ok(labels.includes(stubCopy.thinking.choices[0].label));
    assert.ok(labels.includes(stubCopy.thinking.choices[1].label));
    assert.ok(labels.includes(stubCopy.thinking.choices[2].label));
    assert.ok(labels.includes(stubCopy.thinking.later));

    const tRow = env.doc.getElementById('download-thinking');
    assert.ok(tRow.querySelector('.download-status').textContent.includes(stubCopy.downloads.progress));
    assert.strictEqual(env.doc.getElementById('download-thinking-cancel').textContent, stubCopy.downloads.cancel);
    assert.strictEqual(env.doc.getElementById('download-thinking-retry').textContent, stubCopy.downloads.retry);
    assert.strictEqual(env.doc.getElementById('download-speech-retry').textContent, stubCopy.downloads.retry);

    // Verify hygiene
    for (const req of env.allRequests) {
      assert.strictEqual(req.url.includes('/init/finalize'), false);
      assert.strictEqual(req.url.includes('/init/mark'), false);
      if (req.opts && req.opts.body) {
        const parsed = JSON.parse(req.opts.body);
        assert.strictEqual(parsed.name, undefined);
        assert.strictEqual(parsed.preferred, undefined);
        assert.strictEqual(parsed.retention_mode, undefined);
        assert.strictEqual(parsed.retention_days, undefined);
        assert.strictEqual(parsed.timezone, undefined);
      }
    }
  }

  // CASE 7: Downloads rendering, attempt rules, deadline abort
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    await settle();

    env.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: 'spp', saved: true },
      downloads: {
        thinking: { state: 'downloading', attempt_id: 'att-1', bytes_done: 10, bytes_total: 20 },
        speech: { state: 'downloading', attempt_id: 's-1', bytes_done: 5, bytes_total: null }
      }
    });
    await settle();
    const cap = env.nextPending();
    if (cap) { cap.resolve({ overall: 'ok', checks: [] }); await settle(); }

    const tStatus = env.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(tStatus.includes('10') && tStatus.includes('20'));

    const sStatus = env.doc.getElementById('download-speech').querySelector('.download-status').textContent;
    assert.ok(sStatus.includes('5'));
    assert.strictEqual(sStatus.includes(stubCopy.downloads.complete), false);

    // Boot with thinking saved: true and spp does not post save_thinking or start_thinking_download
    assert.strictEqual(env.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'save_thinking'), false);
    assert.strictEqual(env.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'start_thinking_download'), false);

    // Cancel posts displayed attempt_id
    env.doc.getElementById('download-thinking-cancel').click();
    await settle();
    const cancelReq = env.nextPending();
    assert.ok(cancelReq);
    assert.strictEqual(JSON.parse(cancelReq.opts.body).action, 'cancel_thinking_download');
    assert.strictEqual(JSON.parse(cancelReq.opts.body).attempt_id, 'att-1');
    cancelReq.resolve({
      ...defaultState(),
      downloads: { thinking: { state: 'cancelled', attempt_id: 'att-1' }, speech: { state: 'downloading', attempt_id: 's-1', bytes_done: 5, bytes_total: null } }
    });
    await settle();

    assert.ok(env.doc.getElementById('download-thinking').querySelector('.download-status').textContent.includes(stubCopy.downloads.cancelled));

    // Later downloading snapshot for att-1 does not unhide cancel
    env.advanceTime(1000);
    const pollSnap = env.nextPending();
    assert.ok(pollSnap);
    pollSnap.resolve({
      ...defaultState(),
      downloads: { thinking: { state: 'downloading', attempt_id: 'att-1', bytes_done: 15, bytes_total: 20 }, speech: { state: 'downloading', attempt_id: 's-1', bytes_done: 5, bytes_total: null } }
    });
    await settle();
    assert.strictEqual(env.doc.getElementById('download-thinking-cancel').hidden, true);

    // Speech error with speech_package_unavailable
    env.advanceTime(1000);
    const pollErr = env.nextPending();
    assert.ok(pollErr);
    pollErr.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'error', attempt_id: 'att-1' },
        speech: { state: 'error', error: { code: 'speech_package_unavailable', detail: 'stub-speech-detail' } }
      }
    });
    await settle();
    const sRow = env.doc.getElementById('download-speech');
    assert.strictEqual(sRow.querySelector('.download-status').textContent, 'stub-speech-detail');
    assert.strictEqual(env.doc.getElementById('download-speech-retry').hidden, true);
    assert.strictEqual(sRow.textContent.includes(stubCopy.downloads.error), false);
    assert.strictEqual(sRow.textContent.includes(stubCopy.downloads.speech), false);
    assert.strictEqual(env.doc.getElementById('download-thinking-retry').hidden, false);

    env.doc.getElementById('download-thinking-retry').click();
    await settle();
    const retryReq = env.nextPending();
    assert.ok(retryReq);
    assert.strictEqual(JSON.parse(retryReq.opts.body).action, 'start_thinking_download');
    retryReq.resolve({
      ...defaultState(),
      downloads: { thinking: { state: 'downloading', attempt_id: 'att-2', bytes_done: 0, bytes_total: 10 }, speech: { state: 'idle' } }
    });
    await settle();

    // Normal speech retry with package_unavailable
    env.advanceTime(1000);
    const pollSpeechErr = env.nextPending();
    assert.ok(pollSpeechErr);
    pollSpeechErr.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'idle' },
        speech: { state: 'error', error: { code: 'package_unavailable' } }
      }
    });
    await settle();
    assert.strictEqual(env.doc.getElementById('download-speech-retry').hidden, false);
    env.doc.getElementById('download-speech-retry').click();
    await settle();
    const speechRetryReq = env.nextPending();
    assert.ok(speechRetryReq);
    assert.strictEqual(JSON.parse(speechRetryReq.opts.body).action, 'retry_speech_download');
    speechRetryReq.resolve({
      ...defaultState(),
      downloads: { thinking: { state: 'idle' }, speech: { state: 'downloading' } }
    });
    await settle();

    // Equal bytes while downloading does not show complete
    env.advanceTime(1000);
    const pollEqual = env.nextPending();
    assert.ok(pollEqual);
    pollEqual.resolve({
      ...defaultState(),
      downloads: { thinking: { state: 'downloading', bytes_done: 10, bytes_total: 10 }, speech: { state: 'idle' } }
    });
    await settle();
    const statusText = env.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.strictEqual(statusText.includes(stubCopy.downloads.complete), false);

    // Deadline abort: 8000ms advances while GET /init/api/state is hung in flight
    env.advanceTime(1000);
    const hungPoll = env.nextPending();
    assert.ok(hungPoll && hungPoll.url === '/init/api/state');
    env.advanceTime(8000);
    await settle();

    assert.ok(env.doc.getElementById('thinking-error').textContent.length > 0);

    // Advancing 1000ms issues another poll (timer did not die)
    env.advanceTime(1000);
    const nextPoll = env.nextPending();
    assert.ok(nextPoll && nextPoll.url === '/init/api/state');

    // While unresolved, advancing 1000ms does NOT issue a second concurrent poll
    env.advanceTime(1000);
    const pendingCount = env.pendingRequests.filter(r => r.isPending && r.url === '/init/api/state').length;
    assert.strictEqual(pendingCount, 1);

    // Complete state renders complete
    nextPoll.resolve({
      ...defaultState(),
      downloads: { thinking: { state: 'complete', bytes_done: 10, bytes_total: 10 }, speech: { state: 'idle' } }
    });
    await settle();
    const statusTextComplete = env.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(statusTextComplete.includes(stubCopy.downloads.complete));

    // With save_thinking unresolved and thinking downloading, cancel posts displayed attempt
    const envSaveCancel = createPageEnvironment(markup, scriptCode);
    envSaveCancel.boot();
    await settle();
    envSaveCancel.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok' },
      downloads: { thinking: { state: 'downloading', attempt_id: 'att-save-cancel', bytes_done: 1, bytes_total: 10 }, speech: { state: 'idle' } }
    });
    await settle();
    const capSC = envSaveCancel.nextPending();
    if (capSC) { capSC.resolve({ overall: 'ok', checks: [] }); await settle(); }

    envSaveCancel.doc.getElementById('thinking-continue').click();
    await settle();
    const saveReqInFlight = envSaveCancel.nextPending();
    assert.ok(saveReqInFlight && JSON.parse(saveReqInFlight.opts.body).action === 'save_thinking');

    const cancelControl = envSaveCancel.doc.getElementById('download-thinking-cancel');
    assert.strictEqual(cancelControl.disabled, false);
    cancelControl.click();
    await settle();

    const cancelReqWhileSave = envSaveCancel.pendingRequests.find(r => r.isPending && JSON.parse(r.opts.body).action === 'cancel_thinking_download');
    assert.ok(cancelReqWhileSave);
    assert.strictEqual(JSON.parse(cancelReqWhileSave.opts.body).attempt_id, 'att-save-cancel');

    // Stale poll attempt in flight before B does not adopt and cancel posts displayed attempt
    {
      const envForeign = createPageEnvironment(markup, scriptCode);
      envForeign.boot();
      await settle();

      envForeign.nextPending().resolve({
        ...defaultState(),
        mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
        downloads: {
          thinking: { state: 'downloading', attempt_id: 'attempt-a', bytes_done: 1, bytes_total: 9 },
          speech: { state: 'idle' }
        }
      });
      await settle();

      const capF = envForeign.nextPending();
      if (capF) { capF.resolve({ overall: 'ok', checks: [] }); await settle(); }

      envForeign.advanceTime(1000);
      const pollStale = envForeign.nextPending('/init/api/state');
      assert.ok(pollStale);

      envForeign.doc.getElementById('download-thinking-cancel').click();
      await settle();
      const cancelReq = envForeign.nextPending('/init/api/action');
      cancelReq.resolve({
        ...defaultState(),
        downloads: {
          thinking: { state: 'cancelled', attempt_id: 'attempt-a' },
          speech: { state: 'idle' }
        }
      });
      await settle();

      envForeign.doc.getElementById('download-thinking-retry').click();
      await settle();
      const retryReq = envForeign.nextPending('/init/api/action');
      retryReq.resolve({
        ...defaultState(),
        downloads: {
          thinking: { state: 'downloading', attempt_id: 'attempt-b', bytes_done: 3, bytes_total: 8 },
          speech: { state: 'idle' }
        }
      });
      await settle();

      const tStatusF = envForeign.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
      assert.ok(tStatusF.includes('3') && tStatusF.includes('8'));

      pollStale.resolve({
        ...defaultState(),
        downloads: {
          thinking: { state: 'downloading', attempt_id: 'attempt-a', bytes_done: 1, bytes_total: 9 },
          speech: { state: 'idle' }
        }
      });
      await settle();

      const tStatusF2 = envForeign.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
      assert.ok(tStatusF2.includes('3') && tStatusF2.includes('8'));
      assert.strictEqual(tStatusF2.includes('9'), false);

      envForeign.doc.getElementById('download-thinking-cancel').click();
      await settle();

      const cancelReqF = envForeign.nextPending();
      assert.ok(cancelReqF && cancelReqF.url === '/init/api/action');
      const cancelPayload = JSON.parse(cancelReqF.opts.body);
      assert.strictEqual(cancelPayload.action, 'cancel_thinking_download');
      assert.strictEqual(cancelPayload.attempt_id, 'attempt-b');
    }
  }

  // CASE 8: Double-click thinking continue, finalize in flight, boot with saved local error/cancelled, save_thinking rejection
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    await settle();
    env.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: 'local', saved: true }
    });
    await settle();
    const cap = env.nextPending();
    if (cap) { cap.resolve({ overall: 'ok', checks: [] }); await settle(); }

    const tContinue = env.doc.getElementById('thinking-continue');
    tContinue.click();
    tContinue.click();
    await settle();

    const finReqs = env.allRequests.filter(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'finalize');
    assert.strictEqual(finReqs.length, 1);

    // Boot with saved local, error/cancelled: load posts neither save_thinking nor start_thinking_download
    const envErr = createPageEnvironment(markup, scriptCode);
    envErr.boot();
    await settle();
    envErr.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: 'local', saved: true },
      downloads: { thinking: { state: 'error', attempt_id: 'att-err' }, speech: { state: 'idle' } }
    });
    await settle();
    const capErr = envErr.nextPending();
    if (capErr) { capErr.resolve({ overall: 'ok', checks: [] }); await settle(); }

    assert.strictEqual(envErr.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'save_thinking'), false);
    assert.strictEqual(envErr.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'start_thinking_download'), false);

    envErr.doc.getElementById('thinking-continue').click();
    await settle();
    const finErr = envErr.nextPending();
    assert.ok(finErr && JSON.parse(finErr.opts.body).action === 'finalize');
    finErr.resolve({ ...defaultState(), done: true, redirect: '/app/home-done' });
    await settle();
    assert.strictEqual(envErr.window.location.href, '/app/home-done');

    // Save_thinking rejection stays on thinking
    const envRej = createPageEnvironment(markup, scriptCode);
    envRej.boot();
    await settle();
    envRej.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok' }
    });
    await settle();
    const capRej = envRej.nextPending();
    if (capRej) { capRej.resolve({ overall: 'ok', checks: [] }); await settle(); }

    envRej.doc.getElementById('thinking-continue').click();
    await settle();
    const saveRej = envRej.nextPending();
    saveRej.reject({ rawDetail: 'err-save-raw' });
    await settle();

    assert.strictEqual(envRej.doc.getElementById('setup-thinking').hidden, false);
    assert.strictEqual(envRej.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'finalize'), false);
  }

  // CASE 9: Thrown commit error focus and alert role, Enter on radio plus click in-flight guard
  {
    // Thrown commit: document.activeElement is #mark-error, role alert
    const envCommit = createPageEnvironment(markup, scriptCode);
    envCommit.boot();
    await settle();
    envCommit.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'proposed', proposal_id: 'P_err', render: makeMark() }
    });
    await settle();

    envCommit.doc.getElementById('mark-continue').click();
    await settle();
    const commitReq = envCommit.nextPending('/init/api/action');
    commitReq.reject({ rawDetail: 'err-commit-fail' });
    await settle();

    const markErr = envCommit.doc.getElementById('mark-error');
    assert.strictEqual(envCommit.doc.activeElement, markErr);
    assert.strictEqual(markErr.getAttribute('role'), 'alert');

    // On thinking, select byo, keydown Enter on that radio, and click continue before save resolves. Exactly one save_thinking with byo.
    const envThink = createPageEnvironment(markup, scriptCode);
    envThink.boot();
    await settle();
    envThink.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok' }
    });
    await settle();
    const cap = envThink.nextPending();
    if (cap) { cap.resolve({ overall: 'ok', checks: [] }); await settle(); }

    envThink.doc.getElementById('thinking-change').click();
    await settle();

    const byoRadio = envThink.doc.querySelectorAll('input[name="thinking-choice"]').find(r => r.value === 'byo');
    assert.ok(byoRadio);
    byoRadio.click();
    await settle();

    // keydown Enter on that radio
    byoRadio.keydown('Enter');
    // also click continue before save resolves
    envThink.doc.getElementById('thinking-continue').click();
    await settle();

    const saveByoCalls = envThink.allRequests.filter(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'save_thinking');
    assert.strictEqual(saveByoCalls.length, 1);
    assert.strictEqual(JSON.parse(saveByoCalls[0].opts.body).choice, 'byo');
  }

  // CASE 10: Admitted job discovery with idle and complete boot (Finding 2)
  {
    // Locked mark, capability ok, thinking download idle. Click thinking continue, hold save_thinking.
    const envIdle = createPageEnvironment(markup, scriptCode);
    envIdle.boot();
    await settle();
    envIdle.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok' },
      downloads: { thinking: { state: 'idle' }, speech: { state: 'idle' } }
    });
    await settle();
    const cap1 = envIdle.nextPending('/init/api/local-capability');
    if (cap1) { cap1.resolve({ overall: 'ok', checks: [] }); await settle(); }

    envIdle.doc.getElementById('thinking-continue').click();
    await settle();
    const saveReq = envIdle.nextPending('/init/api/action');
    assert.ok(saveReq && JSON.parse(saveReq.opts.body).action === 'save_thinking');

    // Advance 1000ms. Resolve that poll as downloading attempt-a bytes 4/40. Status includes 4 and 40.
    envIdle.advanceTime(1000);
    const poll1 = envIdle.nextPending('/init/api/state');
    assert.ok(poll1);
    poll1.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'downloading', attempt_id: 'attempt-a', bytes_done: 4, bytes_total: 40 },
        speech: { state: 'idle' }
      }
    });
    await settle();

    const tStatus1 = envIdle.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(tStatus1.includes('4') && tStatus1.includes('40'));

    // Cancel is visible and posts attempt-a
    const cancelBtn1 = envIdle.doc.getElementById('download-thinking-cancel');
    assert.strictEqual(cancelBtn1.hidden, false);
    cancelBtn1.click();
    await settle();

    const cancelReq1 = envIdle.pendingRequests.find(r => r.isPending && r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'cancel_thinking_download');
    assert.ok(cancelReq1 && JSON.parse(cancelReq1.opts.body).action === 'cancel_thinking_download');
    assert.strictEqual(JSON.parse(cancelReq1.opts.body).attempt_id, 'attempt-a');

    // Leave save unresolved and assert no finalize yet
    assert.strictEqual(envIdle.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'finalize'), false);

    saveReq.resolve({
      ...defaultState(),
      thinking: { choice: 'local', saved: true },
      downloads: { thinking: { state: 'idle' }, speech: { state: 'idle' } }
    });
    await settle();

    const tStatus1AfterSave = envIdle.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(tStatus1AfterSave.includes('4') && tStatus1AfterSave.includes('40'));

    // Repeat the discovery once with the boot download complete instead of idle
    const envComp = createPageEnvironment(markup, scriptCode);
    envComp.boot();
    await settle();
    envComp.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: null, saved: false, capability: 'ok' },
      downloads: { thinking: { state: 'complete', bytes_done: 10, bytes_total: 10 }, speech: { state: 'idle' } }
    });
    await settle();
    const cap2 = envComp.nextPending('/init/api/local-capability');
    if (cap2) { cap2.resolve({ overall: 'ok', checks: [] }); await settle(); }

    envComp.doc.getElementById('thinking-continue').click();
    await settle();
    const saveReq2 = envComp.nextPending('/init/api/action');
    assert.ok(saveReq2 && JSON.parse(saveReq2.opts.body).action === 'save_thinking');

    envComp.advanceTime(1000);
    const poll2 = envComp.nextPending('/init/api/state');
    assert.ok(poll2);
    poll2.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'downloading', attempt_id: 'attempt-a', bytes_done: 4, bytes_total: 40 },
        speech: { state: 'idle' }
      }
    });
    await settle();

    const tStatus2 = envComp.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(tStatus2.includes('4') && tStatus2.includes('40'));
    const cancelBtn2 = envComp.doc.getElementById('download-thinking-cancel');
    assert.strictEqual(cancelBtn2.hidden, false);
    cancelBtn2.click();
    await settle();

    const cancelReq2 = envComp.pendingRequests.find(r => r.isPending && r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'cancel_thinking_download');
    assert.ok(cancelReq2 && JSON.parse(cancelReq2.opts.body).action === 'cancel_thinking_download');
    assert.strictEqual(JSON.parse(cancelReq2.opts.body).attempt_id, 'attempt-a');
    assert.strictEqual(envComp.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'finalize'), false);

    saveReq2.resolve({
      ...defaultState(),
      thinking: { choice: 'local', saved: true },
      downloads: { thinking: { state: 'idle' }, speech: { state: 'idle' } }
    });
    await settle();

    const tStatus2AfterSave = envComp.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(tStatus2AfterSave.includes('4') && tStatus2AfterSave.includes('40'));
  }

  // CASE 11: Cancelled retry, pre-retry poll isolation, attempt-b and attempt-c (Finding 6 & 10)
  {
    // Page showing attempt-a downloading.
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    await settle();
    env.nextPending().resolve({
      ...defaultState(),
      mark: { state: 'locked', proposal_id: 'L', render: makeMark() },
      thinking: { choice: 'local', saved: true },
      downloads: {
        thinking: { state: 'downloading', attempt_id: 'attempt-a', bytes_done: 1, bytes_total: 10 },
        speech: { state: 'idle' }
      }
    });
    await settle();
    const cap = env.nextPending('/init/api/local-capability');
    if (cap) { cap.resolve({ overall: 'ok', checks: [] }); await settle(); }

    // Advance 1000ms so a state poll is issued while attempt-a is downloading (before retry)
    env.advanceTime(1000);
    const preRetryPoll = env.nextPending('/init/api/state');
    assert.ok(preRetryPoll);

    // Cancel, resolve cancelled
    env.doc.getElementById('download-thinking-cancel').click();
    await settle();
    const cancelReq = env.pendingRequests.find(r => r.isPending && r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'cancel_thinking_download');
    assert.ok(cancelReq && JSON.parse(cancelReq.opts.body).action === 'cancel_thinking_download');
    assert.strictEqual(JSON.parse(cancelReq.opts.body).attempt_id, 'attempt-a');
    cancelReq.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'cancelled', attempt_id: 'attempt-a' },
        speech: { state: 'idle' }
      }
    });
    await settle();

    // Retry is visible and was not requested on load
    const retryBtn = env.doc.getElementById('download-thinking-retry');
    assert.strictEqual(retryBtn.hidden, false);
    assert.strictEqual(env.allRequests.some(r => r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'start_thinking_download'), false);

    // Click retry, hold start_thinking_download
    retryBtn.click();
    await settle();
    const retryReq = env.pendingRequests.find(r => r.isPending && r.opts && r.opts.body && JSON.parse(r.opts.body).action === 'start_thinking_download');
    assert.ok(retryReq && JSON.parse(retryReq.opts.body).action === 'start_thinking_download');

    // Resolve the poll issued before that retry as A downloading again: row stays cancelled, cancel stays hidden
    preRetryPoll.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'downloading', attempt_id: 'attempt-a', bytes_done: 5, bytes_total: 10 },
        speech: { state: 'idle' }
      }
    });
    await settle();

    assert.ok(env.doc.getElementById('download-thinking').querySelector('.download-status').textContent.includes(stubCopy.downloads.cancelled));
    assert.strictEqual(env.doc.getElementById('download-thinking-cancel').hidden, true);

    // Resolve the retry as attempt-b 2/7. Status includes 2 and 7.
    retryReq.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'downloading', attempt_id: 'attempt-b', bytes_done: 2, bytes_total: 7 },
        speech: { state: 'idle' }
      }
    });
    await settle();

    const statusB = env.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(statusB.includes('2') && statusB.includes('7'));

    // A later body for A does not remove 2 or 7
    env.advanceTime(1000);
    const delayedPollA = env.nextPending('/init/api/state');
    assert.ok(delayedPollA);
    delayedPollA.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'downloading', attempt_id: 'attempt-a', bytes_done: 9, bytes_total: 10 },
        speech: { state: 'idle' }
      }
    });
    await settle();
    const statusBAfter = env.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(statusBAfter.includes('2') && statusBAfter.includes('7'));

    // A newer poll attempt-c 6/11 shows 6 and 11
    env.advanceTime(1000);
    const pollC = env.nextPending('/init/api/state');
    assert.ok(pollC);
    pollC.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'downloading', attempt_id: 'attempt-c', bytes_done: 6, bytes_total: 11 },
        speech: { state: 'idle' }
      }
    });
    await settle();

    const statusC = env.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(statusC.includes('6') && statusC.includes('11'));

    // A later stale A body does not restore A's bytes
    env.advanceTime(1000);
    const pollStaleA = env.nextPending('/init/api/state');
    assert.ok(pollStaleA);
    pollStaleA.resolve({
      ...defaultState(),
      downloads: {
        thinking: { state: 'downloading', attempt_id: 'attempt-a', bytes_done: 10, bytes_total: 10 },
        speech: { state: 'idle' }
      }
    });
    await settle();
    const statusCAfter = env.doc.getElementById('download-thinking').querySelector('.download-status').textContent;
    assert.ok(statusCAfter.includes('6') && statusCAfter.includes('11'));

    // Cancel posts attempt-c
    env.doc.getElementById('download-thinking-cancel').click();
    await settle();
    const cancelReqC = env.nextPending('/init/api/action');
    assert.ok(cancelReqC && JSON.parse(cancelReqC.opts.body).action === 'cancel_thinking_download');
    assert.strictEqual(JSON.parse(cancelReqC.opts.body).attempt_id, 'attempt-c');
  }

  // Malformed locked snapshots must leave the mark decision recoverable.
  for (const corrupt of [r => r.words = [null, 'beta'], r => r.words = [' ', 'beta'],
      r => r.icon1.svg = {}, r => r.icon1.svg = '<garbage/>', r => r.icon1.color.hex = 'invalid']) {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    const render = makeMark();
    corrupt(render);
    env.nextPending('/init/api/state').resolve({...defaultState(), mark: {state: 'locked', proposal_id: 'bad', render}});
    await settle();
    assert.strictEqual(env.doc.getElementById('setup-thinking').hidden, true);
    assert.strictEqual(env.doc.getElementById('mark-continue').disabled, true);
    assert.strictEqual(env.doc.getElementById('setup-retry').hidden, false);
    assert.strictEqual(env.doc.getElementById('mark-error').textContent, stubCopy.mark.error);
    assert.strictEqual(env.doc.getElementById('mark-try-another').hidden, true);
  }

  // Rejecting cancel keeps the real active attempt visible and cancellable.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    const state = {...defaultState(), mark: {state:'locked',proposal_id:'mark',render:makeMark()},
      downloads:{thinking:{state:'downloading',attempt_id:'cancel-rejected',bytes_done:2,bytes_total:11},speech:{state:'idle'}}};
    env.nextPending('/init/api/state').resolve(state);
    await settle();
    const cancel = env.doc.getElementById('download-thinking-cancel');
    cancel.click();
    await settle();
    env.nextPending('/init/api/action').reject(new Error('cancel refused'));
    await settle();
    env.advanceTime(1000);
    env.nextPending('/init/api/state').resolve({...state,downloads:{...state.downloads,
      thinking:{...state.downloads.thinking,bytes_done:8}}});
    await settle();
    assert.strictEqual(cancel.hidden,false);
    assert.strictEqual(cancel.disabled,false);
    assert.ok(env.doc.getElementById('download-thinking').querySelector('.download-status').textContent.includes('8 B / 11 B'));
    const bar = env.doc.getElementById('download-thinking').querySelector('.download-bar');
    assert.strictEqual(bar.hidden, false);
    assert.strictEqual(bar.value, 8);
    assert.strictEqual(bar.max, 11);
  }

  // A read returning idle during admission cannot obsolete its eventual action response.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    const initial = {...defaultState(), mark:{state:'locked',proposal_id:'mark',render:makeMark()}};
    env.nextPending('/init/api/state').resolve(initial);
    await settle();
    env.doc.getElementById('thinking-continue').click();
    await settle();
    const save = env.nextPending('/init/api/action');
    assert.strictEqual(JSON.parse(save.opts.body).action,'save_thinking');
    env.advanceTime(1000);
    env.nextPending('/init/api/state').resolve(initial);
    await settle();
    save.resolve({...initial,thinking:{...initial.thinking,saved:true,choice:'local'},
      downloads:{thinking:{state:'downloading',attempt_id:'eventual-admission',bytes_done:3,bytes_total:11},speech:{state:'idle'}}});
    await settle();
    env.nextPending('/init/api/action').reject(new Error('finalize refused'));
    await settle();
    env.advanceTime(1000);
    const reconciliation = env.nextPending('/init/api/state');
    assert.ok(reconciliation);
    reconciliation.resolve({...initial,downloads:{thinking:{state:'downloading',attempt_id:'eventual-admission',bytes_done:3,bytes_total:11},speech:{state:'idle'}}});
    await settle();
    assert.strictEqual(env.doc.getElementById('download-thinking').hidden,false);
    assert.ok(env.doc.getElementById('download-thinking').querySelector('.download-status').textContent.includes('3 B / 11 B'));
    assert.strictEqual(env.doc.getElementById('download-thinking-cancel').hidden,false);
  }

  // Completed setup resumes at the authoritative landing route.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    env.nextPending('/init/api/state').resolve({...defaultState(),done:true,redirect:'/app/home'});
    await settle();
    assert.strictEqual(env.window.location.href,'/app/home');
    assert.strictEqual(env.allRequests.filter(r => r.url === '/init/api/action').length,0);
  }
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    env.nextPending('/init/api/state').resolve({...defaultState(),mark:{state:'proposed',proposal_id:'mark',render:makeMark()}});
    await settle();
    assert.strictEqual(env.doc.getElementById('welcome-beats').children.length,stubCopy.welcome.how_it_fits.length);
    stubCopy.welcome.how_it_fits.forEach((beat,i) => assert.strictEqual(env.doc.getElementById('welcome-beats').children[i].textContent,beat));
  }
  // A delayed retry B cannot retire another client's C, even if B was never displayed.
  for (const task of ['thinking', 'speech']) {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    const initial = {...defaultState(),mark:{state:'locked',proposal_id:'mark',render:makeMark()},
      thinking:{saved:true,choice:'local',capability:'ok'},
      downloads:{thinking:{state:'downloading',attempt_id:'other-t',bytes_done:1,bytes_total:20},
        speech:{state:'downloading',attempt_id:'other-s',bytes_done:1,bytes_total:20}}};
    initial.downloads[task] = {state:'error',attempt_id:'a',error:{code:'failed'}};
    env.nextPending('/init/api/state').resolve(initial);
    await settle();
    env.doc.getElementById(`download-${task}-retry`).click();
    await settle();
    const retry = env.nextPending('/init/api/action');
    env.advanceTime(1000);
    const current = {...initial,downloads:{...initial.downloads,
      [task]:{state:'downloading',attempt_id:'c',bytes_done:8,bytes_total:20}}};
    env.nextPending('/init/api/state').resolve(current);
    await settle();
    retry.resolve({...initial,downloads:{...initial.downloads,
      [task]:{state:'downloading',attempt_id:'b',bytes_done:2,bytes_total:20}}});
    await settle();
    assert.ok(env.doc.getElementById(`download-${task}`).querySelector('.download-status').textContent.includes('8 B / 20 B'));
    env.advanceTime(1000);
    const fresh = env.nextPending('/init/api/state');
    assert.ok(fresh);
    fresh.resolve({...current,downloads:{...current.downloads,
      [task]:{...current.downloads[task],bytes_done:12}}});
    await settle();
    assert.ok(env.doc.getElementById(`download-${task}`).querySelector('.download-status').textContent.includes('12 B / 20 B'));
    if (task === 'thinking') {
      env.doc.getElementById('download-thinking-cancel').click();
      await settle();
      assert.strictEqual(JSON.parse(env.nextPending('/init/api/action').opts.body).attempt_id,'c');
    }
  }
  // An edited owner draft survives admission, while its side-effect download stays visible.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    const initial = {...defaultState(),mark:{state:'locked',proposal_id:'mark',render:makeMark()}};
    env.nextPending('/init/api/state').resolve(initial);
    await settle();
    env.doc.getElementById('thinking-continue').click();
    await settle();
    const save = env.nextPending('/init/api/action');
    env.doc.getElementById('thinking-change').click();
    env.doc.querySelectorAll('input[name="thinking-choice"]').find(r => r.value === 'byo').click();
    save.resolve({...initial,thinking:{saved:true,choice:'local',capability:'ok'},
      downloads:{thinking:{state:'downloading',attempt_id:'admitted-before-edit',bytes_done:4,bytes_total:11},speech:{state:'idle'}}});
    await settle();
    assert.strictEqual(env.doc.querySelectorAll('input[name="thinking-choice"]').find(r => r.checked).value,'byo');
    assert.strictEqual(env.doc.getElementById('download-thinking').hidden,false);
    assert.ok(env.doc.getElementById('download-thinking').querySelector('.download-status').textContent.includes('4 B / 11 B'));
    assert.strictEqual(env.allRequests.filter(r => r.opts.body && JSON.parse(r.opts.body).action === 'finalize').length,0);
    env.doc.getElementById('download-thinking-cancel').click();
    await settle();
    assert.strictEqual(JSON.parse(env.nextPending('/init/api/action').opts.body).attempt_id,'admitted-before-edit');
  }
  // Speech admission timeout is uncertain; refresh can discover the still-running journal job.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    const initial = {...defaultState(),mark:{state:'locked',proposal_id:'mark',render:makeMark()},
      downloads:{thinking:{state:'idle'},speech:{state:'error',attempt_id:'old',error:{code:'failed'}}}};
    env.nextPending('/init/api/state').resolve(initial);
    await settle();
    env.doc.getElementById('download-speech-retry').click();
    await settle();
    env.nextPending('/init/api/action').reject({reasonCode:'speech_admission_pending',rawDetail:'pending'});
    await settle();
    env.advanceTime(1000);
    const fresh = env.nextPending('/init/api/state');
    assert.ok(fresh);
    fresh.resolve({...initial,downloads:{thinking:{state:'idle'},speech:{state:'downloading',attempt_id:'eventual-speech',bytes_done:6,bytes_total:11}}});
    await settle();
    assert.ok(env.doc.getElementById('download-speech').querySelector('.download-status').textContent.includes('6 B / 11 B'));
    assert.strictEqual(env.doc.getElementById('download-speech-retry').hidden,true);
  }
  // Proposal mutation exclusion must not hold the independent download read guard.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    env.nextPending('/init/api/state').resolve({...defaultState(),
      downloads:{thinking:{state:'idle'},speech:{state:'downloading',attempt_id:'speech',bytes_done:1,bytes_total:11}}});
    await settle();
    const proposal = env.nextPending('/init/api/action');
    assert.strictEqual(JSON.parse(proposal.opts.body).action,'regenerate_mark');
    env.advanceTime(1000);
    const progress = env.nextPending('/init/api/state');
    assert.ok(progress);
    progress.resolve({...defaultState(),downloads:{thinking:{state:'idle'},speech:{state:'downloading',attempt_id:'speech',bytes_done:7,bytes_total:11}}});
    await settle();
    assert.ok(env.doc.getElementById('download-speech').querySelector('.download-status').textContent.includes('7 B / 11 B'));
    assert.strictEqual(proposal.isPending,true);
  }
  // Every glyph the journal can propose must remain usable with strict render validation.
  for (const svg of Object.values(glyphs)) {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    const render = makeMark();
    render.icon1.svg = svg;
    env.nextPending('/init/api/state').resolve({...defaultState(),mark:{state:'proposed',proposal_id:'catalogue',render}});
    await settle();
    assert.strictEqual(env.doc.getElementById('mark-continue').disabled,false);
  }
  // The first journal-owned speech job can be admitted after the initial idle read.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    const initial = {...defaultState(),mark:{state:'proposed',proposal_id:'mark',render:makeMark()}};
    env.nextPending('/init/api/state').resolve(initial);
    await settle();
    env.advanceTime(1000);
    const admitted = env.nextPending('/init/api/state');
    assert.ok(admitted);
    admitted.resolve({...initial,downloads:{thinking:{state:'idle'},speech:{state:'downloading',attempt_id:'automatic-speech',bytes_done:8,bytes_total:11}}});
    await settle();
    assert.strictEqual(env.doc.getElementById('download-speech').hidden,false);
    assert.ok(env.doc.getElementById('download-speech').querySelector('.download-status').textContent.includes('8 B / 11 B'));
    assert.strictEqual(env.doc.getElementById('setup-mark').hidden,false);
  }
  // Mark mutations preserve newer job observations rather than replacing downloads wholesale.
  for (const action of ['regenerate_mark','commit_mark']) {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    const initial = {...defaultState(),mark:{state:'proposed',proposal_id:'x',render:makeMark()},
      downloads:{thinking:{state:'downloading',attempt_id:'a',bytes_done:1,bytes_total:20},speech:{state:'complete'}}};
    env.nextPending('/init/api/state').resolve(initial);
    await settle();
    env.doc.getElementById(action === 'regenerate_mark' ? 'mark-try-another' : 'mark-continue').click();
    await settle();
    const mutation = env.nextPending('/init/api/action');
    assert.strictEqual(JSON.parse(mutation.opts.body).action,action);
    env.advanceTime(1000);
    const current = {...initial,downloads:{...initial.downloads,thinking:{state:'downloading',attempt_id:'c',bytes_done:8,bytes_total:20}}};
    env.nextPending('/init/api/state').resolve(current);
    await settle();
    mutation.resolve({...initial,mark:{state:action === 'commit_mark' ? 'locked' : 'proposed',proposal_id:action === 'commit_mark' ? 'x' : 'y',render:makeMark()},
      downloads:{...initial.downloads,thinking:{state:'downloading',attempt_id:'b',bytes_done:2,bytes_total:20}}});
    await settle();
    assert.ok(env.doc.getElementById('download-thinking').querySelector('.download-status').textContent.includes('8 B / 20 B'));
    env.advanceTime(1000);
    env.nextPending('/init/api/state').resolve({...current,downloads:{...current.downloads,thinking:{...current.downloads.thinking,bytes_done:12}}});
    await settle();
    assert.ok(env.doc.getElementById('download-thinking').querySelector('.download-status').textContent.includes('12 B / 20 B'));
    env.doc.getElementById('download-thinking-cancel').click();
    await settle();
    assert.strictEqual(JSON.parse(env.nextPending('/init/api/action').opts.body).attempt_id,'c');
  }
  // The conflict explanation stays visible after a refreshed proposal is offered for explicit Continue.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    env.nextPending('/init/api/state').resolve({...defaultState(),mark:{state:'proposed',proposal_id:'x',render:makeMark()}});
    await settle();
    env.doc.getElementById('mark-continue').click();
    await settle();
    env.nextPending('/init/api/action').reject({reasonCode:'proposal_conflict',rawDetail:'review the changed mark'});
    await settle();
    env.nextPending('/init/api/state').resolve({...defaultState(),mark:{state:'proposed',proposal_id:'y',render:makeMark(['gamma','delta'])}});
    await settle();
    assert.strictEqual(env.doc.getElementById('mark-error').hidden,false);
    assert.strictEqual(env.doc.getElementById('mark-error').textContent,'review the changed mark');
    assert.strictEqual(env.doc.getElementById('mark-continue').disabled,false);
    assert.strictEqual(env.doc.getElementById('setup-thinking').hidden,true);
    assert.strictEqual(env.allRequests.filter(r => r.opts.body && JSON.parse(r.opts.body).action === 'commit_mark').length,1);
  }
  // Initial offer preparation owns the same exclusion boundary as later mark mutations.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot();
    assert.strictEqual(env.doc.getElementById('mark-try-another').disabled,true);
    env.doc.getElementById('mark-try-another').click();
    await settle();
    assert.strictEqual(env.allRequests.filter(r => r.url === '/init/api/action').length,0);
    env.nextPending('/init/api/state').resolve(defaultState());
    await settle();
    assert.strictEqual(env.allRequests.filter(r => r.url === '/init/api/action').length,1);
    env.doc.getElementById('mark-try-another').click();
    await settle();
    assert.strictEqual(env.allRequests.filter(r => r.url === '/init/api/action').length,1);
    env.nextPending('/init/api/action').reject(new Error('proposal unavailable'));
    await settle();
    assert.strictEqual(env.doc.getElementById('setup-retry').hidden,false);
    assert.strictEqual(env.doc.getElementById('setup-retry').disabled,false);
    assert.strictEqual(env.doc.getElementById('mark-try-another').disabled,true);
    env.doc.getElementById('setup-retry').click();
    await settle();
    assert.ok(env.nextPending('/init/api/state'));
  }
  // An incomplete restore never invokes setup mutations, even with a completed config.
  {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot(); await settle();
    const pending = {recovery: 'pending', done: false, journal_ready: false, in_progress: false,
      source: {kind: 'byo', backend: 's3', repository: 's3:synthetic/retained'}, snapshot_id: 'a'.repeat(64), subtree: '/journal'};
    env.nextPending('/init/api/state').resolve(pending); await settle();
    assert.strictEqual(env.doc.getElementById('setup-mark').hidden, true);
    assert.strictEqual(env.doc.getElementById('setup-thinking').hidden, true);
    assert.strictEqual(env.doc.getElementById('setup-recovery').hidden, false);
    assert.strictEqual(env.doc.getElementById('recovery-form').hidden, false);
    assert.strictEqual(env.allRequests.some(r => r.url === '/init/api/action'), false);
    env.doc.getElementById('recovery-key').value = 'synthetic recovery key';
    env.doc.getElementById('recovery-access').value = 'synthetic access';
    env.doc.getElementById('recovery-secret').value = 'synthetic secret';
    env.doc.getElementById('recovery-continue').click(); await settle();
    const retry = env.nextPending('/init/api/recovery/retry');
    assert.ok(retry);
    const body = JSON.parse(retry.opts.body);
    assert.strictEqual(body.snapshot_id, pending.snapshot_id);
    assert.strictEqual(body.subtree, pending.subtree);
    assert.strictEqual(body.repository, pending.source.repository);
    assert.strictEqual(body.credentials.secret_access_key, 'synthetic secret');
    assert.strictEqual(env.doc.getElementById('recovery-key').value, '');
    assert.strictEqual(env.doc.getElementById('recovery-secret').value, '');
    env.doc.getElementById('recovery-continue').click(); await settle();
    assert.strictEqual(env.allRequests.filter(r => r.url === '/init/api/recovery/retry').length, 1);
    env.advanceTime(1000); await settle();
    env.nextPending('/init/api/state').resolve({...pending, in_progress: true}); await settle();
    assert.strictEqual(env.doc.getElementById('recovery-form').hidden, true);
    assert.strictEqual(env.doc.getElementById('recovery-stop').hidden, false);
    assert.strictEqual(env.window.location.href, '/init');
    env.doc.getElementById('recovery-stop').click(); await settle();
    const stop = env.nextPending('/init/api/recovery/stop'); assert.ok(stop);
    stop.resolve({stopped: true}); await settle();
    env.nextPending('/init/api/state').resolve({...pending, in_progress: true}); await settle();
    assert.strictEqual(env.doc.getElementById('recovery-form').hidden, true, 'a Stop response does not invent readiness');
    retry.reject({serverMessage: 'the restore did not complete'}); await settle();
    const refresh = env.nextPending('/init/api/state');
    assert.ok(refresh); refresh.resolve(pending); await settle();
    assert.strictEqual(env.doc.getElementById('recovery-form').hidden, false);
    assert.strictEqual(env.window.location.href, '/init');
    env.advanceTime(1000); await settle();
    env.nextPending('/init/api/state').resolve({...defaultState(), done: true, redirect: '/app/home'}); await settle();
    assert.strictEqual(env.window.location.href, '/app/home');
  }
  // Unknown records and unknown operation state offer a read, never a speculative restore.
  for (const recovery of ['unavailable', 'waiting', 'pending']) {
    const env = createPageEnvironment(markup, scriptCode);
    env.boot(); await settle();
    env.nextPending('/init/api/state').resolve({recovery, done: false, journal_ready: false, in_progress: null}); await settle();
    assert.strictEqual(env.doc.getElementById('recovery-form').hidden, true);
    assert.strictEqual(env.doc.getElementById('recovery-stop').hidden, true);
    assert.strictEqual(env.doc.getElementById('recovery-check').hidden, false);
    assert.strictEqual(env.allRequests.some(r => r.url === '/init/api/action'), false);
    env.doc.getElementById('recovery-check').click(); await settle();
    assert.ok(env.nextPending('/init/api/state'));
    assert.strictEqual(env.allRequests.some(r => r.url === '/init/api/recovery/retry'), false);
  }
  console.log(`DOM ASSERTIONS: ${passedCases}/${executedCases} passed`);
}

main().catch(err => {
  console.error(err);
  process.exit(1);
});

async function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) {
    console.error('Usage: node init_dom.js <crate-manifest-dir>');
    process.exit(1);
  }
  await runTests(manifestDir);
}
