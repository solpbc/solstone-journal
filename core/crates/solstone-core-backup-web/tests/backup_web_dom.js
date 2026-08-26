// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');

class ClassList {
  constructor(element) {
    this.element = element;
    this.values = new Set();
  }

  setFromString(value) {
    this.values = new Set(String(value || '').split(/\s+/).filter(Boolean));
  }

  sync() {
    if (this.values.size) this.element.attributes.class = Array.from(this.values).join(' ');
    else delete this.element.attributes.class;
  }

  add(...names) {
    names.forEach((name) => this.values.add(name));
    this.sync();
  }

  remove(...names) {
    names.forEach((name) => this.values.delete(name));
    this.sync();
  }

  toggle(name, force) {
    const present = force === undefined ? !this.values.has(name) : force;
    if (present) this.values.add(name);
    else this.values.delete(name);
    this.sync();
    return present;
  }

  contains(name) {
    return this.values.has(name);
  }
}

function dataKey(attribute) {
  return attribute.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function selectorMatches(element, selector) {
  const attributes = [...selector.matchAll(/\[([^\]=]+)(?:=["']?([^\]"']+)["']?)?\]/g)];
  const simple = selector.replace(/\[[^\]]+\]/g, '');
  const tag = simple.match(/^[a-zA-Z][\w-]*/);
  const id = simple.match(/#([\w-]+)/);
  const classes = [...simple.matchAll(/\.([\w-]+)/g)];
  if (tag && element.tagName !== tag[0].toUpperCase()) return false;
  if (id && element.id !== id[1]) return false;
  if (classes.some((match) => !element.classList.contains(match[1]))) return false;
  return attributes.every((match) => {
    if (!element.hasAttribute(match[1])) return false;
    return match[2] === undefined || element.getAttribute(match[1]) === match[2];
  });
}

function matchesSelector(element, selector) {
  return selector.split(',').some((part) => selectorMatches(element, part.trim()));
}

function queryAll(root, selector) {
  const pieces = selector.trim().split(/\s+/);
  let candidates = [root];
  for (const piece of pieces) {
    const next = [];
    for (const candidate of candidates) {
      const visit = (element) => {
        for (const child of element.children) {
          if (matchesSelector(child, piece)) next.push(child);
          visit(child);
        }
      };
      visit(candidate);
    }
    candidates = next;
  }
  return candidates;
}

class Element {
  constructor(tagName, ownerDocument) {
    this.tagName = tagName.toUpperCase();
    this.ownerDocument = ownerDocument;
    this.attributes = {};
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.dataset = {};
    this.classList = new ClassList(this);
    this.style = {};
    this.value = '';
    this.disabled = false;
    this._textContent = '';
  }

  get id() { return this.getAttribute('id') || ''; }
  set id(value) { this.setAttribute('id', value); }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(value) { if (value) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  get textContent() { return this._textContent; }
  set textContent(value) { this._textContent = String(value); }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'class') this.classList.setFromString(value);
    if (name.startsWith('data-')) this.dataset[dataKey(name)] = String(value);
  }

  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  hasAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attributes, name); }

  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'class') this.classList.setFromString('');
    if (name.startsWith('data-')) delete this.dataset[dataKey(name)];
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  append(...children) { children.forEach((child) => this.appendChild(child)); }

  replaceChildren(...children) {
    this.children.forEach((child) => { child.parentElement = null; });
    this.children = [];
    this.append(...children);
  }

  querySelector(selector) { return queryAll(this, selector)[0] || null; }
  querySelectorAll(selector) { return queryAll(this, selector); }
  matches(selector) { return matchesSelector(this, selector); }

  closest(selector) {
    let current = this;
    while (current) {
      if (current.matches(selector)) return current;
      current = current.parentElement;
    }
    return null;
  }

  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }

  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    if (event.bubbles && !event.cancelBubble && this.parentElement) this.parentElement.dispatchEvent(event);
    return !event.defaultPrevented;
  }

  focus() { this.ownerDocument.activeElement = this; }
}

class Document {
  constructor() {
    this.readyState = 'complete';
    this.documentElement = new Element('html', this);
    this.body = new Element('body', this);
    this.documentElement.appendChild(this.body);
    this.activeElement = this.body;
    this.listeners = {};
  }

