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
  get href() { return this.getAttribute('href') || ''; }
  set href(value) { if (value) this.setAttribute('href', value); else this.removeAttribute('href'); }
  get title() { return this.getAttribute('title') || ''; }
  set title(value) { this.setAttribute('title', value); }
  get className() { return this.getAttribute('class') || ''; }
  set className(value) { this.setAttribute('class', value); }
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

  insertAdjacentElement(position, element) {
    const parent = this.parentElement;
    if (!parent) throw new Error('insertAdjacentElement needs a parent');
    const index = parent.children.indexOf(this);
    if (index < 0) throw new Error('insertAdjacentElement needs an attached node');
    if (position === 'afterend') parent.children.splice(index + 1, 0, element);
    else if (position === 'beforebegin') parent.children.splice(index, 0, element);
    else throw new Error('unsupported insertAdjacentElement position ' + position);
    element.parentElement = parent;
    return element;
  }

  replaceChildren(...children) {
    this.children.forEach((child) => { child.parentElement = null; });
    this.children = [];
    this.append(...children);
  }

  cloneNode(deep = false) {
    const clone = new Element(this.tagName, this.ownerDocument);
    for (const [name, value] of Object.entries(this.attributes)) clone.setAttribute(name, value);
    clone.textContent = this.textContent;
    if (deep) for (const child of this.children) clone.appendChild(child.cloneNode(true));
    return clone;
  }

  querySelector(selector) { return queryAll(this, selector)[0] || null; }
  querySelectorAll(selector) { return queryAll(this, selector); }
  matches(selector) { return matchesSelector(this, selector); }

  closest(selector) {
    let current = this;
    while (current && typeof current.matches === 'function') {
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
    this.documentElement.parentElement = this;
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
  dispatchEvent(event) {
    event.currentTarget = this;
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    return !event.defaultPrevented;
  }
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
  return { success: true, enabled: false, mode: 'byo', hosted, operation, last_backup: null, last_verification: null };
}

function response(body, statusCode = 200) {
  return {
    ok: statusCode >= 200 && statusCode < 300,
    status: statusCode,
    json() { return Promise.resolve(body); },
  };
}

function restoreOperation(phase, reasonCode, portalUrl = 'https://services.solstone.app/enable/backup?nonce=nonce&intent=restore') {
  return {
    kind: 'restore_hosted',
    phase,
    reason_code: reasonCode || null,
    portal_url: portalUrl,
    elapsed_ms: 0,
  };
}

function fixture(document) {
  const root = add(document.body, 'section', { class: 'backup-shell', 'data-backup-root': '', 'data-state': 'empty' });
  const banner = add(root, 'div', { 'data-operation-banner': '', hidden: '' });
  add(banner, 'span', { class: 'backup-spinner' });
  add(banner, 'span', { 'data-operation-phase': '' });
  add(banner, 'span', { 'data-operation-error': '' });
  const restoreReport = add(root, 'div', { 'data-operation-restore-report': '', hidden: '' });
  add(restoreReport, 'p', { 'data-operation-restore-summary': '', role: 'status', 'aria-live': 'polite', 'aria-atomic': 'true' });
  const restoreDetails = add(restoreReport, 'details', { 'data-operation-restore-details': '', hidden: '' });
  add(restoreDetails, 'summary', { 'data-copy': 'offload.restore_report.files' });
  add(restoreDetails, 'ol', { 'data-operation-restore-files': '' });
  const cleanupBanner = add(root, 'div', { 'data-cleanup-banner': '', hidden: '' });
  add(cleanupBanner, 'span', { 'data-cleanup-heading': '' });
  add(cleanupBanner, 'span', { 'data-cleanup-reason': '' });
  const cleanup = add(cleanupBanner, 'button', { 'data-action': 'finish-cleanup', hidden: '' });

  add(root, 'p', { 'data-backup-loading': '', role: 'status' });
  const panels = add(root, 'div', { class: 'backup-panels' });
  const intro = add(panels, 'article', { 'data-backup-panel': 'intro', hidden: '' });
  const showRestore = add(intro, 'button', { 'data-action': 'show-restore' });
  const generateKey = add(intro, 'button', { 'data-action': 'understand' });
  const management = add(panels, 'article', { 'data-backup-panel': 'management', hidden: '' });
  const managementGrid = add(management, 'div', { class: 'backup-management-grid' });
  const lastBackup = add(managementGrid, 'p', { 'data-last-backup': '' });
  add(managementGrid, 'p', { 'data-last-successful-backup': '' });
  add(managementGrid, 'p', { 'data-last-successful-backup-reason': '', hidden: '' });
  add(managementGrid, 'p', { 'data-backup-history-note': '', hidden: '', 'data-copy': 'management.status_labels.backup_history_note' });
  const backupNow = add(management, 'button', { 'data-action': 'backup-now' });
  const viewKey = add(management, 'button', { 'data-action': 'view-key' });
  const rotateKey = add(management, 'button', { 'data-action': 'rotate-key', disabled: '' });
  rotateKey.disabled = true;
  const retention = add(management, 'form', { 'data-retention-form': '' });
  const saveRetention = add(retention, 'button', { type: 'submit' });
  const offload = add(management, 'section', { 'data-offload-section': '' });
  const summary = add(offload, 'div', { 'data-offload-summary': '' });
  for (const name of ['raw', 'backup-only', 'proposed', 'eligible', 'projected']) {
    add(summary, 'p', { ['data-offload-' + name + '-bytes']: '' });
  }
  for (const name of ['raw', 'backup-only']) {
    const known = add(summary, 'p', { ['data-offload-' + name + '-known-row']: '', hidden: '' });
    add(known, 'span', { ['data-offload-' + name + '-known-bytes']: '' });
  }
  add(summary, 'p', { 'data-offload-device-free': '' });
  add(summary, 'p', { 'data-offload-device-total': '' });
  add(offload, 'p', { 'data-offload-coverage-note': '', hidden: '' });
  const tiering = add(offload, 'section', { class: 'backup-offload-tiering' });
  add(tiering, 'div', { 'data-offload-days': '' });
  const dayTemplate = add(tiering, 'template', { 'data-offload-day-template': '' });
  dayTemplate.content = new Element('#fragment', document);
  const day = add(dayTemplate.content, 'article', { class: 'backup-offload-day' });
  const detail = add(day, 'div', { 'data-offload-day-detail': '' });
  add(detail, 'strong', { 'data-offload-day-value': '' });
  add(detail, 'span', { 'data-offload-day-backup-only-bytes': '' });
  const rawRow = add(detail, 'span', { 'data-offload-day-raw-row': '' });
  add(rawRow, 'span', { 'data-offload-day-raw-bytes': '' });
  add(day, 'button', { 'data-action': 'offload-restore-day', 'data-offload-day-restore': '' });
  const offloadProof = add(offload, 'div', { 'data-offload-proof': '', hidden: '' });
  const lastOffload = add(offloadProof, 'p', { 'data-offload-last-run': '' });
  const lastVerify = add(offloadProof, 'p', { 'data-offload-last-verify': '' });
  const runHistory = add(offloadProof, 'p', { 'data-offload-run-history': '', hidden: '' });
  const runSuccessTime = add(runHistory, 'span', { 'data-offload-success-time': '' });
  const verifyHistory = add(offloadProof, 'p', { 'data-offload-verify-history': '', hidden: '' });
  const verifySuccessTime = add(verifyHistory, 'span', { 'data-offload-success-time': '' });
  const stallReason = add(offload, 'p', { 'data-offload-stall-reason': '', hidden: '' });
  const saveOffload = add(offload, 'button', { 'data-action': 'offload-save' });
  add(offload, 'button', { 'data-action': 'offload-enable' });
  const teardownSection = add(management, 'section', { class: 'backup-teardown' });
  const teardownOpen = add(teardownSection, 'button', { 'data-action': 'teardown-open' });
  const teardownGate = add(teardownSection, 'div', { class: 'backup-teardown-gate', 'data-teardown-gate': '', hidden: '' });
  add(teardownGate, 'p', { class: 'backup-warning', 'data-teardown-stakes': '' });
  const teardownInput = add(teardownGate, 'input', { 'data-teardown-input': '', name: 'teardown_confirm' });
  add(teardownGate, 'p', { class: 'backup-status-message', 'data-teardown-status': '' });
  add(teardownGate, 'button', { 'data-action': 'teardown-restore-first' });
  const teardownConfirm = add(teardownGate, 'button', { 'data-action': 'teardown-confirm', disabled: '' });
  teardownConfirm.disabled = true;
  add(teardownGate, 'button', { 'data-action': 'teardown-cancel' });

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
  const reopen = add(operated, 'a', { 'data-hosted-restore-reopen': '', 'data-solstone-outside': '', hidden: '' });
  const byo = add(restore, 'div', { 'data-restore-lane-panel': 'byo', hidden: '' });
  add(byo, 'form', { 'data-restore-form': '' });
  const panelCancel = add(restore, 'button', { 'data-action': 'cancel-restore' });
  panelCancel.textContent = 'cancel';
  return { root, showRestore, generateKey, backupNow, viewKey, rotateKey, retention, saveRetention, offloadProof, lastOffload, lastVerify, runHistory, runSuccessTime, verifyHistory, verifySuccessTime, stallReason, saveOffload, restore, destinationByo, destinationHosted, byoLane, operatedLane, operated, byo, heading, keyControl, key, keyReassurance, outcome, primary, attemptCancel, reopen, panelCancel, banner, cleanupBanner, cleanup, management, managementGrid, lastBackup, teardownOpen, teardownGate, teardownInput, teardownConfirm };
}

function createHarness(options = {}) {
  const document = new Document();
  const elements = fixture(document);
  const summary = document.createElement('p');
  summary.setAttribute('data-backup-summary', '');
  elements.root.appendChild(summary);
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
  const location = {
    origin: 'http://127.0.0.1:8080',
    assigned: null,
    set href(val) { this.assigned = val; },
    assign(val) { this.assigned = val; },
  };
  const navigator = { userAgent: options.userAgent || 'Mozilla/5.0 (X11; Linux x86_64)' };
  const window = {
    document,
    console,
    fetch,
    URL,
    location,
    navigator,
    top: null,
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
  window.top = window;
  const context = vm.createContext({ window, document, console, fetch, URL, navigator, location, setTimeout: window.setTimeout, clearTimeout: window.clearTimeout });
  const shellAssets = path.resolve(crateDir, '..', 'solstone-core-convey-shell', 'assets', 'static');
  const hostJs = fs.readFileSync(path.join(shellAssets, 'journal-web-host.js'), 'utf8');
  const navJs = fs.readFileSync(path.join(shellAssets, 'external-navigation.js'), 'utf8');
  vm.runInContext(hostJs, context, { filename: 'journal-web-host.js' });
  if (options.capable) {
    navigator.userAgent = `${navigator.userAgent} ${window.solstoneJournalWebHost.userAgentProduct}`;
  }
  if (options.capability) {
    window[window.solstoneJournalWebHost.javascriptCapability] = 1;
  }
  vm.runInContext(navJs, context, { filename: 'external-navigation.js' });
  const source = fs.readFileSync(path.join(crateDir, 'assets', 'backup.js'), 'utf8');
  vm.runInContext(fs.readFileSync(path.join(crateDir, 'assets', 'backup-status.js'), 'utf8'), context, { filename: 'backup-status.js' });
  vm.runInContext(source, context, { filename: 'backup.js' });
  return {
    ...elements,
    calls,
    timers,
    popups,
    window,
    location,
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

testCase('restore report markup has a live summary and closed native disclosure', () => {
  const workspace = fs.readFileSync(path.join(crateDir, 'assets', 'workspace.html'), 'utf8');
  const start = workspace.indexOf('data-operation-restore-report');
  assert.ok(start >= 0);
  const report = workspace.slice(workspace.lastIndexOf('<div', start), workspace.indexOf('</div>', start));
  assert.match(report, /^<div[^>]*data-operation-restore-report[^>]*\bhidden\b/);
  const summary = report.match(/<p\b[^>]*data-operation-restore-summary[^>]*>/)[0];
  assert.match(summary, /role="status"/);
  assert.match(summary, /aria-live="polite"/);
  assert.match(summary, /aria-atomic="true"/);
  const details = report.match(/<details\b[^>]*data-operation-restore-details[^>]*>/)[0];
  assert.match(details, /\bhidden\b/);
  assert.doesNotMatch(details, /\bopen(?:\s|=|>)/);
  assert.match(report, /<summary\b[^>]*data-copy="offload\.restore_report\.files"[^>]*><\/summary>/);
  assert.match(report, /<ol\b[^>]*data-operation-restore-files[^>]*><\/ol>/);
});

testCase('offload byte units are binary, matching settings’ unit family (g3-108)', () => {
  // Settings renders the same raw-media total as GiB (binary). Before this fix,
  // backup divided by a decimal gigabyte, so the two screens read up to ~7% apart
  // on the one number that gates letting media leave the device. The DOM fixture
  // here has no offload markup to render against, so this pins the constants and
  // suffixes formatBytes reads from directly.
  const source = fs.readFileSync(path.join(crateDir, 'assets', 'backup.js'), 'utf8');
  assert.ok(source.includes('const BYTES_PER_GB = 1073741824;'), 'BYTES_PER_GB must be 2^30, not a decimal gigabyte');
  assert.ok(source.includes('const BYTES_PER_MB = 1048576;'), 'BYTES_PER_MB must be 2^20, not a decimal megabyte');
  assert.ok(source.includes('"gb_suffix": "GiB"'), 'the raw-media suffix must read GiB, matching settings');
  assert.ok(source.includes('"mb_suffix": "MiB"'), 'the sub-GiB suffix must read MiB');
  assert.ok(source.includes('"under_1mb": "under 1 MiB"'), 'the under-threshold copy must match the MiB suffix');
  assert.ok(!source.includes('"gb_suffix": "GB"'), 'no leftover decimal GB suffix');
  assert.ok(!source.includes('"mb_suffix": "MB"'), 'no leftover decimal MB suffix');
});

asyncCase('offload history stays distinct from current operation effects through refresh', async () => {
  const earlier = 1760000000;
  const current = 1770000000;
  let payload = {
    success: true, offload: { enabled: true }, days: [],
    last_offload: { status: 'stalled', time: current, reason: 'confirm_failed', last_ok_time: earlier },
    last_verification: { status: 'error', time: current, reason: 'timeout', last_ok_time: earlier },
  };
  const currentOperation = offloadRestoreOperation({
    status: 'partial', files_restored: 1, bytes_restored: 2 * 1024 * 1024,
    files: [{ day: '20261010', stream: 'stream', segment: 'segment', name: 'audio.flac',
      disposition: 'published', files_restored: 1, bytes_restored: 2 * 1024 * 1024,
      evidence: { completed: false, hold: { detail_available: false } } }],
  });
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') return response({ ...status(currentOperation), enabled: true, recovery_key_confirmed: true });
    if (['/app/backup/offload/status', '/app/backup/offload/config'].includes(call.url)) return response(payload);
  } });
  await ready(harness);
  const title = (time) => new Date(time * 1000).toLocaleString();
  assert.ok(!harness.offloadProof.hidden);
  assert.strictEqual(harness.lastOffload.getAttribute('data-result-status'), 'stalled');
  assert.strictEqual(harness.lastVerify.getAttribute('data-result-status'), 'error');
  assert.strictEqual(harness.lastOffload.getAttribute('title'), title(current));
  assert.strictEqual(harness.lastVerify.getAttribute('title'), title(current));
  assert.strictEqual(harness.runSuccessTime.getAttribute('title'), title(earlier));
  assert.strictEqual(harness.verifySuccessTime.getAttribute('title'), title(earlier));
  assert.ok(!harness.runHistory.hidden && !harness.verifyHistory.hidden);
  assert.ok(harness.stallReason.hidden);
  assert.strictEqual(harness.stallReason.textContent, '');
  const currentReport = restoreReportNodes(harness);
  const effects = currentReport.summary.textContent;
  const fileRows = currentReport.rows.children.map((row) => row.textContent);
  function currentEffectsRemain() {
    assert.ok(!currentReport.container.hidden);
    assert.strictEqual(currentReport.summary.textContent, effects);
    assert.deepStrictEqual(currentReport.rows.children.map((row) => row.textContent), fileRows);
    assert.strictEqual(currentReport.rows.children[0].getAttribute('data-restore-disposition'), 'published');
  }
  assert.ok(effects.includes('2 MiB'));
  assert.strictEqual(fileRows.length, 1);
  currentEffectsRemain();
  const failedOffload = harness.lastOffload.textContent;
  const failedVerify = harness.lastVerify.textContent;

  payload = { ...payload,
    last_offload: { ...payload.last_offload, reason: 'locked' },
    last_verification: { ...payload.last_verification, reason: 'locked' },
  };
  click(harness.saveOffload);
  await settle();
  assert.notStrictEqual(harness.lastOffload.textContent, failedOffload);
  assert.notStrictEqual(harness.lastVerify.textContent, failedVerify);

  currentEffectsRemain();
  harness.stallReason.hidden = false;
  harness.stallReason.textContent = 'stale warning';
  const lockedOffload = harness.lastOffload.textContent;
  const lockedVerify = harness.lastVerify.textContent;
  payload = { ...payload,
    last_offload: { ...payload.last_offload, reason: 'unfamiliar_reason' },
    last_verification: { ...payload.last_verification, reason: 'integrity_failed' },
  };
  click(harness.saveOffload);
  await settle();
  assert.notStrictEqual(harness.lastOffload.textContent, lockedOffload);
  assert.ok(harness.stallReason.hidden);
  assert.strictEqual(harness.stallReason.textContent, '');
  assert.strictEqual(harness.lastOffload.getAttribute('title'), title(current));
  currentEffectsRemain();
  assert.notStrictEqual(harness.lastVerify.textContent, lockedVerify);
  const unknownReason = harness.lastOffload.textContent;
  payload = { ...payload, last_offload: { ...payload.last_offload, reason: null } };
  click(harness.saveOffload);
  await settle();
  assert.notStrictEqual(harness.lastOffload.textContent, unknownReason);

  payload = { ...payload,
    last_offload: { status: 'ok', time: current + 10, last_ok_time: current + 10 },
    last_verification: { status: 'ok', time: current + 10, last_ok_time: current + 10 },
  };
  click(harness.saveOffload);
  await settle();
  currentEffectsRemain();
  assert.strictEqual(harness.lastOffload.getAttribute('data-result-status'), 'ok');
  assert.strictEqual(harness.lastVerify.getAttribute('data-result-status'), 'ok');
  assert.strictEqual(harness.lastOffload.getAttribute('title'), title(current + 10));
  assert.notStrictEqual(harness.lastOffload.textContent, failedOffload);
  assert.notStrictEqual(harness.lastVerify.textContent, failedVerify);
  assert.strictEqual(harness.runSuccessTime.getAttribute('title'), title(current + 10));
  assert.ok(harness.stallReason.hidden);
  assert.strictEqual(harness.stallReason.textContent, '');

  payload = { ...payload, last_offload: null, last_verification: null };
  click(harness.saveOffload);
  await settle();
  currentEffectsRemain();
  for (const element of [harness.lastOffload, harness.lastVerify]) {
    assert.strictEqual(element.getAttribute('data-result-status'), 'missing');
    assert.ok(!element.hasAttribute('title'));
  }
  for (const element of [harness.runHistory, harness.verifyHistory]) assert.ok(element.hidden);
  for (const element of [harness.runSuccessTime, harness.verifySuccessTime]) {
    assert.strictEqual(element.textContent, '');
    assert.ok(!element.hasAttribute('title'));
  }

  payload = { ...payload,
    last_offload: { status: 'unfamiliar', time: current, last_ok_time: earlier },
    last_verification: { last_ok_time: earlier },
  };
  click(harness.saveOffload);
  await settle();
  currentEffectsRemain();
  assert.strictEqual(harness.lastOffload.getAttribute('data-result-status'), 'unknown');
  assert.strictEqual(harness.lastVerify.getAttribute('data-result-status'), 'missing');
  assert.strictEqual(harness.lastOffload.getAttribute('title'), title(current));
  assert.strictEqual(harness.runSuccessTime.getAttribute('title'), title(earlier));
  assert.strictEqual(harness.verifySuccessTime.getAttribute('title'), title(earlier));
});

asyncCase('checked accounting forwards distinct totals and clears incomplete observations', async () => {
  const MiB = 1048576;
  let payload = {
    success: true, offload: { enabled: true },
    days: [{ day: '20260101', raw_media_bytes: 23 * MiB, raw_media_complete: true, backup_only_bytes: 3 * MiB, backup_only_segments: 1, backup_only_complete: true }],
    raw_media: { total_bytes: 23 * MiB, known_bytes: 23 * MiB, complete: true },
    backup_only: { total_bytes: 3 * MiB, complete: true, degraded: false },
    proposed_release: { structural_bytes: 11 * MiB, eligible_bytes: 7 * MiB, projected_local_bytes: 16 * MiB, available: true },
    device: { free_bytes: 31 * MiB, total_bytes: 100 * MiB },
  };
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') return response({ ...status(), enabled: true, recovery_key_confirmed: true });
    if (['/app/backup/offload/status', '/app/backup/offload/config', '/app/backup/offload/enable'].includes(call.url)) return response(payload);
  } });
  await ready(harness);
  const get = (name) => harness.root.querySelector('[data-offload-' + name + ']');
  const refresh = async () => {
    click(harness.saveOffload.disabled ? harness.root.querySelector('[data-action="offload-enable"]') : harness.saveOffload);
    await settle();
  };
  for (const [name, amount] of [['raw-bytes', 23], ['backup-only-bytes', 3], ['proposed-bytes', 11], ['eligible-bytes', 7], ['projected-bytes', 16], ['device-free', 31], ['device-total', 100]]) {
    assert.strictEqual(get(name).textContent, amount + ' MiB');
  }
  const boundDay = get('days').querySelector('.backup-offload-day');
  assert.strictEqual(boundDay.querySelector('[data-offload-day-backup-only-bytes]').textContent, '3 MiB');
  assert.strictEqual(boundDay.querySelector('[data-offload-day-raw-bytes]').textContent, '23 MiB');
  assert.ok(!boundDay.querySelector('[data-offload-day-restore]').disabled);
  assert.ok(!boundDay.querySelector('.backup-warning'));
  assert.ok(get('coverage-note').hidden && get('raw-known-row').hidden);
  payload.proposed_release.available = false;
  payload.days = [];
  click(harness.saveOffload); await settle();
  const completeEmpty = get('days').children[0].textContent;
  const unavailable = get('eligible-bytes').textContent;
  assert.ok(unavailable && unavailable !== '7 MiB');
  assert.strictEqual(get('projected-bytes').textContent, unavailable);
  assert.strictEqual(get('proposed-bytes').textContent, '11 MiB');

  payload.backup_only = { total_bytes: null, known_bytes: 2 * MiB, complete: false, degraded: true };
  click(harness.saveOffload); await settle();
  assert.strictEqual(get('raw-bytes').textContent, '23 MiB');
  assert.strictEqual(get('backup-only-bytes').textContent, unavailable);
  assert.ok(!get('coverage-note').hidden);

  payload.raw_media = { total_bytes: null, known_bytes: 5 * MiB, complete: false };
  payload.backup_only = { total_bytes: null, known_bytes: 2 * MiB, complete: false, unbound_files: 1, degraded: false };
  payload.proposed_release.available = true;
  payload.offload.enabled = false;
  payload.days = [{ day: '20260101', raw_media_bytes: null, raw_media_complete: false, backup_only_bytes: null, backup_only_segments: null, backup_only_complete: false, unbound_files: 1 }];
  click(harness.saveOffload); await settle();
  assert.strictEqual(get('raw-bytes').textContent, unavailable);
  assert.strictEqual(get('backup-only-bytes').textContent, unavailable);
  assert.strictEqual(get('projected-bytes').textContent, unavailable);
  assert.ok(!get('raw-known-row').hidden && !get('backup-only-known-row').hidden);
  assert.strictEqual(get('raw-known-bytes').textContent, '5 MiB');
  assert.strictEqual(get('backup-only-known-bytes').textContent, '2 MiB');
  assert.ok(!harness.root.querySelector('.backup-offload-tiering').hidden);
  const day = get('days').querySelector('.backup-offload-day');
  assert.ok(day && day.querySelector('.backup-warning'));
  assert.strictEqual(day.querySelector('[data-offload-day-raw-bytes]').textContent, unavailable);
  assert.ok(!day.querySelector('[data-offload-day-raw-row]').hidden);
  assert.strictEqual(day.querySelector('[data-offload-day-backup-only-bytes]').textContent, unavailable);
  assert.ok(day.querySelector('[data-offload-day-restore]').disabled);

  payload.days = [];
  await refresh();
  assert.ok(!get('coverage-note').hidden);
  assert.ok(get('days').querySelector('.backup-warning'));
  assert.notStrictEqual(get('days').children[0].textContent, completeEmpty);

  payload.raw_media = { total_bytes: 0, complete: true };
  payload.backup_only = { total_bytes: 0, complete: true, degraded: false };
  payload.proposed_release = { structural_bytes: 0, eligible_bytes: 0, projected_local_bytes: 0, available: true };
  payload.offload.enabled = true;
  await refresh();
  for (const name of ['raw-bytes', 'backup-only-bytes', 'proposed-bytes', 'eligible-bytes', 'projected-bytes']) assert.strictEqual(get(name).textContent, '0 GiB');
  for (const name of ['raw', 'backup-only']) {
    assert.ok(get(name + '-known-row').hidden);
    assert.strictEqual(get(name + '-known-bytes').textContent, '');
  }
  assert.ok(get('coverage-note').hidden);
  assert.strictEqual(get('coverage-note').textContent, '');
  assert.strictEqual(get('days').children[0].textContent, completeEmpty);
});