  createElement(tagName) { return new Element(tagName, this); }
  querySelector(selector) {
    if (selector === 'body') return this.body;
    return this.documentElement.querySelector(selector);
  }
  querySelectorAll(selector) { return this.documentElement.querySelectorAll(selector); }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
}

function event(type, options = {}) {
  return {
    type,
    bubbles: Boolean(options.bubbles),
    key: options.key,
    target: options.target,
    defaultPrevented: false,
    cancelBubble: false,
    preventDefault() { this.defaultPrevented = true; },
    stopPropagation() { this.cancelBubble = true; },
  };
}

function add(parent, tagName, attributes = {}) {
  const element = parent.ownerDocument.createElement(tagName);
  for (const [name, value] of Object.entries(attributes)) element.setAttribute(name, value);
  parent.appendChild(element);
  return element;
}

function status(operation = null, hosted = { bound: false }) {
  return { success: true, enabled: false, mode: 'byo', hosted, operation };
}

function response(body, statusCode = 200) {
  return {
    ok: statusCode >= 200 && statusCode < 300,
    status: statusCode,
    json() { return Promise.resolve(body); },
  };
}

function restoreOperation(phase, reasonCode) {
  return {
    kind: 'restore_hosted',
    phase,
    reason_code: reasonCode || null,
    elapsed_ms: 0,
  };
}

function fixture(document) {
  const root = add(document.body, 'section', { class: 'backup-shell', 'data-backup-root': '', 'data-state': 'empty' });
  const banner = add(root, 'div', { 'data-operation-banner': '', hidden: '' });
  add(banner, 'span', { 'data-operation-phase': '' });
  add(banner, 'span', { 'data-operation-error': '' });

  const panels = add(root, 'div', { class: 'backup-panels' });
  add(panels, 'article', { 'data-backup-panel': 'intro' });
  add(panels, 'article', { 'data-backup-panel': 'management', hidden: '' });
  const destination = add(panels, 'article', { 'data-backup-panel': 'destination', hidden: '' });
  const destinationByo = add(destination, 'button', { class: 'backup-mode is-selected', 'data-mode': 'byo', role: 'radio', 'aria-checked': 'true' });
  destinationByo.textContent = 'your own';
  const destinationHosted = add(destination, 'button', { class: 'backup-mode', 'data-mode': 'hosted', role: 'radio', 'aria-checked': 'false' });
  destinationHosted.textContent = 'operated';
  add(destination, 'div', { 'data-mode-panel': 'byo' });
  add(destination, 'div', { 'data-mode-panel': 'hosted', hidden: '' });

  const restore = add(panels, 'article', { 'data-backup-panel': 'restore', hidden: '' });
  add(restore, 'h2', { 'data-copy': 'action_labels.restore' });
  add(restore, 'p', { 'data-copy': 'restore.hosted.choose_lane' });
  const lanes = add(restore, 'div', { class: 'backup-restore-lanes', role: 'radiogroup' });
  const byoLane = add(lanes, 'button', { class: 'backup-restore-lane', 'data-restore-lane': 'byo', role: 'radio', 'aria-checked': 'false', tabindex: '0' });
  byoLane.textContent = 'your own';
  const operatedLane = add(lanes, 'button', { class: 'backup-restore-lane', 'data-restore-lane': 'operated', role: 'radio', 'aria-checked': 'false', tabindex: '-1' });
  operatedLane.textContent = 'operated';
  const operated = add(restore, 'div', { 'data-restore-lane-panel': 'operated', hidden: '' });
  const heading = add(operated, 'h4', { id: 'backup-restore-hosted-heading', 'data-copy': 'restore.hosted.lane_title' });
  const hint = add(operated, 'p', { 'data-hosted-restore-hint': '', 'data-copy': 'restore.hosted.lane_intro' });
  hint.textContent = '';
  const keyControl = add(operated, 'label', { 'data-hosted-restore-key-control': '' });
  const key = add(keyControl, 'textarea', { 'data-restore-hosted-input': '', 'aria-describedby': heading.id });
  const keyReassurance = add(operated, 'p', { 'data-hosted-restore-key-reassurance': '' });
  const outcome = add(operated, 'p', { id: 'backup-restore-hosted-outcome', 'data-hosted-restore-outcome': '', role: 'status', 'aria-live': 'polite', 'aria-atomic': 'true', hidden: '' });
  outcome.textContent = '';
  const primary = add(operated, 'button', { class: 'primary', 'data-action': 'restore-hosted-unbound-start', disabled: '' });
  primary.disabled = true;
  const attemptCancel = add(operated, 'button', { 'data-action': 'cancel-hosted-restore-attempt', hidden: '' });
  const byo = add(restore, 'div', { 'data-restore-lane-panel': 'byo', hidden: '' });
  add(byo, 'form', { 'data-restore-form': '' });
  const panelCancel = add(restore, 'button', { 'data-action': 'cancel-restore' });
  panelCancel.textContent = 'cancel';
  return { root, restore, destinationByo, destinationHosted, byoLane, operatedLane, operated, byo, heading, keyControl, key, keyReassurance, outcome, primary, attemptCancel, panelCancel, banner };
}