asyncCase('initial settings stay hidden until status is known', async () => {
  let release;
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') return new Promise((resolve) => { release = resolve; });
  } });
  await settle();
  const panels = harness.root.querySelectorAll('[data-backup-panel]');
  assert.ok(panels.every((panel) => panel.hidden));
  assert.ok(!harness.root.querySelector('[data-backup-loading]').hidden);
  release(response(status()));
  await settle();
  assert.ok(harness.root.querySelector('[data-backup-loading]').hidden);
  assert.ok(!harness.root.querySelector('[data-backup-panel="intro"]').hidden);
});

asyncCase('backup settings become usable while storage measurement is pending', async () => {
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/offload/status') return new Promise(() => {});
  } });
  await settle();
  assert.ok(harness.root.querySelector('[data-backup-loading]').hidden);
  assert.ok(!harness.root.querySelector('[data-backup-panel="intro"]').hidden);
});

asyncCase('an initial status failure never reveals assumed settings', async () => {
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') return Promise.reject(new Error('status unavailable'));
  } });
  await settle();
  assert.ok(harness.root.querySelectorAll('[data-backup-panel]').every((panel) => panel.hidden));
  const loading = harness.root.querySelector('[data-backup-loading]');
  assert.ok(!loading.hidden);
  assert.ok(loading.textContent.includes("couldn't load backup settings"));
});