function createHarness(options = {}) {
  const document = new Document();
  const elements = fixture(document);
  const calls = [];
  const timers = [];
  const popups = [];
  let nextTimer = 1;
  const statusQueue = (options.statusQueue || []).slice();
  const popupFactory = options.popupFactory || (() => ({
    closed: false,
    opener: {},
    location: { replaced: null, replace(value) { this.replaced = value; } },
    close() { this.closed = true; },
  }));
  const fetch = (url, request = {}) => {
    const call = { url: String(url), request };
    calls.push(call);
    let result;
    if (options.respond) {
      result = options.respond(call, calls, statusQueue);
    }
    if (result === undefined) {
      if (call.url === '/app/backup/status') result = response(statusQueue.shift() || status());
      else if (call.url === '/app/backup/offload/status') result = response({ success: true, offload: {}, days: [] });
      else throw new Error('unexpected fetch ' + call.url);
    }
    return Promise.resolve(result);
  };
  const window = {
    document,
    console,
    fetch,
    URL,
    open(...args) {
      const popup = popupFactory(...args);
      if (popup) popups.push(popup);
      return popup;
    },
    setTimeout(callback, delay) {
      const timer = { id: nextTimer++, callback, delay, cleared: false };
      timers.push(timer);
      return timer.id;
    },
    clearTimeout(id) {
      const timer = timers.find((candidate) => candidate.id === id);
      if (timer) timer.cleared = true;
    },
  };
  window.window = window;
  const context = vm.createContext({ window, document, console, fetch, URL, navigator: {}, setTimeout: window.setTimeout, clearTimeout: window.clearTimeout });
  const source = fs.readFileSync(path.join(crateDir, 'assets', 'backup.js'), 'utf8');
  vm.runInContext(source, context, { filename: 'backup.js' });
  return {
    ...elements,
    calls,
    timers,
    popups,
    window,
    clearCalls() { calls.splice(0); },
    async runTimer(delay) {
      const timer = timers.find((candidate) => !candidate.cleared && candidate.delay === delay);
      assert.ok(timer, 'timer ' + delay + ' is scheduled after ' + calls.map((call) => call.url).join(', '));
      timer.cleared = true;
      await timer.callback();
    },
  };
}

async function settle(rounds = 16) {
  for (let index = 0; index < rounds; index += 1) await Promise.resolve();
}

async function ready(harness) {
  await settle();
  harness.clearCalls();
}

function click(element) {
  element.dispatchEvent(event('click', { bubbles: true }));
}

function setKey(harness, value) {
  harness.key.value = value;
  harness.key.dispatchEvent(event('input'));
}

function selectOperated(harness) {
  click(harness.operatedLane);
}

function hostedSequence(statusPayload) {
  let statusRequests = 0;
  return (call) => {
    if (call.url === '/app/backup/restore-hosted/prepare') return response({ capability: 'capability' });
    if (call.url === '/app/backup/restore-hosted/key') return response({ portal_url: 'https://services.solstone.app/enable/backup?nonce=nonce&intent=restore' });
    if (call.url === '/app/backup/restore-hosted/arm') return response(status(restoreOperation('restoring')));
    if (call.url === '/app/backup/restore-hosted/activate') return response(status(restoreOperation('restoring')));
    if (call.url === '/app/backup/status') {
      statusRequests += 1;
      return response(statusRequests === 1 ? status() : statusPayload || status(restoreOperation('restoring')));
    }
    if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
    throw new Error('unexpected fetch ' + call.url);
  };
}

async function startToPolling(harness) {
  assert.ok(!harness.primary.disabled, 'hosted primary is enabled before starting');
  click(harness.primary);
  await settle();
  await harness.runTimer(0);
  await settle();
}