asyncCase('pending cleanup survives a status failure and exposes recovery after reload', async () => {
  let statusReads = 0;
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') {
      statusReads += 1;
      if (statusReads === 2) return Promise.reject(new Error('temporary status failure'));
      return response(status(restoreOperation('cleanup_pending', 'cleanup_pending')));
    }
  } });
  await ready(harness);
  assert.strictEqual(harness.root.getAttribute('data-state'), 'cleanup_pending');
  assert.ok(!harness.cleanup.hidden && !harness.cleanup.disabled);
  await harness.runTimer(800);
  assert.strictEqual(harness.root.getAttribute('data-state'), 'cleanup_pending');
  assert.ok(!harness.cleanup.hidden && !harness.cleanup.disabled);
  await harness.runTimer(800);
  assert.strictEqual(statusReads, 3);
  assert.ok(harness.calls.every((call) => !call.request.method || call.request.method === 'GET'));
});

asyncCase('global cleanup survives reload and fetch failure without changing an earlier success', async () => {
  let reads = 0;
  const completed = restoreOperation('done');
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') {
      reads += 1;
      if (reads === 2) return Promise.reject(new Error('temporary fetch failure'));
      return response(Object.assign(status(completed), { cleanup_admission: 'pending' }));
    }
    if (call.url === '/app/backup/api/cleanup/retry') {
      return response({ operation: completed, cleanup_admission: 'ready' });
    }
  } });
  await ready(harness);
  const phase = harness.root.querySelector('[data-operation-phase]').textContent;
  assert.strictEqual(harness.root.querySelector('[data-operation-error]').textContent, '');
  assert.ok(harness.banner.querySelector('.backup-spinner').hidden);
  assert.ok(!harness.cleanupBanner.hidden && !harness.cleanup.disabled);
  assert.strictEqual(harness.cleanup.textContent, 'finish stopping task');
  await harness.runTimer(800);
  assert.strictEqual(harness.root.querySelector('[data-operation-phase]').textContent, phase);
  assert.ok(!harness.cleanupBanner.hidden);
  await harness.runTimer(800);
  assert.strictEqual(reads, 3);
  click(harness.cleanup);
  await settle();
  assert.ok(harness.cleanupBanner.hidden);
  assert.strictEqual(harness.root.querySelector('[data-operation-phase]').textContent, phase);
  assert.deepStrictEqual(harness.calls.filter((call) => call.request.method === 'POST')
    .map((call) => call.url), ['/app/backup/api/cleanup/retry']);
});

asyncCase('a successful operation has no failure fallback or active spinner', async () => {
  const harness = createHarness({ statusQueue: [status(restoreOperation('done'))] });
  await ready(harness);
  assert.strictEqual(harness.root.querySelector('[data-operation-error]').textContent, '');
  assert.ok(harness.banner.querySelector('.backup-spinner').hidden);
  assert.ok(harness.cleanupBanner.hidden);
});

function offloadRestoreOperation(report, phase = 'error', reason = null) {
  return { kind: 'offload_restore', phase, reason_code: reason, report, elapsed_ms: 0 };
}

function restoreReportNodes(harness) {
  const query = (selector) => harness.root.querySelector(selector);
  return {
    container: query('[data-operation-restore-report]'),
    summary: query('[data-operation-restore-summary]'),
    details: query('[data-operation-restore-details]'),
    rows: query('[data-operation-restore-files]'),
  };
}

asyncCase('restore reports preserve ordered dispositions, effects and literal locations', async () => {
  const dispositions = ['published', 'already_present', 'conflict', 'unverified', 'unattempted'];
  const files = dispositions.map((disposition, index) => ({
    day: '20261010', stream: 'stream-' + index, segment: 'segment', name: 'same.wav',
    disposition, files_restored: index === 0 || index === 3 ? 1 : 0,
    bytes_restored: index === 0 || index === 3 ? 2 * 1024 * 1024 : 0,
    evidence: index === 3 ? { completed: false, register: { status: 'refused' }, hold: { detail_available: false } } : {},
  }));
  files[4].name = '<img src=x onerror=alert(1)>.wav';
  files[4].stream = '<script>private</script>';
  const harness = createHarness({ statusQueue: [status(offloadRestoreOperation({
    status: 'partial', files_restored: 7, bytes_restored: 9 * 1024 * 1024, files,
    reason_detail: 'PRIVATE DIAGNOSTIC',
  }))] });
  await ready(harness);
  const { container, summary, details, rows } = restoreReportNodes(harness);
  assert.ok(!container.hidden && !details.hidden);
  assert.strictEqual(details.getAttribute('open'), null);
  assert.ok(summary.textContent.includes('7') && summary.textContent.includes('9 MiB'));
  assert.strictEqual(rows.children.length, 5);
  assert.deepStrictEqual(rows.children.map((row) => row.getAttribute('data-restore-disposition')), dispositions);
  for (let index = 0; index < rows.children.length; index += 1) {
    const row = rows.children[index];
    assert.ok(row.textContent.includes([files[index].day, files[index].stream, files[index].segment, files[index].name].join('/')));
    assert.strictEqual(row.children.length, 0);
    assert.ok(!row.textContent.includes('PRIVATE DIAGNOSTIC'));
  }
  assert.ok(rows.children[0].textContent.includes('2 MiB'));
  assert.ok(rows.children[1].textContent.includes('0 GiB'));
  assert.ok(rows.children[3].textContent.includes('2 MiB'));
  assert.strictEqual(rows.querySelectorAll('img').length, 0);
  assert.ok(harness.banner.querySelector('.backup-spinner').hidden);
  assert.deepStrictEqual(harness.calls, []);
});