let cases = 0;
function testCase(name, fn) {
  try {
    fn();
    cases += 1;
  } catch (error) {
    error.message = name + ': ' + error.message;
    throw error;
  }
}

const asyncCases = [];
function asyncCase(name, fn) {
  asyncCases.push(async () => {
    try {
      await fn();
      cases += 1;
    } catch (error) {
      error.message = name + ': ' + error.message;
      throw error;
    }
  });
}

testCase('workspace has only the new restore lane contract', () => {
  const workspace = fs.readFileSync(path.join(crateDir, 'assets', 'workspace.html'), 'utf8');
  const restoreStart = workspace.indexOf('data-backup-panel="restore"');
  const restoreSection = workspace.slice(restoreStart, workspace.indexOf('</article>', restoreStart));
  const operatedStart = workspace.indexOf('data-restore-lane-panel="operated"');
  const operatedSection = workspace.slice(operatedStart, workspace.indexOf('data-restore-lane-panel="byo"', operatedStart));
  const byoStart = workspace.indexOf('data-restore-lane-panel="byo"');
  const byoSection = workspace.slice(byoStart, workspace.indexOf('</form>', byoStart));
  assert.ok(workspace.includes('data-restore-lane="byo"'));
  assert.ok(workspace.includes('data-restore-lane-panel="operated"'));
  assert.ok(workspace.includes('data-action="restore-hosted-unbound-start"'));
  assert.ok(!workspace.includes('data-action="restore-hosted"'));
  assert.ok(!workspace.includes('hosted.restore_hint'));
  assert.ok(workspace.includes('data-copy="restore.hosted.byo_desc"'));
  assert.ok(workspace.includes('data-copy="restore.hosted.operated_desc"'));
  assert.ok(workspace.includes('data-copy="restore.hosted.key_label"'));
  assert.ok(workspace.includes('data-copy="restore.hosted.key_reassurance"'));
  assert.ok(workspace.includes('data-copy="restore.hosted.primary"'));
  assert.ok(workspace.includes('id="backup-restore-source-question"'));
  assert.ok(workspace.includes('aria-labelledby="backup-restore-source-question"'));
  assert.ok(workspace.includes('data-hosted-restore-key-reassurance'));
  assert.ok(!restoreSection.includes('data-copy="destination.modes.byo.desc"'));
  assert.ok(!restoreSection.includes('data-copy="destination.modes.hosted.desc"'));
  assert.ok(!operatedSection.includes('data-copy="confirm.prompt"'));
  assert.ok(!operatedSection.includes('data-copy="action_labels.restore"'));
  assert.ok(byoSection.includes('data-copy="restore.hosted.key_label"'));
  assert.ok(!byoSection.includes('data-copy="confirm.prompt"'));
});

testCase('operated restore copy stays on its governed keys', () => {
  const source = fs.readFileSync(path.join(crateDir, 'assets', 'backup.js'), 'utf8');
  for (const literal of [
    "where is the encrypted copy you're restoring from?",
    'storage you bring yourself, reached with credentials you provide.',
    'storage sol pbc runs, reached from your services.',
    'enter your recovery key, then sign in to your services and confirm the restore.',
    'this journal uses your key and never sends it to sol pbc.',
    'sign in to restore →',
    "sol pbc isn't holding an encrypted copy for the sign-in you used.",
    'sol pbc deleted that copy once 30 days had passed since encrypted backup stopped.',
    "that recovery key didn't unlock the backup. check the key, then try signing in again.",
    "the sign-in window didn't open. try again, and check whether your browser blocked it.",
  ]) assert.ok(source.includes(JSON.stringify(literal)));
});

asyncCase('restore lanes start unselected, stay isolated, and use roving tabindex', async () => {
  const harness = createHarness();
  await ready(harness);
  assert.strictEqual(harness.byoLane.getAttribute('aria-checked'), 'false');
  assert.strictEqual(harness.operatedLane.getAttribute('aria-checked'), 'false');
  assert.strictEqual(harness.byoLane.getAttribute('tabindex'), '0');
  assert.strictEqual(harness.operatedLane.getAttribute('tabindex'), '-1');
  assert.ok(harness.byo.hidden && harness.operated.hidden);
  assert.ok(harness.panelCancel);
  selectOperated(harness);
  assert.strictEqual(harness.operatedLane.getAttribute('aria-checked'), 'true');
  assert.ok(!harness.operated.hidden && harness.byo.hidden);
  assert.strictEqual(harness.destinationByo.getAttribute('aria-checked'), 'true');
  click(harness.destinationHosted);
  assert.strictEqual(harness.destinationHosted.getAttribute('aria-checked'), 'true');
  assert.strictEqual(harness.operatedLane.getAttribute('aria-checked'), 'true');
  harness.operatedLane.dispatchEvent(event('keydown', { key: 'ArrowLeft' }));
  assert.strictEqual(harness.byoLane.getAttribute('aria-checked'), 'true');
  assert.strictEqual(harness.window.document.activeElement, harness.byoLane);
});