asyncCase('restore attention comes only from an explicit hold, independently of effects', async () => {
  const files = [undefined, null, { detail_available: false }].map((hold, index) => ({
    name: 'same.wav', disposition: 'unverified', files_restored: 1, bytes_restored: 2 * 1024 * 1024,
    evidence: { completed: false, register: { status: 'refused' }, ...(index === 0 ? {} : { hold }) },
  }));
  const harness = createHarness({ statusQueue: [status(offloadRestoreOperation({ files_restored: 3, bytes_restored: 6 * 1024 * 1024, files }))] });
  await ready(harness);
  const { rows } = restoreReportNodes(harness);
  assert.strictEqual(rows.children[0].textContent, rows.children[1].textContent);
  assert.ok(rows.children[2].textContent.startsWith(rows.children[0].textContent + ' · '));
  for (const row of rows.children) assert.ok(row.textContent.includes('2 MiB'));
});

asyncCase('unavailable restore counts and malformed rows never become zero or success', async () => {
  const invalid = [undefined, null, -1, 1.5, '4', Number.MAX_SAFE_INTEGER + 1];
  const files = invalid.map((value) => ({ disposition: 'future', files_restored: value, bytes_restored: value }));
  files.push(null, 'invalid', [], { disposition: ['published'], files_restored: 0, bytes_restored: 0 });
  const harness = createHarness({ statusQueue: [status(offloadRestoreOperation({ files_restored: null, bytes_restored: Number.MAX_SAFE_INTEGER + 1, files }))] });
  await ready(harness);
  const { summary, rows } = restoreReportNodes(harness);
  const unavailable = harness.root.querySelector('[data-offload-proposed-bytes]').textContent;
  assert.ok(unavailable.length > 0);
  assert.strictEqual(summary.textContent.split(unavailable).length - 1, 2);
  for (let index = 0; index < invalid.length; index += 1) {
    assert.strictEqual(rows.children[index].textContent.split(unavailable).length - 1, 2);
  }
  assert.ok(rows.children.every((row) => row.getAttribute('data-restore-disposition') === 'unknown'));
  for (const row of rows.children.slice(invalid.length, -1)) {
    assert.ok(row.textContent.length > 0);
    assert.ok(!row.textContent.includes('0 GiB'));
  }
  assert.ok(rows.children.at(-1).textContent.includes('0 GiB'));
});

asyncCase('current restore snapshots replace populated rows and clear missing reports', async () => {
  const report = (name, count) => ({ files_restored: count, bytes_restored: count * 1024 * 1024,
    files: [{ day: '20261010', name, disposition: 'published', files_restored: count, bytes_restored: count * 1024 * 1024 }] });
  const first = offloadRestoreOperation(report('first.wav', 1), 'restoring');
  const updated = { ...first, report: report('updated.wav', 2) };
  const next = offloadRestoreOperation(report('next.wav', 3), 'restoring');
  const harness = createHarness({ statusQueue: [first, updated, next,
    offloadRestoreOperation({ files_restored: 0, bytes_restored: 0, files: [] }, 'restoring'),
    offloadRestoreOperation(undefined, 'restoring'),
    offloadRestoreOperation({}, 'restoring'),
    restoreOperation('done'),
  ].map((operation) => status(operation)) });
  await ready(harness);
  const { container, summary, details, rows } = restoreReportNodes(harness);
  for (const [name, count] of [['first.wav', 1], ['updated.wav', 2], ['next.wav', 3]]) {
    assert.strictEqual(rows.children.length, 1);
    assert.ok(rows.children[0].textContent.includes(name));
    assert.ok(summary.textContent.includes(count + ' MiB'));
    assert.ok(!harness.banner.querySelector('.backup-spinner').hidden);
    details.setAttribute('open', '');
    await harness.runTimer(800);
    assert.strictEqual(details.getAttribute('open'), null);
  }
  assert.ok(!container.hidden && details.hidden && rows.children.length === 0);
  assert.ok(summary.textContent.includes('0 GiB'));
  await harness.runTimer(800);
  assert.ok(container.hidden && details.hidden);
  assert.strictEqual(summary.textContent, '');
  assert.strictEqual(rows.children.length, 0);
  await harness.runTimer(800);
  assert.ok(!container.hidden && details.hidden);
  assert.ok(!summary.textContent.includes('0 GiB'));
  await harness.runTimer(800);
  assert.ok(container.hidden);
  assert.strictEqual(summary.textContent, '');
  assert.strictEqual(rows.children.length, 0);
  assert.ok(harness.banner.querySelector('.backup-spinner').hidden);
  assert.ok(harness.calls.every((call) => call.url === '/app/backup/status' && !call.request.method));
});

asyncCase('held summary-only restore reports keep zero and explain actual refusal tokens', async () => {
  const explanations = [];
  for (const reason of ['archive_unbound', 'restore_held', 'future_reason', '__proto__']) {
    const harness = createHarness({ statusQueue: [status(offloadRestoreOperation({ files_restored: 0, bytes_restored: 0 }, 'refused', reason))] });
    const missingCopy = [];
    harness.window.logError = (error) => missingCopy.push(error);
    await ready(harness);
    const { container, summary, details, rows } = restoreReportNodes(harness);
    assert.ok(!container.hidden && details.hidden);
    assert.ok(summary.textContent.includes('0 GiB'));
    assert.strictEqual(rows.children.length, 0);
    const explanation = harness.root.querySelector('[data-operation-error]').textContent;
    assert.ok(explanation.length > 0 && !explanation.includes(reason));
    explanations.push(explanation);
    assert.strictEqual(missingCopy.length, reason === 'archive_unbound' || reason === 'restore_held' ? 0 : 1);
    assert.ok(harness.banner.querySelector('.backup-spinner').hidden);
    assert.deepStrictEqual(harness.calls, []);
  }
  assert.notStrictEqual(explanations[0], explanations[1]);
  assert.notStrictEqual(explanations[0], explanations[2]);
  assert.strictEqual(explanations[2], explanations[3]);
});

asyncCase('contended admission is visible and recoverable with no invented operation', async () => {
  const harness = createHarness({
    statusQueue: [Object.assign(status(), { cleanup_admission: 'contended' })],
    respond(call) {
      if (call.url === '/app/backup/api/cleanup/retry') {
        return response({ operation: null, cleanup_admission: 'ready' });
      }
    },
  });
  await ready(harness);
  assert.ok(harness.banner.hidden);
  assert.ok(!harness.cleanupBanner.hidden);
  assert.strictEqual(harness.cleanup.textContent, 'check again');
  click(harness.cleanup);
  await settle();
  assert.ok(harness.banner.hidden && harness.cleanupBanner.hidden);
  assert.deepStrictEqual(harness.calls.map((call) => call.url), ['/app/backup/api/cleanup/retry']);
});

for (const cancelFailure of ['busy', 'fetch']) {
asyncCase(cancelFailure + ' cancellation keeps the hosted attempt and discovers pending cleanup', async () => {
  const sequence = hostedSequence(Object.assign(
    status(restoreOperation('cleanup_pending', 'cleanup_pending')), { cleanup_admission: 'pending' }));
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/restore-hosted/cancel') {
      return cancelFailure === 'busy' ? response({ reason_code: 'backup_busy' }, 400)
        : Promise.reject(new Error('temporary cancellation fetch failure'));
    }
    if (call.url === '/app/backup/restore-hosted/arm') {
      return response(Object.assign(status(restoreOperation('restoring')), { cleanup_admission: 'ready' }));
    }
    return sequence(call);
  } });
  await ready(harness);
  click(harness.showRestore);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  await startToPolling(harness);
  click(harness.panelCancel);
  await settle();
  assert.ok(!harness.restore.hidden);
  assert.ok(harness.primary.disabled || harness.primary.hidden);
  await harness.runTimer(800);
  assert.strictEqual(harness.root.getAttribute('data-state'), 'cleanup_pending');
  assert.ok(!harness.cleanupBanner.hidden && !harness.cleanup.disabled);
  assert.ok(!harness.outcome.textContent.includes('cancelled'));
  assert.strictEqual(harness.calls.filter((call) => call.url.endsWith('/activate')).length, 1);
});
}

asyncCase('Windows active state survives a fetch failure before cleanup publication', async () => {
  let reads = 0;
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') {
      reads += 1;
      if (reads === 2) return Promise.reject(new Error('temporary fetch failure'));
      return response(Object.assign(status(restoreOperation(reads === 1 ? 'restoring' : 'cleanup_pending')),
        { cleanup_admission: reads === 1 ? 'ready' : 'pending' }));
    }
  } });
  await ready(harness);
  await harness.runTimer(800);
  assert.strictEqual(harness.root.getAttribute('data-state'), 'restoring');
  await harness.runTimer(800);
  assert.strictEqual(harness.root.getAttribute('data-state'), 'cleanup_pending');
  assert.ok(!harness.cleanup.hidden);
});

for (const scope of ['own', 'global', 'contended']) {
  asyncCase(scope + ' waiting disables conflicting controls and restores their prior eligibility', async () => {
    const original = scope === 'own' ? restoreOperation('error', 'failed') : restoreOperation('done');
    const waiting = scope === 'own' ? restoreOperation('cleanup_pending', 'cleanup_pending') : original;
    const cleared = Object.assign(status(original), { enabled: true, cleanup_admission: 'ready' });
    const harness = createHarness({
      statusQueue: [Object.assign(status(waiting), { enabled: true, cleanup_admission: scope === 'contended' ? 'contended' : 'pending' })],
      respond(call) {
        if (call.url === '/app/backup/api/cleanup/retry' || call.url === '/app/backup/backup-now') {
          return response(cleared);
        }
      },
    });
    await ready(harness);
    assert.ok(harness.backupNow.disabled && harness.generateKey.disabled && harness.saveRetention.disabled);
    assert.ok(harness.rotateKey.disabled, 'previously ineligible remains disabled');
    assert.ok(!harness.viewKey.disabled && !harness.cleanup.disabled, 'read and recovery actions stay available');
    click(harness.backupNow);
    click(harness.generateKey);
    harness.retention.dispatchEvent(event('submit'));
    setKey(harness, 'synthetic recovery key');
    assert.ok(harness.primary.disabled, 'ordinary input render cannot reopen a blocked action');
    click(harness.primary);
    // Programmatic activation cannot bypass the delegated pending guard.
    harness.backupNow.disabled = false;
    click(harness.backupNow);
    await settle();
    assert.ok(harness.backupNow.disabled);
    assert.strictEqual(harness.calls.length, 0, 'no conflicting POST while waiting');
    click(harness.cleanup);
    await settle();
    assert.deepStrictEqual(harness.calls.map((call) => call.url), ['/app/backup/api/cleanup/retry']);
    assert.ok(!harness.backupNow.disabled && !harness.generateKey.disabled && !harness.saveRetention.disabled);
    assert.ok(harness.rotateKey.disabled, 'clearing pending does not enable an otherwise ineligible action');
    click(harness.backupNow);
    await settle();
    assert.deepStrictEqual(harness.calls.map((call) => call.url), ['/app/backup/api/cleanup/retry', '/app/backup/backup-now']);
  });
}

asyncCase('cleanup recovery sends only its explicit request and preserves the original failed result', async () => {
  let release;
  const harness = createHarness({
    statusQueue: [status(restoreOperation('cleanup_pending', 'cleanup_pending'))],
    respond(call) {
      if (call.url === '/app/backup/api/cleanup/retry') {
        return new Promise((resolve) => { release = resolve; });
      }
    },
  });
  await ready(harness);
  click(harness.cleanup);
  await settle();
  assert.ok(harness.cleanup.disabled);
  click(harness.cleanup);
  assert.deepStrictEqual(harness.calls.map((call) => call.url), ['/app/backup/api/cleanup/retry']);
  assert.strictEqual(harness.calls[0].request.method, 'POST');
  release(response(status(restoreOperation('error', 'failed'))));
  await settle();
  assert.ok(harness.cleanup.hidden);
  assert.strictEqual(harness.root.querySelector('[data-operation-phase]').textContent, "couldn't finish");
  assert.strictEqual(harness.calls.length, 1);
});

asyncCase('failed cleanup request keeps recovery available without rerunning an operation', async () => {
  const harness = createHarness({
    statusQueue: [status(restoreOperation('cleanup_pending', 'cleanup_pending'))],
    respond(call) {
      if (call.url === '/app/backup/api/cleanup/retry') {
        return Promise.reject(new Error('temporary recovery failure'));
      }
    },
  });
  await ready(harness);
  click(harness.cleanup);
  await settle();
  assert.strictEqual(harness.root.getAttribute('data-state'), 'cleanup_pending');
  assert.ok(!harness.cleanup.hidden && !harness.cleanup.disabled);
  assert.deepStrictEqual(harness.calls.map((call) => call.url), ['/app/backup/api/cleanup/retry']);
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

asyncCase('restore prepare consent-window expiry has its own copy', async () => {
  const harness = createHarness({ respond: hostedSequence(status(restoreOperation('error', 'restore_prepare_expired'))) });
  await ready(harness);
  selectOperated(harness);
  setKey(harness, 'recovery key');
  await startToPolling(harness);
  await harness.runTimer(800);
  assert.strictEqual(harness.outcome.textContent, 'the approval took too long. try again.');
  assert.notStrictEqual(harness.outcome.textContent, 'start with the recovery key. if it still fails, check the destination details.');
});

asyncCase('hosted recovery-key ARIA separates local validation from C3 failures', async () => {
  const harness = createHarness({ popupFactory: () => null });
  await ready(harness);
  click(harness.showRestore);
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
  assert.strictEqual(harness.root.querySelectorAll('[role="status"]').filter((element) => {
    for (let current = element; current; current = current.parentElement) {
      if (current.hidden) return false;
    }
    return true;
  }).length, 1);
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

// F-1: 'checked_subset' is restic's --read-data-subset selector. The last ISO
// week of the year selects the 52nd slice, and the sentence must still say one
// slice out of 52 rather than reading as a whole-backup verification.
asyncCase('the verification scope reports one slice of the data, never a snapshot count', async () => {
  const verified = Object.assign(status(), {
    enabled: true,
    last_verification: { time: 1769990000, status: 'ok', reason: null, checked_subset: '52/52', last_ok_time: 1769990000 },
  });
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') return response(verified);
  } });
  await settle();
  const subset = harness.root.querySelector('[data-last-verification-subset]');
  assert.ok(subset, 'the verification scope line is mounted');
  assert.ok(!subset.hidden, 'the verification scope line is shown');
  assert.strictEqual(subset.textContent, 'checked 1 of 52 parts of your backup.');
  assert.ok(!/snapshot/.test(subset.textContent), 'the scope line never counts snapshots');
});

async function lastBackupReason(lastBackup) {
  const payload = Object.assign(status(), { enabled: true, last_backup: lastBackup });
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') return response(payload);
  } });
  await settle();
  const reason = harness.root.querySelector('[data-last-backup-reason]')
    || harness.document.querySelector('[data-last-backup-reason]');
  return reason && !reason.hidden ? reason.textContent : '';
}

asyncCase('a hosted backup failure says why it failed', async () => {
  const text = await lastBackupReason({ time: 1769990000, status: 'error', error_reason: 'broker_unreachable' });
  assert.ok(text, 'a hosted failure reason is shown');
});

asyncCase('a backup that stored an incomplete copy says so', async () => {
  const text = await lastBackupReason({ time: 1769990000, status: 'ok', error_reason: 'incomplete' });
  assert.ok(/incomplete/.test(text), 'an incomplete copy is named');
  const clean = await lastBackupReason({ time: 1769990000, status: 'ok', error_reason: null });
  assert.strictEqual(clean, '', 'a clean backup shows no reason');
});

asyncCase('a failed attempt shows its own time and preserves an incomplete successful copy', async () => {
  const earlier = 1769990000;
  const later = 1770000000;
  const payload = { ...status(), enabled: true,
    last_backup: { status: 'error', time: later, error_reason: 'timeout', last_success: {
      status: 'ok', time: earlier, snapshot_id: 'stored-copy', error_reason: 'incomplete',
    } },
  };
  const harness = createHarness({ statusQueue: [payload] });
  await settle();
  const success = harness.root.querySelector('[data-last-successful-backup]');
  const caveat = harness.root.querySelector('[data-last-successful-backup-reason]');
  const latestReason = harness.root.querySelector('[data-last-backup-reason]');
  assert.strictEqual(harness.lastBackup.title, new Date(later * 1000).toLocaleString());
  assert.strictEqual(success.title, new Date(earlier * 1000).toLocaleString());
  assert.ok(!latestReason.hidden && latestReason.textContent);
  assert.ok(!caveat.hidden && caveat.textContent);
  const note = harness.root.querySelector('[data-backup-history-note]');
  assert.ok(!note.hidden && note.textContent, 'history has a visible coverage/destination qualification');
});