asyncCase('popup preflight failures make no server calls', async () => {
  for (const popupFactory of [() => null, () => ({ closed: true }), () => { throw new Error('blocked'); }]) {
    const harness = createHarness({ popupFactory });
    await ready(harness);
    selectOperated(harness);
    setKey(harness, 'recovery key');
    click(harness.primary);
    await settle();
    assert.strictEqual(harness.calls.length, 0);
    assert.ok(!harness.heading.hidden && !harness.keyControl.hidden && !harness.primary.hidden);
    assert.ok(!harness.outcome.hidden);
    assert.strictEqual(harness.key.getAttribute('aria-invalid'), null);
    assert.strictEqual(harness.key.getAttribute('aria-errormessage'), null);
  }
});

asyncCase('rapid hosted restore clicks issue one prepare request', async () => {
  let resolvePrepare;
  const harness = createHarness({
    respond(call) {
      if (call.url === '/app/backup/restore-hosted/prepare') {
        return new Promise((resolve) => { resolvePrepare = resolve; });
      }
      if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
      if (call.url === '/app/backup/status') return response(status());
      throw new Error('unexpected fetch ' + call.url);
    },
  });
  await ready(harness);
  selectOperated(harness);
  assert.ok(!harness.keyReassurance.hidden);
  setKey(harness, 'recovery key');
  click(harness.primary);
  click(harness.primary);
  await settle();
  assert.strictEqual(harness.calls.filter((call) => call.url.endsWith('/prepare')).length, 1);
  resolvePrepare(response({ capability: 'capability' }));
});

asyncCase('switching restore lanes cancels a prepare lease received after cancellation', async () => {
  let resolvePrepare;
  const harness = createHarness({
    respond(call) {
      if (call.url === '/app/backup/restore-hosted/prepare') {
        return new Promise((resolve) => { resolvePrepare = resolve; });
      }
      if (call.url === '/app/backup/restore-hosted/cancel') return response(status());
      if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
      if (call.url === '/app/backup/status') return response(status());
      throw new Error('unexpected fetch ' + call.url);
    },
  });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  click(harness.primary);
  await settle();
  click(harness.byoLane);
  await settle();
  resolvePrepare(response({ capability: 'late-capability' }));
  await settle();
  const cancel = harness.calls.find((call) => call.url === '/app/backup/restore-hosted/cancel');
  assert.ok(cancel);
  assert.deepStrictEqual(JSON.parse(cancel.request.body), { capability: 'late-capability' });
  assert.ok(!harness.byo.hidden && harness.operated.hidden);
});

asyncCase('cancelling after key submission stops before arm', async () => {
  const harness = createHarness({
    respond(call) {
      if (call.url === '/app/backup/restore-hosted/prepare') return response({ capability: 'capability' });
      if (call.url === '/app/backup/restore-hosted/key') return response({ portal_url: 'https://services.solstone.app/enable/backup?nonce=nonce&intent=restore' });
      if (call.url === '/app/backup/restore-hosted/cancel') return response(status(restoreOperation('error', 'cancelled')));
      if (call.url === '/app/backup/status') return response(status());
      if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
      throw new Error('unexpected fetch ' + call.url);
    },
  });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  click(harness.primary);
  await settle();
  assert.ok(!harness.attemptCancel.hidden);
  click(harness.attemptCancel);
  await settle();
  assert.deepStrictEqual(harness.calls.map((call) => call.url), [
    '/app/backup/restore-hosted/prepare',
    '/app/backup/restore-hosted/key',
    '/app/backup/restore-hosted/cancel',
  ]);
  assert.strictEqual(harness.calls.filter((call) => call.url.endsWith('/arm')).length, 0);
  assert.strictEqual(harness.calls.filter((call) => call.url.endsWith('/activate')).length, 0);
});