asyncCase('success history distinguishes intact legacy evidence from unknown and malformed values', async () => {
  for (const [lastBackup, hasDate] of [
    [{ status: 'ok', time: 1769990000 }, true],
    [{ status: 'error', time: 1770000000 }, false],
    [{ status: null, time: null }, false],
    [{ status: 'ok', time: 1769990000, last_success: null }, false],
    [{ status: 'error', last_success: { status: 'ok', time: 'yesterday' } }, false],
    [{ status: 'error', last_success: { status: 'ok', time: 1e30 } }, false],
  ]) {
    const harness = createHarness({ statusQueue: [{ ...status(), enabled: true, last_backup: lastBackup }] });
    await settle();
    const success = harness.root.querySelector('[data-last-successful-backup]');
    assert.strictEqual(Boolean(success.title), hasDate);
    assert.ok(success.textContent, 'absence/malformed history has a visible unknown state');
  }
});

asyncCase('refresh advances attempt history without presenting retained evidence as current protection', async () => {
  const earlier = 1769990000;
  const success = { status: 'ok', time: earlier, error_reason: null };
  const initial = { ...status(restoreOperation('restoring')), enabled: true,
    last_backup: { ...success, last_success: success },
  };
  const changed = { ...status(restoreOperation('restoring')), enabled: true,
    destination: { repository: 's3:other-bucket/other-backup', backend: 's3' },
    last_backup: { status: 'error', time: 1770000000, error_reason: 'timeout', last_success: success },
  };
  const harness = createHarness({ statusQueue: [initial, changed], respond(call, calls, queue) {
    if (call.url === '/app/backup/status' && !queue.length) return response({}, 503);
  } });
  await settle();
  const history = harness.root.querySelector('[data-last-successful-backup]');
  const initialTitle = history.title;
  await harness.runTimer(800);
  await settle();
  assert.strictEqual(history.title, initialTitle);
  assert.notStrictEqual(harness.lastBackup.title, initialTitle);
  assert.ok(!harness.root.querySelector('[data-backup-history-note]').hidden);
  await harness.runTimer(800);
  await settle();
  assert.strictEqual(history.title, initialTitle, 'failed refresh does not manufacture or erase historical success');
  assert.ok(!harness.root.querySelector('[data-backup-history-note]').hidden);
  const reloaded = createHarness({ statusQueue: [changed] });
  await settle();
  assert.strictEqual(reloaded.root.querySelector('[data-last-successful-backup]').title, initialTitle);
});

asyncCase('a single-part verification says it covered everything', async () => {
  const verified = Object.assign(status(), {
    enabled: true,
    last_verification: { time: 1769990000, status: 'ok', reason: null, checked_subset: '1/1', last_ok_time: 1769990000 },
  });
  const harness = createHarness({ respond(call) {
    if (call.url === '/app/backup/status') return response(verified);
  } });
  await settle();
  const subset = harness.root.querySelector('[data-last-verification-subset]');
  assert.strictEqual(subset.textContent, 'checked all of your backup this week.');
});

asyncCase('capable host unbound restore start assigns location, avoids window.open, and exposes reopen anchor', async () => {
  const sequence = hostedSequence(status(restoreOperation('restoring')));
  const harness = createHarness({
    capable: true,
    respond: sequence,
  });
  await ready(harness);
  click(harness.showRestore);
  selectOperated(harness);
  setKey(harness, 'valid recovery key');
  click(harness.primary);
  await settle();
  await harness.runTimer(0);
  await settle();
  assert.strictEqual(harness.popups.length, 0, 'capable host does not open popups');
  assert.strictEqual(harness.location.assigned, 'https://services.solstone.app/enable/backup?nonce=nonce&intent=restore');
  assert.ok(!harness.reopen.hidden, 'reopen anchor is visible while waiting for activation');
  assert.strictEqual(harness.reopen.getAttribute('href'), 'https://services.solstone.app/enable/backup?nonce=nonce&intent=restore');

  // Clicking reopen anchor navigates top-level location
  harness.location.assigned = null;
  click(harness.reopen);
  assert.strictEqual(harness.location.assigned, 'https://services.solstone.app/enable/backup?nonce=nonce&intent=restore');
});

asyncCase('hosted restore key exchange rejecting unsafe portal URL cleans up attempt and shows error', async () => {
  const harness = createHarness({
    respond(call) {
      if (call.url === '/app/backup/restore-hosted/prepare') return response({ capability: 'cap123' });
      if (call.url === '/app/backup/restore-hosted/key') return response({ portal_url: 'javascript:alert(1)' });
      if (call.url === '/app/backup/restore-hosted/cancel') return response({ success: true });
      if (call.url === '/app/backup/status') return response(status());
      if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
      throw new Error('unexpected fetch ' + call.url);
    },
  });
  await ready(harness);
  click(harness.showRestore);
  selectOperated(harness);
  setKey(harness, 'key for bad portal');
  click(harness.primary);
  await settle();
  assert.strictEqual(harness.location.assigned, null, 'no location assigned on bad portal');
  assert.strictEqual(harness.popups.length, 1, 'browser popup was created');
  assert.strictEqual(harness.popups[0].closed, true, 'browser popup was closed');
  assert.ok(!harness.outcome.hidden, 'outcome error is displayed');
});

asyncCase('cancel during restore-hosted prepare releases gate and drops attempt', async () => {
  let prepareResolve;
  const harness = createHarness({
    respond(call) {
      if (call.url === '/app/backup/restore-hosted/prepare') {
        return new Promise((resolve) => { prepareResolve = resolve; });
      }
      if (call.url === '/app/backup/restore-hosted/cancel') return response(status());
      if (call.url === '/app/backup/status') return response(status());
      if (call.url === '/app/backup/offload/status') return response({ success: true, offload: {}, days: [] });
      throw new Error('unexpected fetch ' + call.url);
    },
  });
  await ready(harness);
  click(harness.showRestore);
  selectOperated(harness);
  setKey(harness, 'key for cancel test');
  click(harness.primary);
  await settle();
  // Cancel while prepare is inflight
  click(harness.attemptCancel);
  await settle();
  // Late resolution of prepare
  prepareResolve(response({ capability: 'late_cap' }));
  await settle();
  // Verify cancel POST body has capability: 'late_cap' and no key/arm/activate was called
  const cancelCalls = harness.calls.filter((c) => c.url === '/app/backup/restore-hosted/cancel');
  assert.strictEqual(cancelCalls.length, 1, 'cancel was called for late prepare');
  assert.deepStrictEqual(JSON.parse(cancelCalls[0].request.body), { capability: 'late_cap' });
  assert.strictEqual(harness.calls.filter((c) => c.url.includes('/key')).length, 0, 'no key call after cancel');
  assert.strictEqual(harness.calls.filter((c) => c.url.includes('/arm')).length, 0, 'no arm call after cancel');
  assert.strictEqual(harness.calls.filter((c) => c.url.includes('/activate')).length, 0, 'no activate call after cancel');
});

for (const boundary of ['key', 'arm', 'activate']) {
  for (const rejects of [false, true]) {
    asyncCase(`late ${boundary} ${rejects ? 'failure' : 'success'} cannot change replacement restore`, async () => {
      let completeOld;
      let rejectOld;
      let preparations = 0;
      const url = (id) => `https://services.solstone.app/enable/backup?nonce=${id}&intent=restore`;
      const harness = createHarness({ capable: true, respond(call) {
        const body = call.request.body && JSON.parse(call.request.body);
        if (call.url.endsWith('/prepare')) return response({ capability: ++preparations === 1 ? 'A' : 'B' });
        if (call.url.endsWith('/' + boundary) && body.capability === 'A') {
          return new Promise((resolve, reject) => { completeOld = resolve; rejectOld = reject; });
        }
        if (call.url.endsWith('/key')) return response({ portal_url: url(body.capability) });
        if (call.url.endsWith('/arm') || call.url.endsWith('/activate')) {
          return response(status(Object.assign(restoreOperation('restoring'), { portal_url: url(body.capability) })));
        }
        if (call.url.endsWith('/cancel')) return response(status());
      } });
      await ready(harness);
      click(harness.showRestore);
      selectOperated(harness);
      setKey(harness, 'local key A');
      click(harness.primary);
      await settle();
      if (boundary !== 'key') { await harness.runTimer(0); await settle(); }
      assert.ok(completeOld, 'old request reached the held boundary');
      click(harness.attemptCancel);
      await settle();
      setKey(harness, 'local key B');
      click(harness.primary);
      await settle();
      await harness.runTimer(0);
      await settle();
      const before = harness.calls.length;
      const outcome = harness.outcome.textContent;
      const destination = harness.location.assigned;
      if (rejects) rejectOld(new Error('old request failed'));
      else completeOld(response(boundary === 'key' ? { portal_url: url('A') }
        : status(Object.assign(restoreOperation('error', 'auth_failed'), { portal_url: url('A') }))));
      await settle(40);
      assert.strictEqual(harness.location.assigned, destination, 'old reply cannot launch');
      assert.strictEqual(harness.outcome.textContent, outcome, 'old reply cannot set error or reset UI');
      assert.strictEqual(harness.calls.length, before, 'old reply cannot poll, advance or cancel replacement');
      assert.strictEqual(harness.calls.filter(c => c.url.endsWith('/cancel')).length, 1);
      assert.deepStrictEqual(JSON.parse(harness.calls.find(c => c.url.endsWith('/cancel')).request.body), { capability: 'A' });
    });
  }
}

asyncCase('terminal arm reply stops activation and closes its browser reservation', async () => {
  const harness = createHarness({ respond(call) {
    if (call.url.endsWith('/prepare')) return response({ capability: 'terminal' });
    if (call.url.endsWith('/key')) return response({ portal_url: 'https://services.solstone.app/enable/backup?nonce=terminal' });
    if (call.url.endsWith('/arm')) return response(status(restoreOperation('error', 'restore_prepare_expired')));
  } });
  await ready(harness);
  click(harness.showRestore);
  selectOperated(harness);
  setKey(harness, 'local key');
  click(harness.primary);
  await settle();
  await harness.runTimer(0);
  await settle();
  assert.strictEqual(harness.calls.filter(c => c.url.endsWith('/activate')).length, 0);
  assert.strictEqual(harness.popups[0].closed, true);
  assert.strictEqual(harness.reopen.hidden, true);
});

for (const failure of ['reject', 'throw']) {
  asyncCase(`outside dispatch ${failure} releases only its own restore capability before arm`, async () => {
    const harness = createHarness({ capable: true, respond(call) {
      if (call.url.endsWith('/prepare')) return response({ capability: 'dispatch' });
      if (call.url.endsWith('/key')) return response({ portal_url: 'https://services.solstone.app/enable/backup?nonce=dispatch' });
      if (call.url.endsWith('/cancel')) return response(status());
    } });
    await ready(harness);
    harness.window.solstoneOutside.navigateReserved = () => {
      if (failure === 'throw') throw new Error('dispatch failed');
      return false;
    };
    click(harness.showRestore);
    selectOperated(harness);
    setKey(harness, 'local key');
    click(harness.primary);
    await settle();
    await harness.runTimer(0);
    await settle();
    assert.strictEqual(harness.calls.filter(c => c.url.endsWith('/arm') || c.url.endsWith('/activate')).length, 0);
    assert.deepStrictEqual(JSON.parse(harness.calls.find(c => c.url.endsWith('/cancel')).request.body), { capability: 'dispatch' });
    assert.ok(!harness.outcome.hidden && harness.outcome.classList.contains('is-error'));
  });
}

for (const phase of ['restoring', 'done', 'error', 'cleanup_pending', 'refused', 'needs_subscription']) {
  asyncCase(`reopen after reload follows server operation eligibility: ${phase}`, async () => {
    const operation = Object.assign(restoreOperation(phase), { portal_url: 'https://services.solstone.app/enable/backup?nonce=reloaded' });
    const harness = createHarness({ capable: true, statusQueue: [status(operation)] });
    await ready(harness);
    click(harness.showRestore);
    selectOperated(harness);
    assert.strictEqual(harness.reopen.hidden, phase !== 'restoring');
    if (phase === 'restoring') {
      const before = harness.calls.length;
      click(harness.reopen);
      assert.strictEqual(harness.location.assigned, operation.portal_url);
      assert.strictEqual(harness.calls.length, before, 'reopen repeats no preparation');
    } else assert.strictEqual(harness.reopen.href, '');
  });
}

asyncCase('general backup summary replaces a reassuring result after failed polling and recovers on reload', async () => {
  let reads = 0;
  const success = { status: 'ok', time: 1770000000, snapshot_id: 'copy-a' };
  const harness = createHarness({ respond(call) {
    if (call.url !== '/app/backup/status') return undefined;
    reads += 1;
    if (reads === 2) throw new Error('temporary failed read');
    return response({ ...status(reads < 3 ? restoreOperation('restoring') : restoreOperation('done')),
      enabled: true, last_backup: success });
  } });
  await ready(harness);
  const summary = harness.root.querySelector('[data-backup-summary]');
  assert.strictEqual(summary.dataset.tone, 'neutral');
  await harness.runTimer(800);
  assert.strictEqual(summary.dataset.tone, 'unavailable');
  const reloaded = createHarness({ statusQueue: [{ ...status(), enabled: true, last_backup: success }] });
  await ready(reloaded);
  assert.strictEqual(reloaded.root.querySelector('[data-backup-summary]').dataset.tone, 'neutral');
});

asyncCase('a malformed fresh status cannot borrow successful facts from the previous status', async () => {
  const success = { status: 'ok', time: 1770000000, snapshot_id: 'copy-a' };
  for (const malformed of [{ success: true }, { success: true, enabled: true }]) {
    const harness = createHarness({ statusQueue: [
      { ...status(restoreOperation('restoring')), enabled: true, last_backup: success },
      malformed,
      { ...status(restoreOperation('done')), enabled: true, last_backup: success },
    ] });
    await ready(harness);
    const summary = harness.root.querySelector('[data-backup-summary]');
    assert.strictEqual(summary.dataset.tone, 'neutral');
    await harness.runTimer(800);
    assert.strictEqual(summary.dataset.tone, 'unavailable');
    await harness.runTimer(800);
    assert.strictEqual(summary.dataset.tone, 'neutral');
  }
});

asyncCase('teardown confirmation gate requires exact phrase and POSTs with no body', async () => {
  const backupJsSource = fs.readFileSync(path.join(crateDir, 'assets', 'backup.js'), 'utf8');
  const match = /"teardown_confirm_phrase":\s*"([^"]+)"/.exec(backupJsSource);
  assert.ok(match, 'backup.js must contain teardown_confirm_phrase');
  const phrase = match[1];

  let teardownCalled = false;
  let teardownRequestBody = 'uncalled';
  const harness = createHarness({
    statusQueue: [{ ...status(), enabled: true }],
    respond(call) {
      if (call.url === '/app/backup/teardown') {
        teardownCalled = true;
        teardownRequestBody = call.request.body;
        return response({ success: true, operation: { kind: 'teardown', phase: 'starting' } });
      }
    },
  });
  await ready(harness);

  click(harness.teardownOpen);
  await settle();

  assert.strictEqual(harness.teardownGate.hidden, false);
  assert.strictEqual(harness.teardownConfirm.disabled, true);

  harness.teardownInput.value = '';
  harness.teardownInput.dispatchEvent(event('input'));
  assert.strictEqual(harness.teardownConfirm.disabled, true);
  click(harness.teardownConfirm);
  assert.strictEqual(teardownCalled, false);

  harness.teardownInput.value = 'wrong';
  harness.teardownInput.dispatchEvent(event('input'));
  assert.strictEqual(harness.teardownConfirm.disabled, true);
  click(harness.teardownConfirm);
  assert.strictEqual(teardownCalled, false);

  harness.teardownInput.value = phrase;
  harness.teardownInput.dispatchEvent(event('input'));
  assert.strictEqual(harness.teardownConfirm.disabled, false);

  click(harness.teardownConfirm);
  await settle();
  assert.strictEqual(teardownCalled, true);
  assert.strictEqual(teardownRequestBody, undefined);
});

async function runAsyncCases() {
  for (const runCase of asyncCases) await runCase();
  console.log('DOM CASES: ' + cases + ' passed');
}

runAsyncCases().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