asyncCase('invalid hosted portal URL is cancelled before arm', async () => {
  const invalidPortalUrl = 'http://services.solstone.app/enable/backup?nonce=nonce&intent=restore';
  const harness = createHarness({
    respond(call) {
      if (call.url === '/app/backup/restore-hosted/prepare') return response({ capability: 'capability' });
      if (call.url === '/app/backup/restore-hosted/key') return response({ portal_url: invalidPortalUrl });
      if (call.url === '/app/backup/restore-hosted/cancel') return response(status(restoreOperation('error', 'cancelled')));
      if (call.url === '/app/backup/status') return response(status());
      if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
      throw new Error('unexpected fetch ' + call.url);
    },
  });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  click(harness.primary);
  await settle();
  assert.deepStrictEqual(harness.calls.map((call) => call.url), [
    '/app/backup/restore-hosted/prepare',
    '/app/backup/restore-hosted/key',
    '/app/backup/restore-hosted/cancel',
  ]);
  assert.strictEqual(harness.popups[0].location.replaced, null);
  assert.strictEqual(harness.calls.filter((call) => call.url.endsWith('/arm')).length, 0);
  assert.strictEqual(harness.calls.filter((call) => call.url.endsWith('/activate')).length, 0);
  await settle(32);
  assert.strictEqual(harness.outcome.textContent, 'something went wrong. you can try again.');
});

asyncCase('switching restore lanes cancels an in-flight hosted attempt', async () => {
  const harness = createHarness({
    respond(call) {
      if (call.url === '/app/backup/restore-hosted/prepare') return response({ capability: 'capability' });
      if (call.url === '/app/backup/restore-hosted/key') return new Promise(() => {});
      if (call.url === '/app/backup/restore-hosted/cancel') return response(status(restoreOperation('error', 'cancelled')));
      if (call.url === '/app/backup/status') return response(status());
      if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
      throw new Error('unexpected fetch ' + call.url);
    },
  });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  click(harness.primary);
  await settle();
  assert.ok(!harness.attemptCancel.hidden);
  click(harness.byoLane);
  await settle();
  const cancel = harness.calls.find((call) => call.url === '/app/backup/restore-hosted/cancel');
  assert.ok(cancel);
  assert.deepStrictEqual(JSON.parse(cancel.request.body), { capability: 'capability' });
  assert.ok(harness.attemptCancel.hidden);
  assert.ok(!harness.byo.hidden && harness.operated.hidden);
});

asyncCase('hosted restore follows prepare key arm activate and resumes status polling', async () => {
  const harness = createHarness({ respond: hostedSequence() });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  await startToPolling(harness);
  assert.deepStrictEqual(harness.calls.slice(0, 4).map((call) => call.url), [
    '/app/backup/restore-hosted/prepare',
    '/app/backup/restore-hosted/key',
    '/app/backup/restore-hosted/arm',
    '/app/backup/restore-hosted/activate',
  ]);
  assert.deepStrictEqual(JSON.parse(harness.calls[1].request.body), { capability: 'capability', recovery_key: 'recovery key' });
  assert.deepStrictEqual(JSON.parse(harness.calls[2].request.body), { capability: 'capability' });
  assert.deepStrictEqual(JSON.parse(harness.calls[3].request.body), { capability: 'capability' });
  assert.strictEqual(harness.popups[0].location.replaced, 'https://services.solstone.app/enable/backup?nonce=nonce&intent=restore');
  assert.strictEqual(harness.popups[0].opener, null);
  await harness.runTimer(800);
  assert.strictEqual(harness.calls[4].url, '/app/backup/status');
});

asyncCase('no hosted backup refusal is contained in the operated lane', async () => {
  const harness = createHarness({ respond: hostedSequence(status(restoreOperation('refused', 'no_hosted_backup'))) });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  await startToPolling(harness);
  await harness.runTimer(800);
  assert.strictEqual(harness.outcome.textContent, "sol pbc isn't holding an encrypted copy for the sign-in you used.");
  assert.ok(harness.keyControl.hidden && harness.keyReassurance.hidden && harness.primary.hidden);
  assert.strictEqual(harness.operatedLane.getAttribute('aria-checked'), 'true');
  assert.ok(harness.panelCancel && harness.banner.hidden);
});

asyncCase('expired hosted backup refusal is contained in the operated lane', async () => {
  const harness = createHarness({ respond: hostedSequence(status(restoreOperation('refused', 'hosted_backup_expired'))) });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  await startToPolling(harness);
  await harness.runTimer(800);
  assert.strictEqual(harness.outcome.textContent, 'sol pbc deleted that copy once 30 days had passed since encrypted backup stopped.');
  assert.ok(harness.keyControl.hidden && harness.keyReassurance.hidden && harness.primary.hidden && harness.banner.hidden);
});

asyncCase('needs subscription leaves the operated lane quiet', async () => {
  const harness = createHarness({ respond: hostedSequence(status(restoreOperation('needs_subscription'))) });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  await startToPolling(harness);
  await harness.runTimer(800);
  assert.ok(harness.outcome.hidden);
  assert.ok(!harness.keyControl.hidden && !harness.primary.hidden);
});

asyncCase('operated auth failure uses copy distinct from the shared reason', async () => {
  const harness = createHarness({ respond: hostedSequence(status(restoreOperation('error', 'auth_failed'))) });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  await startToPolling(harness);
  await harness.runTimer(800);
  assert.strictEqual(harness.outcome.textContent, "that recovery key didn't unlock the backup. check the key, then try signing in again.");
  assert.notStrictEqual(harness.outcome.textContent, "that recovery key didn't unlock the backup. check the key first, then the destination details.");
});

asyncCase('hosted recovery-key ARIA separates local validation from C3 failures', async () => {
  const harness = createHarness({ popupFactory: () => null });
  await ready(harness);
  selectOperated(harness);
  assert.ok(harness.primary.disabled);
  click(harness.primary);
  await settle();
  assert.strictEqual(harness.key.getAttribute('aria-describedby'), harness.heading.id);
  assert.strictEqual(harness.key.getAttribute('aria-invalid'), 'true');
  assert.strictEqual(harness.key.getAttribute('aria-errormessage'), harness.outcome.id);
  setKey(harness, 'recovery key');
  assert.ok(!harness.primary.disabled);
  assert.strictEqual(harness.key.getAttribute('aria-describedby'), harness.heading.id);
  click(harness.primary);
  await settle();
  assert.strictEqual(harness.key.getAttribute('aria-invalid'), null);
  assert.strictEqual(harness.key.getAttribute('aria-errormessage'), null);
  assert.strictEqual(harness.key.getAttribute('aria-describedby'), harness.heading.id);
  assert.strictEqual(harness.root.querySelectorAll('[role="status"]').length, 1);
});

asyncCase('server invalid_key marks the hosted recovery key invalid', async () => {
  const harness = createHarness({
    respond(call) {
      if (call.url === '/app/backup/restore-hosted/prepare') return response({ capability: 'capability' });
      if (call.url === '/app/backup/restore-hosted/key') return response({ reason_code: 'invalid_key' }, 400);
      if (call.url === '/app/backup/restore-hosted/cancel') return response(status());
      if (call.url === '/app/backup/status') return response(status());
      if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
      throw new Error('unexpected fetch ' + call.url);
    },
  });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  click(harness.primary);
  await settle(32);
  assert.strictEqual(harness.key.getAttribute('aria-invalid'), 'true');
  assert.strictEqual(harness.key.getAttribute('aria-errormessage'), harness.outcome.id);
  assert.strictEqual(
    harness.outcome.textContent,
    'that recovery key didn\'t unlock the backup. re-enter the key from your saved copy.',
  );
});

asyncCase('primary remains disabled through an in-flight attempt', async () => {
  let resolvePrepare;
  const harness = createHarness({
    respond(call) {
      if (call.url === '/app/backup/restore-hosted/prepare') return new Promise((resolve) => { resolvePrepare = resolve; });
      if (call.url === '/app/backup/status') return response(status());
      if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
      throw new Error('unexpected fetch ' + call.url);
    },
  });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  click(harness.primary);
  await settle();
  assert.ok(harness.primary.disabled);
  harness.primary.disabled = false;
  harness.key.dispatchEvent(event('input'));
  assert.ok(harness.primary.disabled);
  resolvePrepare(response({ capability: 'capability' }));
});

async function runAsyncCases() {
  for (const runCase of asyncCases) await runCase();
  console.log('DOM CASES: ' + cases + ' passed');
}

runAsyncCases().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
