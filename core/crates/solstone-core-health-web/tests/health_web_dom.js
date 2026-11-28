// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { loadWindowCatalog } = require('../../solstone-core-convey-shell/tests/journal_catalog_dom.js');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');
const dateFormatSource = fs.readFileSync(path.resolve(crateDir, '../solstone-core-convey-shell/assets/static/date_format.js'), 'utf8');
const focusSource = fs.readFileSync(path.resolve(crateDir, '../solstone-core-convey-shell/assets/static/focus.js'), 'utf8');

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
      // A loop, not a closure: a closure over `next` keeps the matches, and
      // through `parentNode` their tree, while the engine compiles it.
      const pending = [candidate];
      while (pending.length) {
        const node = pending.pop();
        if (node !== candidate && matchesSelector(node, piece)) next.push(node);
        const kids = node.children;
        for (let i = kids.length - 1; i >= 0; i -= 1) pending.push(kids[i]);
      }
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
  get isConnected() { return this === this.ownerDocument.body || Boolean(this.parentElement?.isConnected); }
  set id(value) { this.setAttribute('id', value); }
  get href() { return this.getAttribute('href') || ''; }
  set href(value) { this.setAttribute('href', value); }
  get className() { return this.getAttribute('class') || ''; }
  set className(value) { this.setAttribute('class', value); }
  get parentNode() { return this.parentElement; }
  get nextSibling() {
    if (!this.parentElement) return null;
    const siblings = this.parentElement.children;
    const idx = siblings.indexOf(this);
    return idx >= 0 && idx < siblings.length - 1 ? siblings[idx + 1] : null;
  }
  get open() { return this.hasAttribute('open'); }
  set open(value) { if (value) this.setAttribute('open', ''); else this.removeAttribute('open'); }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(value) { if (value) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  get textContent() {
    if (this.children.length > 0) {
      return this.children.map((c) => c.textContent).join('');
    }
    return this._textContent;
  }
  set textContent(value) {
    this.children = [];
    this._textContent = String(value);
  }
  set innerHTML(value) {
    assert.strictEqual(value, '', 'this DOM harness uses element APIs for content');
    this.replaceChildren();
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'class') this.classList.setFromString(value);
    if (name.startsWith('data-')) this.dataset[dataKey(name)] = String(value);
  }

  getAttribute(name) {
    if (name.startsWith('data-')) {
      return Object.prototype.hasOwnProperty.call(this.dataset, dataKey(name)) ? this.dataset[dataKey(name)] : null;
    }
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  hasAttribute(name) { return this.getAttribute(name) !== null; }

  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'class') this.classList.setFromString('');
    if (name.startsWith('data-')) delete this.dataset[dataKey(name)];
  }

  appendChild(child) {
    if (child.tagName === '#DOCUMENT-FRAGMENT') {
      child.children.splice(0).forEach((node) => this.appendChild(node));
      return child;
    }
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  append(...children) {
    children.forEach((child) => this.appendChild(
      typeof child === 'string' ? this.ownerDocument.createTextNode(child) : child));
  }
  prepend(child) {
    if (child.parentElement) child.parentElement.removeChild(child);
    child.parentElement = this;
    this.children.unshift(child);
  }

  replaceChildren(...children) {
    this.children.forEach((child) => { child.parentElement = null; });
    this.children = [];
    this.append(...children);
  }

  removeChild(child) {
    this.children = this.children.filter(item => item !== child);
    child.parentElement = null;
    return child;
  }

  remove() { if (this.parentElement) this.parentElement.removeChild(this); }

  insertBefore(node, child) {
    if (node.parentElement) node.parentElement.removeChild(node);
    node.parentElement = this;
    if (!child) {
      this.children.push(node);
    } else {
      const idx = this.children.indexOf(child);
      if (idx === -1) this.children.push(node);
      else this.children.splice(idx, 0, node);
    }
    return node;
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
  removeEventListener(type, listener) {
    this.listeners[type] = (this.listeners[type] || []).filter((candidate) => candidate !== listener);
  }

  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    if (event.bubbles && !event.cancelBubble && this.parentElement) this.parentElement.dispatchEvent(event);
    return !event.defaultPrevented;
  }

  focus() { this.ownerDocument.activeElement = this; }
  scrollIntoView() { this.scrolledIntoView = true; }
  // A box while it and every ancestor are shown and it is in the page.
  getClientRects() {
    for (let node = this; node; node = node.parentElement) if (node.hidden) return [];
    return this.isConnected ? [{}] : [];
  }
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
  createDocumentFragment() { return new Element('#document-fragment', this); }
  createTextNode(text) {
    const node = new Element('#text', this);
    node.textContent = text;
    return node;
  }
  getElementById(id) { return this.querySelector('#' + id); }
  querySelector(selector) {
    if (selector === 'body') return this.body;
    return this.documentElement.querySelector(selector);
  }
  querySelectorAll(selector) { return this.documentElement.querySelectorAll(selector); }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
  removeEventListener(type, listener) {
    this.listeners[type] = (this.listeners[type] || []).filter((candidate) => candidate !== listener);
  }
  dispatchEvent(event) {
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    return true;
  }
}

function loadHealthSource() {
  return fs.readFileSync(path.join(crateDir, 'assets/static/health.js'), 'utf8');
}

function createHealthDOM() {
  const doc = new Document();
  const backlogVerdict = doc.createElement('div');
  backlogVerdict.id = 'backlogVerdict';

  const verdictLine = doc.createElement('p');
  verdictLine.setAttribute('class', 'backlog-verdict-line');
  backlogVerdict.appendChild(verdictLine);

  const unfinishedLine = doc.createElement('p');
  unfinishedLine.setAttribute('class', 'backlog-unfinished-line');
  unfinishedLine.hidden = true;
  backlogVerdict.appendChild(unfinishedLine);

  const searchLine = doc.createElement('p');
  searchLine.setAttribute('class', 'backlog-search-line');
  searchLine.hidden = true;
  backlogVerdict.appendChild(searchLine);

  doc.body.appendChild(backlogVerdict);

  const stuckRowsContainer = doc.createElement('div');
  stuckRowsContainer.setAttribute('data-backlog-stuck-rows', '');
  const rowsHost = doc.createElement('div');
  rowsHost.setAttribute('data-backlog-rows', '');
  stuckRowsContainer.appendChild(rowsHost);
  doc.body.appendChild(stuckRowsContainer);

  return doc;
}

let cases = 0;
const asyncCases = [];
function test(name, fn) {
  cases += 1;
  asyncCases.push(async () => {
    try {
      await fn();
    } catch (err) {
      console.error(`FAILED: ${name}`);
      throw err;
    }
  });
}

function loadHealthCatalog(context) {
  loadWindowCatalog(context);
  vm.runInContext(sourceBlock(loadHealthSource(), 'function renderCount(', 'function logHealthError('), context);
}

function extractRenderFunctions(source) {
  // Extract renderBacklogState and renderBacklogVerdict
  const start = source.indexOf('function renderBacklogVerdict(');
  const end = source.indexOf('function renderMediaBacklog(');
  assert.ok(start >= 0 && end > start, 'renderBacklog functions found in health.js');
  return source.slice(start, end);
}

const healthCode = extractRenderFunctions(loadHealthSource());

function createEnvironment() {
  const doc = createHealthDOM();
  const context = vm.createContext({
    reportHealthReady() {},
    document: doc,
    state: {},
    renderHeldWork: () => {},
    window: { createViewReady: () => () => {},
      location: { href: '' },
    },
    console,
    encodeURIComponent,
    Number,
    String,
    Array,
    Object,
  });
  loadHealthCatalog(context);
  vm.runInContext(dateFormatSource, context);
  context.window.JournalFormat.day = (d) => `Apr 3, 2026`;

  const glue = `
    let backlogCopy = {};
    function clearHealthStateError() {}
    ${healthCode}
  `;
  vm.runInContext(glue, context);
  return { doc, context };
}

test('unfinished activities renders single day with template_one', async () => {
  const { doc, context } = createEnvironment();
  const payload = {
    verdict: "your journal's caught up.",
    unfinished_activities: { activities: 1, day_count: 1, oldest_day: "20260403" },
    copy: {
      unfinished_template_one: "an activity from {day} couldn't finish processing",
      unfinished_template_many_one_day: "{n} activities from {day} couldn't finish processing",
      unfinished_template_many_days: "{n} activities couldn't finish processing. oldest: {day}",
    },
  };
  context.payload = payload;
  vm.runInContext('renderBacklogState(payload, null);', context);

  const unfinishedLine = doc.querySelector('#backlogVerdict .backlog-unfinished-line');
  assert.strictEqual(unfinishedLine.hidden, false);
  const link = unfinishedLine.querySelector('a');
  assert.ok(link, 'link is rendered');
  assert.strictEqual(link.textContent, "an activity from Apr 3, 2026 couldn't finish processing →");
  assert.strictEqual(link.getAttribute('href'), '/app/transcripts/20260403');
});

test('unfinished activities with missing template renders nothing', async () => {
  const { doc, context } = createEnvironment();
  const payload = {
    verdict: "your journal's caught up.",
    unfinished_activities: { activities: 1, day_count: 1, oldest_day: "20260403" },
    copy: {}, // No templates
  };
  context.payload = payload;
  vm.runInContext('renderBacklogState(payload, null);', context);

  const unfinishedLine = doc.querySelector('#backlogVerdict .backlog-unfinished-line');
  assert.strictEqual(unfinishedLine.hidden, true);
  assert.strictEqual(unfinishedLine.children.length, 0);
});

test('unfinished activities renders multiple activities across multiple days', async () => {
  const { doc, context } = createEnvironment();
  const payload = {
    verdict: "your journal's caught up.",
    unfinished_activities: { activities: 3, day_count: 2, oldest_day: "20260401" },
    copy: {
      unfinished_template_many_days: "{n} activities couldn't finish processing. oldest: {day}",
    },
  };
  context.payload = payload;
  vm.runInContext('renderBacklogState(payload, null);', context);

  const unfinishedLine = doc.querySelector('#backlogVerdict .backlog-unfinished-line');
  assert.strictEqual(unfinishedLine.hidden, false);
  const link = unfinishedLine.querySelector('a');
  assert.ok(link);
  assert.strictEqual(link.textContent, "3 activities couldn't finish processing. oldest: Apr 3, 2026 →");
});

test('search index line renders when present and hides when absent', async () => {
  const { doc, context } = createEnvironment();
  const searchIndex = {
    state: 'failing',
    text: "some journal updates couldn't be added to search.",
  };
  context.payload = { verdict: "your journal's caught up." };
  context.searchIndex = searchIndex;
  vm.runInContext('renderBacklogState(payload, searchIndex);', context);

  const searchLine = doc.querySelector('#backlogVerdict .backlog-search-line');
  assert.strictEqual(searchLine.hidden, false);
  assert.strictEqual(
    searchLine.textContent,
    "some journal updates couldn't be added to search.",
  );

  vm.runInContext('renderBacklogState(payload, { state: "unknown", text: "" });', context);
  assert.strictEqual(searchLine.hidden, true);

  vm.runInContext('renderBacklogState(payload, null);', context);
  assert.strictEqual(searchLine.hidden, true);
});

function extractGlanceFunction(source) {
  const start = source.indexOf('function queuedWorkHolds(');
  const end = source.indexOf('function formatGlanceSentence(');
  assert.ok(start >= 0 && end > start, 'selectGlanceSentence found in health.js');
  return source.slice(start, end);
}

function glanceFor(crashed, staleHeartbeats, searchIndex = null, attention = {}) {
  const context = vm.createContext({
    reportHealthReady() {}, Array, Object, String, Number, Math, Set, Map, window: {} });
  loadHealthCatalog(context);
  context.crashedEntries = crashed;
  context.staleHeartbeats = staleHeartbeats;
  context.searchIndex = searchIndex;
  context.brainSnapshot = attention.brain || null;
  context.deviceVerdict = attention.device || null;
  context.clients = attention.clients || [];
  vm.runInContext(`
    const connectError = false;
    const GLANCE_DEVICES_ACTION = {};
    const STALE_MS = 1;
    const SERVICE_NAMES = {};
    function serviceName(internal) { return String(internal || '').replace(/[_:-]+/g, ' '); }
    function selectDeviceVerdict() { return deviceVerdict; }
    function relativeTime() { return ''; }
    function ageAgo() { return ''; }
    const state = {
      agents: new Map(), imports: new Map(), clients: new Map(clients), services: new Map([['observe', {}]]),
      crashed: new Map(crashedEntries.map(name => [name, {}])),
      health: { stale_heartbeats: staleHeartbeats },
      searchIndex,
      connected: true, heldWorkCurrent: true, heldWorkSnapshot: null,
      backupView: { tone: 'neutral' },
    };
    ${extractGlanceFunction(loadHealthSource())}
    result = selectGlanceSentence(state, 0);
  `, context);
  return context.result;
}

test('a stale check-in is not counted as a failing service', async () => {
  const both = glanceFor(['observe'], ['work-laptop (/home/owner/journal)']);
  assert.strictEqual(both.key, 'HEALTH_GLANCE_SERVICE_ATTENTION');
  assert.ok(!both.vars.services.includes('laptop'), both.vars.services);

  const staleOnly = glanceFor([], ['work-laptop (/home/owner/journal)']);
  assert.strictEqual(staleOnly.key, 'HEALTH_GLANCE_CHECKIN_STALE');
  assert.strictEqual(staleOnly.vars.run, 'work-laptop (/home/owner/journal)');

  const none = glanceFor([], []);
  assert.strictEqual(none.key, 'HEALTH_GLANCE_OK');
});

test('failing search prevents an otherwise healthy headline; catching up does not', async () => {
  const failing = { state: 'failing', text: 'search needs attention' };
  const selection = glanceFor([], [], failing);
  assert.notStrictEqual(selection.key, 'HEALTH_GLANCE_OK');
  assert.strictEqual(selection.vars.headline, failing.text);
  for (const searchState of ['ok', 'behind', 'building']) {
    assert.strictEqual(glanceFor([], [], { state: searchState, text: 'search line' }).key, 'HEALTH_GLANCE_OK');
  }
  assert.strictEqual(glanceFor(['observe'], [], { state: 'failing', text: 'search needs attention' }).key, 'HEALTH_GLANCE_SERVICE_ATTENTION');
  const search = { state: 'failing', text: 'search needs attention' };
  const thinking = glanceFor([], [], search, { brain: { state: 'blocked', headline: 'thinking needs attention' } });
  assert.strictEqual(thinking.vars.headline, 'thinking needs attention');
  const device = { key: 'HEALTH_GLANCE_DEVICE_FAILING', vars: { device: 'work laptop' } };
  assert.strictEqual(glanceFor([], [], search, { device }).key, device.key);
  const quietClient = glanceFor([], [], search, { clients: [['work laptop', { lastSeen: -10 }]] });
  assert.strictEqual(quietClient.key, 'HEALTH_GLANCE_CLIENT_SILENT');
});

test('folder privacy needs an explicit choice and quiets only after it is saved', async () => {
  const { context, doc } = createEnvironment();
  const section = doc.createElement('section');
  section.id = 'storagePrivacy';
  const message = doc.createElement('p'); message.setAttribute('data-storage-privacy-message', '');
  const button = doc.createElement('button'); button.setAttribute('data-storage-privacy-consent', '');
  const result = doc.createElement('p'); result.setAttribute('data-storage-privacy-result', '');
  section.appendChild(message); section.appendChild(button); section.appendChild(result); doc.body.appendChild(section);
  context.state = {};
  context.updateStatusSummary = () => {};
  const source = loadHealthSource();
  vm.runInContext(sourceBlock(source, 'function renderStoragePrivacy(', 'let timeoutFired'), context);
  const privacy = { private: false, consented: false, filesystem_type: 'fat', message: 'unprotected' };
  context.privacy = privacy;
  let posts = 0;
  context.getJson = async (url, options) => {
    posts += 1;
    assert.strictEqual(url, '/app/health/api/storage-privacy/consent');
    assert.strictEqual(options.method, 'POST');
    assert.deepStrictEqual(JSON.parse(options.body), { filesystem_type: 'fat' });
    return { ...privacy, consented: true, message: 'chosen' };
  };
  vm.runInContext('renderStoragePrivacy(privacy)', context);
  assert.strictEqual(section.hidden, false);
  assert.strictEqual(button.hidden, false);
  assert.strictEqual(posts, 0);
  await button.onclick();
  assert.strictEqual(posts, 1);
  assert.strictEqual(button.hidden, true);
  assert.strictEqual(context.state.storagePrivacy.consented, true);
  vm.runInContext('renderStoragePrivacy(privacy)', context);
  context.getJson = async () => { throw { error: 'save failed' }; };
  await button.onclick();
  assert.strictEqual(result.hidden, false);
  assert.strictEqual(button.hidden, false);
  assert.strictEqual(context.state.storagePrivacy.consented, false);
});

test('loaded search status reaches the headline and survives a later state-read error', async () => {
  const { context } = createEnvironment();
  const source = loadHealthSource();
  const loaderStart = source.indexOf('let healthStateRequest = null;');
  const loaderEnd = source.indexOf('let timeoutFired', loaderStart);
  const errorsStart = source.indexOf('function renderAgentErrorsState(');
  const errorsEnd = source.indexOf('function renderMediaBacklog(', errorsStart);
  assert.ok(loaderStart >= 0 && loaderEnd > loaderStart && errorsStart >= 0 && errorsEnd > errorsStart);
  context.state = {};
  context.elements = {};
  context.loadMediaBacklog = () => {};
  context.renderWorkProgress = () => {};
  context.seedAgentErrors = () => {};
  context.updateStatusSummary = () => { context.selection = glanceFor([], [], context.state.searchIndex); };
  context.renderHealthStateError = (error) => { context.readError = error; };
  const search = { state: 'failing', text: 'search needs attention' };
  context.getJson = async (route) => {
    assert.strictEqual(route, '/app/health/api/state');
    return { backlog: {}, search_index: search, agent_errors: {} };
  };
  vm.runInContext(source.slice(errorsStart, errorsEnd) + source.slice(loaderStart, loaderEnd), context);
  await vm.runInContext('loadHealthState()', context);
  assert.strictEqual(context.state.searchIndex, search);
  assert.notStrictEqual(context.selection.key, 'HEALTH_GLANCE_OK');
  assert.strictEqual(context.selection.vars.headline, search.text);
  const readError = new Error('state unavailable');
  context.getJson = async () => { throw readError; };
  await vm.runInContext('loadHealthState()', context);
  assert.strictEqual(context.readError, readError);
  assert.strictEqual(context.state.searchIndex, search);
});

function sourceBlock(source, startToken, endToken) {
  const start = source.indexOf(startToken);
  const end = source.indexOf(endToken, start);
  assert.ok(start >= 0 && end > start, `production block found: ${startToken}`);
  return source.slice(start, end);
}

function vitalsSweepEnvironment() {
  const source = loadHealthSource();
  const timers = [];
  const handlers = [];
  const context = vm.createContext({
    sweeps: 0,
    window: {
      appEvents: {
        listen() {},
        onConnectionState(fn) {
          handlers.push(fn);
          fn({ connected: false, state: 'disconnected' });
        },
      },
    },
    setTimeout: (fn, ms) => { timers.push({ fn, ms }); return timers.length; },
  });
  vm.runInContext(`
    let timeoutFired = false;
    function sweepUnresolvedSkeletons() { if (timeoutFired) return; timeoutFired = true; sweeps += 1; }
    function invalidateRealtime() {}
    function wireBacklogReprocessActions() {}
    function handleEvent() {}
    ${sourceBlock(source, '// The supervisor reports status every 5 s', '// Utility functions')}
    ${sourceBlock(source, 'function initHealthRealtime() {', 'function initHealth() {')}
    initHealthRealtime();
  `, context);
  const emit = (state) => handlers.forEach((fn) => fn({ connected: state === 'connected', state }));
  return { context, timers, emit };
}

test('vitals stay loading while the stream opens, and time out 7 s after it connects', async () => {
  const env = vitalsSweepEnvironment();
  assert.strictEqual(env.timers.length, 0, 'no clock before the stream is attempted');
  env.emit('connecting');
  assert.strictEqual(env.timers.length, 0, 'a slow stream open is still loading');
  assert.strictEqual(env.context.sweeps, 0);
  env.emit('connected');
  assert.strictEqual(env.timers.length, 1);
  assert.strictEqual(env.timers[0].ms, 7000);
  env.emit('connected');
  assert.strictEqual(env.timers.length, 1, 'armed once');
  env.timers[0].fn();
  assert.strictEqual(env.context.sweeps, 1);
});

test('vitals turn unavailable at once when the stream fails', async () => {
  const env = vitalsSweepEnvironment();
  env.emit('connecting');
  env.emit('disconnected');
  assert.strictEqual(env.context.sweeps, 1, 'a failed stream leaves nothing on its way');
  assert.strictEqual(env.timers.length, 0);
});

function createDeviceEnvironment() {
  const source = loadHealthSource();
  const doc = new Document();
  const elements = {};
  const observeCode = sourceBlock(source, 'function observeQuietState(', 'function updateClients(');
  const names = new Set(['registeredClientsCard', 'registeredClientsStrip',
    ...Array.from(observeCode.matchAll(/elements\.(\w+)/g), match => match[1])]);
  for (const name of names) {
    elements[name] = doc.createElement('div');
    elements[name].id = name;
    if (name !== 'registeredClientsStrip') doc.body.appendChild(elements[name]);
  }
  elements.registeredClientsCard.appendChild(elements.registeredClientsStrip);
  const observeCard = doc.createElement('div');
  observeCard.className = 'observe-card';
  doc.body.appendChild(observeCard);
  const heading = doc.createElement('p');
  heading.className = 'surface-state-heading';
  elements.observeEmpty.appendChild(heading);
  const clock = { now: 120000 };
  class ControlledDate extends Date { static now() { return clock.now; } }
  const state = {
    registeredClients: null, registeredClientsFailed: false, registeredClientsObservedAt: null,
    clients: new Map(), localHost: 'local', hostCapabilities: {desktop_capture: true}, agents: new Map(), imports: new Map(),
    services: new Map([['observe', {}]]), crashed: new Map(), health: {}, connected: true,
    heldWorkCurrent: true, heldWorkSnapshot: null,
    backupView: { tone: 'neutral' },
  };
  const context = vm.createContext({
    reportHealthReady() {},
    document: doc, elements, state, Date: ControlledDate, clock, console: { warn() {} },
    window: { createViewReady: () => () => {}, location: { hash: '' } },
    requestAnimationFrame: fn => fn(), CustomEvent: class { constructor(type) { this.type = type; } },
    STALE_MS: 30000, brainSnapshot: null, connectError: false, updateClients() {},
    serviceName: value => value, relativeTime: value => String(value), ageAgo: value => String(value),
    fetch: async () => ({ ok: true, json: async () => ({ clients: [] }) }),
  });
  loadHealthCatalog(context);
  vm.runInContext(dateFormatSource, context);
  context.updateStatusSummary = () => {
    context.selection = vm.runInContext('selectGlanceSentence(state, Date.now())', context);
  };
  vm.runInContext([
    sourceBlock(source, 'const HEALTH_GLANCE_COPY =', 'let brainSnapshot ='),
    sourceBlock(source, 'function registeredClientName(', 'function formatGlanceSentence('),
    sourceBlock(source, 'const sinceDay =', 'async function requestBacklogReprocess('),
    observeCode,
    sourceBlock(source, 'let devicesDeepLinkScrolled =', '// Update cortex grid'),
  ].join('\n'), context);
  const verdict = () => vm.runInContext('selectGlanceSentence(state, Date.now())', context);
  const render = () => vm.runInContext('renderRegisteredClients(state.registeredClients)', context);
  const observe = () => vm.runInContext('updateObserve()', context);
  const load = async clients => {
    context.fetch = async route => {
      assert.strictEqual(route, '/app/network/api/clients');
      return { ok: true, json: async () => ({ clients }) };
    };
    await vm.runInContext('loadRegisteredClients()', context);
  };
  return { context, doc, elements, state, clock, verdict, render, observe, load };
}

function device(cid, sourceDelivery = null) {
  return { cid, display_label: cid, capture_state: 'active', failing: false, source_delivery: sourceDelivery };
}

function rejection(reason = 'synthetic-rejection') {
  return { state: 'needs_attention', ingest_rejection: { active_count: 1, reason_code: reason } };
}

test('source rejection stays visible through sibling success and clears only with that source', async () => {
  const env = createDeviceEnvironment();
  for (const sources of [{ audio: rejection() }, { audio: rejection(), location: { state: 'current' } }]) {
    await env.load([device('phone', sources)]);
    assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICE_FAILING');
    assert.ok(env.verdict().vars.device.includes('audio'));
    const rows = env.doc.querySelectorAll('.registered-client-row');
    assert.strictEqual(rows.length, 1);
    assert.ok(rows[0].querySelector('.registered-client-detail').textContent.includes('audio'));
    assert.ok(rows[0].querySelector('.registered-client-tech').textContent.includes('synthetic-rejection'));
    assert.strictEqual(rows[0].querySelector('a').getAttribute('href'), '/app/network/#devices');
  }
  await env.load([device('phone', { audio: rejection(), location: { state: 'current', elapsed_ms: 0 } })]);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICE_FAILING');
  await env.load([device('phone', { audio: { state: 'current' }, location: { state: 'current' } })]);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_OK');
  assert.strictEqual(env.doc.querySelector('.registered-client-detail'), null);
});

test('attention counts devices and keeps global rejection when sources are absent', async () => {
  const env = createDeviceEnvironment();
  const phone = device('phone', { audio: rejection(), location: rejection() });
  await env.load([phone]);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICE_FAILING');
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-detail').length, 2);
  const laptop = { ...device('laptop'), capture_state: 'degraded', failing: true,
    ingest_rejection: { active_count: 2, reason_code: 'global-rejection' } };
  await env.load([phone, laptop]);
  assert.strictEqual(env.verdict().vars.count, 2);
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 2);
  assert.ok(env.verdict().vars.devices.includes('laptop'));
  assert.ok(env.doc.querySelectorAll('.registered-client-tech').some(el => el.textContent.includes('global-rejection')));
});

test('quiet source unknown and setup controls add no rejected-source diagnosis', async () => {
  const env = createDeviceEnvironment();
  for (const clients of [[], [device('phone')], [device('phone', {})],
    [{ ...device('new phone'), capture_state: 'no_capture' }],
    [device('phone', { audio: { state: 'unknown', ingest_rejection: null }, location: { state: 'current' } })]]) {
    await env.load(clients);
    assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_OK');
    assert.strictEqual(env.doc.querySelector('.registered-client-detail'), null);
  }
  await env.load([device('phone', { audio: rejection(), location: { state: 'current' } })]);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICE_FAILING');
  for (const capture_state of ['offline', 'stale']) {
    await env.load([{ ...device('phone'), capture_state }]);
    assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICE_SILENT_NO_AGE');
  }
});

test('failed refresh retains evidence and observation time until a new successful read', async () => {
  const env = createDeviceEnvironment();
  const clients = [device('phone', { audio: rejection() })];
  await env.load(clients);
  const observedAt = env.state.registeredClientsObservedAt;
  const before = env.doc.querySelectorAll('.registered-client-detail').map(el => el.textContent);
  env.doc.querySelector('.registered-client-tech').open = true;
  env.doc.querySelector('.registered-client-tech summary').focus();
  env.clock.now += 60000;
  env.context.fetch = async () => ({ ok: false, status: 503 });
  await vm.runInContext('loadRegisteredClients()', env.context);
  assert.strictEqual(env.state.registeredClients, clients);
  assert.strictEqual(env.state.registeredClientsObservedAt, observedAt);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICES_UNAVAILABLE');
  assert.deepStrictEqual(env.doc.querySelectorAll('.registered-client-detail').map(el => el.textContent), before);
  assert.ok(env.doc.querySelector('[data-read-state="unavailable"]'));
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 1);
  assert.strictEqual(env.doc.querySelector('.registered-client-tech').open, true);
  assert.strictEqual(env.doc.activeElement, env.doc.querySelector('.registered-client-tech summary'));
  env.clock.now += 60000;
  await env.load([device('phone', { audio: { state: 'current' } })]);
  assert.strictEqual(env.state.registeredClientsObservedAt, env.clock.now);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_OK');
  assert.strictEqual(env.doc.querySelector('[data-read-state="unavailable"]'), null);
  assert.strictEqual(env.doc.querySelector('.registered-client-detail'), null);
  const healthyReadAt = env.state.registeredClientsObservedAt;
  env.clock.now += 60000;
  env.context.fetch = async () => { throw new Error('request failed after healthy read'); };
  await vm.runInContext('loadRegisteredClients()', env.context);
  assert.strictEqual(env.state.registeredClientsObservedAt, healthyReadAt);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICES_UNAVAILABLE');
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 0);
  assert.ok(env.doc.querySelector('[data-read-state="unavailable"]'));
  assert.strictEqual(env.doc.querySelector('.registered-client-label.connected'), null);
});

test('initial empty and malformed failed reads have a reachable unavailable destination', async () => {
  for (const failure of [async () => { throw new Error('network unavailable'); },
    async () => ({ ok: true, json: async () => ({ clients: null }) }),
    async () => ({ ok: true, json: async () => { throw new Error('invalid JSON'); } })]) {
    const env = createDeviceEnvironment();
    env.context.window.location.hash = '#registeredClientsCard';
    env.context.fetch = failure;
    await vm.runInContext('loadRegisteredClients()', env.context);
    assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICES_UNAVAILABLE');
    assert.strictEqual(env.state.registeredClientsObservedAt, null);
    assert.strictEqual(env.elements.registeredClientsCard.classList.contains('hidden'), false);
    assert.ok(env.doc.querySelector('[data-read-state="unavailable"]'));
    assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 0);
    assert.strictEqual(env.elements.registeredClientsCard.scrolledIntoView, true);
    await env.load([]);
    env.context.fetch = failure;
    await vm.runInContext('loadRegisteredClients()', env.context);
    assert.ok(env.doc.querySelector('[data-read-state="unavailable"]'));
    assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 0);
  }
});

test('unknown device knowledge cannot mute a diagnosed fault or fabricate a failure', async () => {
  const env = createDeviceEnvironment();
  await env.load([{ ...device('phone'), capture_state: 'unknown' }]);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICES_UNKNOWN');
  await env.load([{ ...device('phone'), capture_state: 'unknown' }, device('laptop', { audio: rejection() })]);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICE_FAILING');
  for (const unavailable of ['failed-read', 'unknown-device']) {
    env.state.registeredClients = [{ ...device('phone'), capture_state: 'unknown' }];
    env.state.registeredClientsFailed = unavailable === 'failed-read';
    env.state.crashed.set('observe', {});
    assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_SERVICE_ATTENTION');
    env.state.crashed.clear();
    env.context.brainSnapshot = { state: 'blocked', headline: 'brain diagnosis' };
    assert.strictEqual(env.verdict().vars.headline, 'brain diagnosis');
    env.context.brainSnapshot = null;
    env.state.searchIndex = { state: 'failing', text: 'index diagnosis' };
    assert.strictEqual(env.verdict().vars.headline, 'index diagnosis');
    env.state.clients.set('local', { lastSeen: env.clock.now - 30000 });
    assert.strictEqual(env.verdict().vars.headline, 'index diagnosis');
    env.state.clients.clear();
    env.state.searchIndex = null;
  }
  await env.load([device('phone')]);
  env.state.agents.set('task', { event: 'start' });
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_CATCHING_UP');
});

test('phone host does not expect a local desktop source while showing another device', async () => {
  const env = createDeviceEnvironment();
  await env.load([device('phone')]);
  env.state.clients.set('remote', {lastSeen: env.clock.now, mode: 'screencast', audio: {threshold_hits: 7}});
  env.state.hostCapabilities = {desktop_capture: false};
  env.observe();
  assert.strictEqual(env.elements.observeSourceNote.classList.contains('hidden'), true);
  assert.strictEqual(env.elements.observeContent.classList.contains('hidden'), false);
  assert.ok(env.elements.audioStatus.textContent.includes('7'));
  env.state.hostCapabilities = null;
  env.observe();
  assert.strictEqual(env.elements.observeSourceNote.classList.contains('hidden'), true);
});

test('observe uses fresh selected evidence rather than a stale populated map', async () => {
  const env = createDeviceEnvironment();
  await env.load([device('phone')]);
  env.state.registeredClientsFailed = true;
  for (const clients of [new Map(), new Map([['local', { lastSeen: env.clock.now - 30000, mode: 'screencast', audio: { threshold_hits: 999 } }]])]) {
    env.state.clients = clients;
    env.observe();
    assert.strictEqual(env.doc.querySelector('.observe-card').dataset.unavailable, 'true');
    assert.strictEqual(env.elements.observeContent.classList.contains('hidden'), true);
  }
  env.state.clients.set('remote', { lastSeen: env.clock.now, mode: 'screencast', audio: { threshold_hits: 7 } });
  env.observe();
  assert.strictEqual(env.doc.querySelector('.observe-card').dataset.unavailable, 'false');
  assert.strictEqual(env.elements.observeContent.classList.contains('hidden'), false);
  assert.ok(env.elements.observeSourceNote.textContent.includes('local') === false);
  assert.ok(env.elements.audioStatus.textContent.includes('7'));
  assert.ok(!env.elements.audioStatus.textContent.includes('999'));
  env.state.localHost = null;
  env.observe();
  assert.ok(!env.elements.observeSourceNote.textContent.includes('remote'));
  env.context.window.JournalFormat.installStreamLabels({ status: 'complete', labels: { ja1r: 'LABEL-ONE' } });
  env.state.clients.delete('remote');
  env.state.clients.set('ja1r', { lastSeen: env.clock.now, mode: 'screencast', audio: { threshold_hits: 7 } });
  env.observe();
  assert.ok(env.elements.observeSourceNote.textContent.includes('LABEL-ONE'));
  assert.ok(!env.elements.observeSourceNote.textContent.includes('ja1r'));
  const labelSpan = env.elements.observeSourceNote.querySelector('span');
  assert.strictEqual(labelSpan.textContent, 'LABEL-ONE');
  assert.strictEqual(labelSpan.title, 'ja1r');
  env.state.clients = new Map();
  env.state.registeredClients = [];
  env.observe();
  assert.strictEqual(env.doc.querySelector('.observe-card').dataset.unavailable, 'true');
});

test('rejection details preserve disclosure and keyboard focus without taking outside focus', async () => {
  const env = createDeviceEnvironment();
  const client = device('<phone>', { '<audio>': rejection('<reason>') });
  await env.load([client]);
  const detail = env.doc.querySelector('.registered-client-tech');
  detail.open = true;
  detail.querySelector('summary').focus();
  env.render();
  const refreshed = env.doc.querySelector('.registered-client-tech');
  assert.strictEqual(refreshed.open, true);
  assert.strictEqual(env.doc.activeElement, refreshed.querySelector('summary'));
  assert.ok(refreshed.textContent.includes('<reason>'));
  assert.strictEqual(env.doc.querySelector('script'), null);
  env.doc.body.focus();
  env.render();
  assert.strictEqual(env.doc.activeElement, env.doc.body);
});

function createHeldWorkEnvironment() {
  const env = createDeviceEnvironment();
  const { doc, context, state } = env;
  state.registeredClients = [];
  state.heldWorkCurrent = false;
  const section = doc.createElement('section');
  section.id = 'processingHolds';
  for (const attr of ['notice', 'rows', 'support']) {
    const child = doc.createElement('div');
    child.setAttribute('data-held-work-' + attr, '');
    section.appendChild(child);
  }
  doc.body.appendChild(section);
  context.scheduleName = () => null;
  context.updateVitals = () => {
    vm.runInContext('renderHeldWork()', context);
    context.updateStatusSummary();
  };
  for (const name of ['updateCortexGrid', 'updateImporterGrid', 'updateThinkCard',
    'updateObserve', 'updateConnectionHealth']) context[name] = () => {};
  state.schedules = [];
  const source = loadHealthSource();
  vm.runInContext(sourceBlock(source, 'const TALENT_NAMES =', 'const SCHEDULE_NAMES ='), context);
  vm.runInContext(sourceBlock(source, 'function handleSupervisorEvent(', 'function handleCortexEvent(')
    + sourceBlock(source, 'function invalidateRealtime(', 'function initHealthRealtime(')
    + sourceBlock(source, 'function updateConnectionHealth(', '// Sweep stale agents')
      .replace('function updateConnectionHealth(', 'function expireRealtime('), context);
  env.receive = payload => {
    context.message = { event: 'status', services: [{name: 'convey'}], queues: {}, ...payload };
    vm.runInContext('handleSupervisorEvent(message)', context);
  };
  env.section = section;
  return env;
}

test('large hold lists stay bounded and every page is reachable', async () => {
  const env = createHeldWorkEnvironment();
  const pager = env.doc.createElement('nav');
  pager.setAttribute('data-held-work-pages', '');
  for (const attr of ['data-held-previous', null, 'data-held-next']) {
    const element = env.doc.createElement(attr ? 'button' : 'span');
    if (attr) element.setAttribute(attr, '');
    pager.appendChild(element);
  }
  env.section.appendChild(pager);
  env.state.work = { inventory: { as_of_ms: env.clock.now, holds: Array.from({length: 75}, (_, i) => ({
    day: String(i), reason: 'spending_confirmation', count: 1000,
  })) } };
  env.receive({ queues: {} });
  assert.strictEqual(env.doc.querySelectorAll('.held-work-row').length, 30);
  assert.strictEqual(pager.hidden, false);
  assert.strictEqual(pager.querySelector('[data-held-previous]').disabled, true);
  pager.querySelector('[data-held-next]').onclick();
  pager.querySelector('[data-held-next]').onclick();
  assert.strictEqual(env.doc.querySelectorAll('.held-work-row').length, 15);
  assert.strictEqual(pager.querySelector('[data-held-next]').disabled, true);
  assert.ok(env.doc.querySelector('.held-work-row pre').textContent.includes('1000'));
});

test('live sources tolerate unloaded names and keep equal labels as separate devices', async () => {
  const { doc, context } = createEnvironment();
  context.elements = { clientsCard: doc.createElement('div'), clientsGrid: doc.createElement('div') };
  context.STALE_MS = 30000;
  context.state = { registeredClients: null, clients: new Map([
    ['opaque', { cid: 'one', lastSeen: Date.now() }],
    ['other', { cid: 'two', lastSeen: Date.now() }],
  ]) };
  vm.runInContext(sourceBlock(loadHealthSource(), 'function updateClients(', 'const sinceDay ='), context);
  context.window.JournalFormat.installStreamLabels({ status: 'complete', labels: { opaque: 'LABEL-TWO' } });
  vm.runInContext('updateClients()', context);
  assert.strictEqual(context.elements.clientsGrid.children.length, 2);
  const qualEls = context.elements.clientsGrid.querySelectorAll('.client-stream-qualifier');
  assert.strictEqual(qualEls[0].textContent, 'LABEL-TWO');
  assert.strictEqual(qualEls[0].title, 'opaque');
  assert.strictEqual(context.elements.clientsGrid.children.length, 2);
});

test('a stream first met on the socket re-reads the label table once and renders what it learned', async () => {
  const env = createDeviceEnvironment();
  await env.load([device('phone')]);
  const tables = [];
  let reads = 0;
  env.context.fetch = async (route) => {
    if (route === '/api/streams/labels') {
      reads += 1;
      return { ok: true, json: async () => tables.shift() };
    }
    return { ok: true, json: async () => ({ clients: [] }) };
  };
  let clientRenders = 0;
  env.context.updateClients = () => { clientRenders += 1; };
  vm.runInContext(sourceBlock(loadHealthSource(), 'function handleObserveEvent(', 'function handleImporterEvent('), env.context);
  const receive = async (stream) => {
    vm.runInContext(`handleObserveEvent(${JSON.stringify({ stream, mode: 'screencast', audio: { threshold_hits: 3 } })})`, env.context);
    await new Promise((resolve) => setImmediate(resolve));
    await new Promise((resolve) => setImmediate(resolve));
  };
  const unknown = env.context.window.JournalFormat.stream('_default');

  tables.push({ status: 'complete', labels: { ja1r: 'LABEL-ONE' } });
  await receive('ja1r');
  assert.strictEqual(reads, 1);
  const span = env.elements.observeSourceNote.querySelector('span');
  assert.strictEqual(span.textContent, 'LABEL-ONE');
  assert.strictEqual(span.title, 'ja1r');

  // A known stream re-reads nothing and renders once per message.
  const before = clientRenders;
  await receive('ja1r');
  assert.strictEqual(reads, 1);
  assert.strictEqual(clientRenders, before + 1);

  // A second new stream gets its own re-read; one the registry lacks is
  // unknown, and the note then says nothing about what it shows.
  env.state.clients.delete('ja1r');
  tables.push({ status: 'complete', labels: { ja1r: 'LABEL-ONE' } });
  await receive('device_new');
  assert.strictEqual(reads, 2);
  assert.strictEqual(env.context.window.JournalFormat.stream('device_new'), unknown);
  assert.strictEqual(env.elements.observeSourceNote.querySelector('span'), null);
  assert.ok(!env.elements.observeSourceNote.textContent.includes('device_new'));
  await receive('device_new');
  assert.strictEqual(reads, 2, 'a name gets one re-read');
});

test('holds require queued work at their scope even while unrelated work is active', async () => {
  const env = createHeldWorkEnvironment();
  const hold = { partition: 'daily', ref: 'day-task', reasons: ['root_live'] };
  env.state.agents.set('busy', { event: 'start' });
  env.receive({ held: [hold], queues: { daily: 2, activity: 1 } });
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_WORK_HELD');
  assert.strictEqual(env.verdict().action.href, '#processingHolds');
  assert.strictEqual(env.doc.querySelectorAll('.held-work-row').length, 1);
  assert.ok(env.doc.querySelector('pre').textContent.includes('day-task'));
  env.receive({ held: [hold], queues: { activity: 1 } });
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_CATCHING_UP');
  assert.strictEqual(env.section.hidden, true);
  env.receive({ held: [hold], queues: {}, queue_hold: { reason: 'records_unreadable', detail: 'read error' } });
  assert.strictEqual(env.section.hidden, true);
  env.receive({ held: [hold, { ...hold, partition: 'activity', ref: 'activity-task' }],
    queues: { daily: 2, activity: 1 }, queue_hold: { reason: 'records_unreadable', detail: '<unsafe>' } });
  assert.strictEqual(env.doc.querySelectorAll('.held-work-row').length, 3);
  assert.deepStrictEqual(Array.from(env.doc.querySelectorAll('.held-work-row'), row => row.dataset.key),
    ['queue', 'partition:daily', 'partition:activity']);
  assert.ok(env.doc.querySelector('pre').textContent.includes('<unsafe>'));
  assert.strictEqual(env.doc.querySelector('unsafe'), null);
});

test('failed or expired status retains dated hold evidence and only a snapshot clears it', async () => {
  const env = createHeldWorkEnvironment();
  env.state.loadedAt = env.clock.now;
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_OK');
  env.context.window.location.hash = '#processingHolds';
  env.receive({ held: [{ partition: 'daily', reasons: ['root_live'] }], queues: { daily: 1 } });
  assert.strictEqual(env.section.scrolledIntoView, true);
  const at = env.state.heldWorkSnapshot.observedAt;
  const details = env.doc.querySelector('details');
  details.open = true;
  details.querySelector('summary').focus();
  env.clock.now += 1000;
  env.receive({ event: 'status-error' });
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_WORK_UNAVAILABLE');
  assert.strictEqual(env.state.heldWorkSnapshot.observedAt, at);
  assert.strictEqual(env.section.dataset.current, 'false');
  assert.strictEqual(env.doc.querySelector('details'), details);
  assert.strictEqual(details.open, true);
  assert.strictEqual(env.doc.activeElement, details.querySelector('summary'));
  env.receive({ event: 'held', service: 'daily', reasons: ['root_live'] });
  assert.strictEqual(env.state.heldWorkCurrent, false);
  env.receive({ queues: { daily: 1 } }); // omitted optional fields are a supported clearance
  assert.strictEqual(env.section.hidden, true);
  assert.strictEqual(env.doc.querySelectorAll('.held-work-row').length, 0);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_OK');
  assert.strictEqual(env.state.heldWorkSnapshot.observedAt, env.clock.now);
  env.clock.now += 30000;
  vm.runInContext('expireRealtime()', env.context);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_WORK_UNAVAILABLE');
  assert.strictEqual(env.state.heldWorkCurrent, false);
});

test('held work grace window suppresses unavailable verdict before first authority status', async () => {
  const env = createHeldWorkEnvironment();
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_WORK_UNAVAILABLE');

  env.state.loadedAt = env.clock.now;
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_OK');

  env.state.work = { error: 'unreadable' };
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_WORK_UNAVAILABLE');

  env.state.work = { inventory: { as_of_ms: env.clock.now - 180000 } };
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_WORK_UNAVAILABLE');

  env.state.work = null;
  env.receive({ held: [{ partition: 'daily', reasons: ['root_live'] }], queues: { daily: 1 } });
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_WORK_HELD');

  const envExpired = createHeldWorkEnvironment();
  envExpired.state.loadedAt = envExpired.clock.now;
  envExpired.state.work = null;
  envExpired.clock.now += 15000;
  assert.strictEqual(envExpired.verdict().key, 'HEALTH_GLANCE_WORK_UNAVAILABLE');
});

test('set-aside units share one list, retain focus, and render retry errors as text', async () => {
  const env = createHeldWorkEnvironment();
  const id = 'a'.repeat(64);
  env.state.work = { inventory: { as_of_ms: env.clock.now, holds: [{ retry_unit: id, day: '20260101', reason: 'schema_invalid' }] },
    retry_records: [{ id, unit: { talent: 'documents' }, reason: 'schema_invalid' }] };
  env.state.legacyHolds = [{ day: '20260101', reason: 'legacy duplicate' }];
  env.receive({ held: [{ partition: 'daily', reasons: ['root_live'] }], queues: { daily: 1 } });
  assert.strictEqual(env.doc.querySelectorAll('.held-work-row').length, 2);
  const row = env.doc.querySelectorAll('.held-work-row')[1];
  const button = row.querySelector('button');
  button.focus();
  row.querySelector('details').open = true;
  let posted;
  env.context.fetch = async (url, options) => {
    posted = [url, JSON.parse(options.body)];
    return { ok: false, json: async () => ({ message: '<script>held</script>' }) };
  };
  env.context.loadHealthState = async () => vm.runInContext('renderHeldWork()', env.context);
  await button.onclick();
  assert.strictEqual(posted[0], '/app/health/api/retry-work');
  assert.strictEqual(posted[1].unit, id);
  assert.strictEqual(row.querySelector('[data-retry-feedback]').textContent, '<script>held</script>');
  assert.strictEqual(env.doc.querySelector('script'), null);
  assert.strictEqual(env.doc.activeElement, button);
  assert.strictEqual(row.querySelector('details').open, true);
  assert.strictEqual(button.disabled, false);
});

test('backlog guidance keeps the server wording when the locale uses no spaces', async () => {
  const env = createHeldWorkEnvironment();
  env.state.legacyHolds = [{ day: '20260101', reason: '再試行してください' }];
  env.receive({ held: [], queues: {} });
  const row = env.doc.querySelector('.held-work-row');
  assert.ok(row.querySelector('p').textContent.endsWith('再試行してください'));
});

test('known service device brain and index faults retain priority over held work', async () => {
  const env = createHeldWorkEnvironment();
  env.receive({ held: [{ partition: 'daily', reasons: ['root_live'] }], queues: { daily: 1 } });
  env.state.crashed.set('convey', {});
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_SERVICE_ATTENTION');
  env.state.crashed.clear();
  env.context.brainSnapshot = { state: 'blocked', headline: 'known brain fault' };
  assert.strictEqual(env.verdict().vars.headline, 'known brain fault');
  env.context.brainSnapshot = null;
  env.state.registeredClients = [device('phone', { audio: rejection() })];
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_DEVICE_FAILING');
  env.state.registeredClients = [];
  env.state.searchIndex = { state: 'failing', text: 'known index fault' };
  assert.strictEqual(env.verdict().vars.headline, 'known index fault');
});

test('work progress keeps unknown and stale reach honest and only confirms after a saved choice', async () => {
  const { doc, context } = createEnvironment();
  const section = doc.createElement('section');
  section.id = 'workProgress';
  for (const key of ['reach', 'history', 'bands', 'confirm', 'cost', 'details', 'allow', 'action-result']) {
    const child = doc.createElement('div');
    child.setAttribute('data-work-' + key, '');
    section.appendChild(child);
    if (key === 'details') child.appendChild(doc.createElement('pre'));
  }
  doc.body.appendChild(section);
  const now = 1800000000000;
  context.Date = class extends Date { static now() { return now; } };
  let refreshed = 0;
  context.loadHealthState = async () => { refreshed += 1; };
  vm.runInContext(sourceBlock(loadHealthSource(), 'function renderWorkProgress(', 'let healthStateRequest = null;'), context);
  context.work = { inventory: null, history_confirmed: false, confirmation_available: true };
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(section.querySelector('[data-work-bands]').children.length, 0);
  assert.strictEqual(section.querySelector('[data-work-confirm]').hidden, false);
  context.work.confirmation_available = false;
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(section.querySelector('[data-work-confirm]').hidden, true);
  context.work.confirmation_available = true;
  vm.runInContext('renderWorkProgress(work)', context);
  const button = section.querySelector('[data-work-allow]');
  let submitted;
  context.fetch = async (route, options) => {
    assert.strictEqual(route, '/app/settings/api/config');
    submitted = JSON.parse(options.body);
    return { ok: false };
  };
  await button.onclick();
  assert.strictEqual(submitted.data.history_confirmed, true);
  assert.strictEqual(refreshed, 0);
  assert.strictEqual(context.work.history_confirmed, false);
  assert.strictEqual(section.querySelector('[data-work-confirm]').hidden, false);
  context.fetch = async () => ({ ok: true });
  await button.onclick();
  assert.strictEqual(refreshed, 1);
  context.work.history_confirmed = true;
  context.work.inventory = { as_of_ms: now - 200000, complete: true, oldest_day: '19990101', newest_day: '20261011', days_inspected: 1, days_total: 2, unreadable: [], bands: Array.from({ length: 6 }, () => ({ segments_complete: 1, segments_owed: 2, daily_units_complete: 3, daily_units_owed: 4 })) };
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(section.querySelector('[data-work-bands]').children.length, 6);
  assert.strictEqual(section.querySelector('[data-work-confirm]').hidden, true);
  const stale = section.querySelector('[data-work-reach]').textContent;
  // A finished pass resting until its next one is due is still current.
  context.work.inventory.next_pass_ms = now + 600000;
  vm.runInContext('renderWorkProgress(work)', context);
  const resting = section.querySelector('[data-work-reach]').textContent;
  assert.notStrictEqual(resting, stale);
  delete context.work.inventory.next_pass_ms;
  context.work.inventory.as_of_ms = now;
  context.work.inventory.complete = false;
  vm.runInContext('renderWorkProgress(work)', context);
  assert.notStrictEqual(section.querySelector('[data-work-reach]').textContent, stale);
  context.work.inventory.complete = true;
  vm.runInContext('renderWorkProgress(work)', context);
  const cost = section.querySelector('[data-work-cost]');
  assert.deepStrictEqual(cost.textContent.match(/\d+/g), ['4', '8', '0']);
  context.work.inventory.bands[5].segments_owed = 11;
  vm.runInContext('renderWorkProgress(work)', context);
  assert.deepStrictEqual(cost.textContent.match(/\d+/g), ['13', '8', '0']);
  context.work.inventory.complete = false;
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(cost.textContent.match(/\d+/g), null);
  const history = section.querySelector('[data-work-history]');
  assert.strictEqual(history.hidden, true);
  context.work.inventory.history = { caught_up_since: '20260918', through_day: '20261011',
    segments_owed: 223701, daily_units_owed: 10161, net_per_hour: null, set_aside_units: 2, unreadable_days: 1 };
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(history.hidden, false);
  assert(history.textContent.includes((223701).toLocaleString()));
  assert(history.textContent.includes((10161).toLocaleString()));
  const noRate = history.textContent;
  context.work.inventory.history.net_per_hour = 311;
  vm.runInContext('renderWorkProgress(work)', context);
  assert.notStrictEqual(history.textContent, noRate);
  context.work.inventory.history.net_per_hour = 0.25;
  vm.runInContext('renderWorkProgress(work)', context);
  assert(history.textContent.includes((0.25).toLocaleString()));
  context.work.inventory.as_of_ms = now - 200000;
  vm.runInContext('renderWorkProgress(work)', context);
  assert(!history.textContent.includes((0.25).toLocaleString()));
  // A rest whose next pass was moved back reads current until the new time;
  // once a stopped worker misses its last promised time, it reads as checked.
  context.work.inventory.next_pass_ms = now + 600000;
  vm.runInContext('renderWorkProgress(work)', context);
  assert(history.textContent.includes((0.25).toLocaleString()));
  assert(!history.textContent.startsWith('last checked: '));
  context.work.inventory.next_pass_ms = now - 180000;
  vm.runInContext('renderWorkProgress(work)', context);
  assert(history.textContent.startsWith('last checked: '));
  assert(!history.textContent.includes((0.25).toLocaleString()));
  delete context.work.inventory.next_pass_ms;
  context.work.inventory.history = null;
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(history.hidden, true);
  assert.strictEqual(history.textContent, '');

  context.work.error = '<unreadable>';
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(section.querySelector('[data-work-details]').hidden, false);
  assert.strictEqual(section.querySelector('pre').textContent, '<unreadable>');
});

test('the import band gets its own calm row with a pause that only covers older work', async () => {
  const { doc, context } = createEnvironment();
  const section = doc.createElement('section');
  section.id = 'workProgress';
  for (const key of ['reach', 'history', 'bands', 'confirm', 'cost', 'details', 'allow', 'action-result']) {
    const child = doc.createElement('div');
    child.setAttribute('data-work-' + key, '');
    section.appendChild(child);
    if (key === 'details') child.appendChild(doc.createElement('pre'));
  }
  const block = doc.createElement('div');
  block.setAttribute('data-work-backfill', '');
  const bar = doc.createElement('div');
  bar.setAttribute('data-work-backfill-bar', '');
  const fill = doc.createElement('div');
  fill.setAttribute('data-work-backfill-fill', '');
  bar.appendChild(fill);
  block.appendChild(bar);
  for (const key of ['line', 'toggle', 'result']) {
    const child = doc.createElement(key === 'toggle' ? 'button' : 'p');
    child.setAttribute('data-work-backfill-' + key, '');
    block.appendChild(child);
  }
  section.appendChild(block);
  doc.body.appendChild(section);
  const now = 1800000000000;
  context.Date = class extends Date { static now() { return now; } };
  let refreshed = 0;
  context.loadHealthState = async () => { refreshed += 1; };
  vm.runInContext(sourceBlock(loadHealthSource(), 'function renderWorkProgress(', 'let healthStateRequest = null;'), context);
  const bands = Array.from({ length: 6 }, () => ({ segments_complete: 1, segments_owed: 2, daily_units_complete: 3, daily_units_owed: 4 }));
  const inventory = { as_of_ms: now, complete: true, oldest_day: '20210101', newest_day: '20261124', days_inspected: 2, days_total: 2, unreadable: [], bands };
  const backfill = { paused: false, landed_segments: 1420, interpreted_segments: 612, owed_segments: 808, activities_owed: 4, daily_units_owed: 0, permission_held: 0 };
  const line = block.querySelector('[data-work-backfill-line]');
  const toggle = block.querySelector('[data-work-backfill-toggle]');

  // Older work in flight: its own row, and the band list doesn't repeat it.
  context.work = { inventory, backfill };
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(block.hidden, false);
  assert.strictEqual(section.querySelector('[data-work-bands]').children.length, 5);
  assert.strictEqual(bar.getAttribute('aria-valuenow'), '612');
  assert.strictEqual(bar.getAttribute('aria-valuemax'), '1420');
  assert.ok(Math.abs(parseFloat(fill.style.width) - (612 / 1420) * 100) < 0.001);
  assert.ok(line.textContent.includes((612).toLocaleString()) && line.textContent.includes((1420).toLocaleString()));
  assert.strictEqual(block.classList.contains('is-paused'), false);
  const running = line.textContent;
  const pauseLabel = toggle.textContent;

  // Pausing posts the flip and refreshes; a refused save says so and keeps the button.
  let posted;
  context.fetch = async (route, options) => {
    assert.strictEqual(route, '/app/health/api/work/pause-backfill');
    assert.strictEqual(options.method, 'POST');
    posted = JSON.parse(options.body);
    return { ok: false };
  };
  await toggle.onclick();
  assert.deepStrictEqual(posted, { paused: true });
  assert.strictEqual(refreshed, 0);
  assert.strictEqual(block.querySelector('[data-work-backfill-result]').hidden, false);
  assert.strictEqual(toggle.disabled, false);
  context.fetch = async (route, options) => { posted = JSON.parse(options.body); return { ok: true }; };
  await toggle.onclick();
  assert.strictEqual(refreshed, 1);
  assert.strictEqual(block.querySelector('[data-work-backfill-result]').hidden, true);

  // Paused: different words and button, same counts; resume posts the reverse.
  // Older bands held by the pause say paused, not that permission is owed.
  const permissionLine = section.querySelector('[data-work-bands]').children[4].textContent;
  inventory.bands.forEach((band) => { band.permission_held = 2; });
  context.work.backfill = { ...backfill, paused: true, permission_held: 3 };
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(block.classList.contains('is-paused'), true);
  assert.notStrictEqual(line.textContent, running);
  assert.notStrictEqual(toggle.textContent, pauseLabel);
  const pausedBand = section.querySelector('[data-work-bands]').children[4].textContent;
  assert.notStrictEqual(pausedBand, permissionLine);
  assert.ok(!pausedBand.includes('2 waiting'));
  assert.ok(section.querySelector('[data-work-bands]').children[0].textContent.includes('2 waiting'));
  await toggle.onclick();
  assert.deepStrictEqual(posted, { paused: false });

  // Paused before anything arrived: no bar, still a way back.
  context.work.backfill = { paused: true, landed_segments: 0, interpreted_segments: 0, owed_segments: 0 };
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(block.hidden, false);
  assert.strictEqual(bar.hidden, true);

  // Finished, only day outputs left, or never started: the list keeps all six bands.
  context.work.backfill = { ...backfill, landed_segments: 0, interpreted_segments: 0, owed_segments: 0, email_exchanges_done: 2, email_exchanges_total: 7 };
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(block.hidden, false);
  assert.strictEqual(bar.getAttribute('aria-valuenow'), '2');
  assert.strictEqual(bar.getAttribute('aria-valuemax'), '7');
  context.work.backfill.email_exchanges_done = 7;
  vm.runInContext('renderWorkProgress(work)', context);
  assert.strictEqual(block.hidden, true);

  for (const finished of [{ paused: false, landed_segments: 1420, interpreted_segments: 1420, owed_segments: 0 },
    { paused: false, landed_segments: 0, interpreted_segments: 0, owed_segments: 0, daily_units_owed: 5, activities_owed: 2 }, undefined]) {
    context.work.backfill = finished;
    vm.runInContext('renderWorkProgress(work)', context);
    assert.strictEqual(block.hidden, true);
    assert.strictEqual(section.querySelector('[data-work-bands]').children.length, 6);
  }
});

function createBackupEnvironment() {
  const env = createDeviceEnvironment();
  const { doc, context, state } = env;
  state.registeredClients = [];
  const section = doc.createElement('section');
  section.id = 'backup';
  const summary = doc.createElement('p');
  summary.setAttribute('data-backup-summary', '');
  const facts = doc.createElement('div');
  facts.setAttribute('data-backup-facts', '');
  section.append(summary, facts);
  doc.body.appendChild(section);
  Object.assign(context, { AbortController, setTimeout, clearTimeout, logHealthError() {} });
  loadHealthCatalog(context);
  vm.runInContext(fs.readFileSync(path.resolve(crateDir, '..', 'solstone-core-backup-web', 'assets', 'backup-status.js'), 'utf8'), context);
  vm.runInContext(sourceBlock(loadHealthSource(), 'const backupSection =', 'function clearHealthStateError('), context);
  const read = async payload => {
    context.getJson = async path => {
      assert.strictEqual(path, '/app/backup/status');
      if (payload instanceof Error) throw payload;
      return payload;
    };
    await vm.runInContext('loadBackupStatus()', context);
  };
  return { ...env, section, summary, facts, read };
}

const successfulBackup = { status: 'ok', time: 1770000000, snapshot_id: 'copy-a' };
const backupSnapshot = attempt => ({ success: true, enabled: true, operation: null,
  last_backup: attempt, last_verification: { status: 'ok', time: 1770000100, checked_subset: '1/52' } });

test('backup producer facts survive failed refresh and recover without a misleading Health verdict', async () => {
  const env = createBackupEnvironment();
  const failed = { status: 'error', time: 1770000200, error_reason: 'timeout', last_success: successfulBackup };
  await env.read(backupSnapshot(failed));
  assert.strictEqual(env.state.backupView.tone, 'attention');
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_BACKUP');
  const rows = env.facts.querySelectorAll('dd');
  assert.ok(rows[2].textContent.includes(new Date(failed.time * 1000).toLocaleString()));
  assert.ok(rows[3].textContent.includes(new Date(successfulBackup.time * 1000).toLocaleString()));
  assert.ok(rows[4].textContent.includes('1 of 52 parts'));
  assert.ok(!rows[4].textContent.includes('1/52'), 'the raw part selector never reaches the owner');
  assert.strictEqual(env.facts.querySelector('details').hasAttribute('open'), false);
  env.facts.querySelector('details').open = true;
  env.facts.querySelector('summary').focus();
  await env.read(backupSnapshot(failed));
  assert.strictEqual(env.facts.querySelector('details').open, true);
  assert.strictEqual(env.doc.activeElement, env.facts.querySelector('summary'));
  await env.read(new Error('failed read'));
  assert.strictEqual(env.state.backupView.tone, 'unavailable');
  assert.strictEqual(env.facts.children.length, 0);
  assert.strictEqual(env.doc.activeElement, env.summary);
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_BACKUP');
  await env.read(backupSnapshot(successfulBackup));
  assert.strictEqual(env.state.backupView.tone, 'neutral');
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_OK');
  await env.read(backupSnapshot(failed));
  env.facts.querySelector('summary').focus();
  await env.read(backupSnapshot(successfulBackup));
  assert.strictEqual(env.facts.querySelector('details'), null);
  assert.strictEqual(env.doc.activeElement, env.summary,
    'removing resolved technical details keeps keyboard focus at the backup status');
});

test('Health backup verification describes one checked part and rejects invalid selectors', async () => {
  const env = createBackupEnvironment();
  const readSubset = async checked_subset => {
    const snapshot = backupSnapshot(successfulBackup);
    snapshot.last_verification.checked_subset = checked_subset;
    await env.read(snapshot);
    return env.facts.querySelectorAll('dd')[4].textContent;
  };
  const first = await readSubset('1/52');
  assert.strictEqual(await readSubset('42/52'), first);
  assert.ok(first.includes('1 of 52'));
  assert.ok(!first.includes('1/52'));
  assert.ok((await readSubset('1/1')).includes('all of your backup'));
  const unknown = await readSubset(undefined);
  for (const invalid of ['/1', '0/1', '2/1', '1.5/52', '1/0', '-1/52', '1/9007199254740992']) {
    assert.strictEqual(await readSubset(invalid), unknown);
  }
});

test('backup off, unknown, incomplete, active and changed-destination history stay distinct', async () => {
  const env = createBackupEnvironment();
  await env.read({ ...backupSnapshot(null), enabled: false });
  assert.strictEqual(env.state.backupView.tone, 'neutral');
  await env.read(backupSnapshot(null));
  assert.strictEqual(env.state.backupView.tone, 'unavailable');
  const incomplete = { ...successfulBackup, error_reason: 'incomplete', detail: '<img src=x onerror=evil()>' };
  await env.read(backupSnapshot(incomplete));
  assert.strictEqual(env.state.backupView.tone, 'attention');
  assert.strictEqual(env.facts.querySelectorAll('img').length, 0);
  assert.ok(env.facts.querySelector('details').textContent.includes(incomplete.detail));
  const failure = { status: 'error', time: 1770000200, last_success: incomplete };
  const active = { ...backupSnapshot(failure), operation: { kind: 'restore', phase: 'restoring' }, destination: { repository: 'changed' } };
  await env.read(active);
  assert.strictEqual(env.state.backupView.tone, 'attention');
  assert.strictEqual(env.state.backupView.snapshot.operation.phase, 'restoring');
  assert.strictEqual(env.state.backupView.snapshot.last_backup.last_success.error_reason, 'incomplete');
  await env.read(backupSnapshot({ ...successfulBackup, last_success: null }));
  assert.ok(!env.facts.querySelectorAll('dd')[3].textContent.includes(new Date(successfulBackup.time * 1000).toLocaleString()));
  await env.read({ ...backupSnapshot(successfulBackup), enabled: 'false' });
  assert.strictEqual(env.state.backupView.tone, 'unavailable');
  await env.read({ ...backupSnapshot(successfulBackup), operation: { kind: 'restore', phase: 'future-phase' } });
  assert.strictEqual(env.state.backupView.tone, 'unavailable');
  await env.read({ ...backupSnapshot(successfulBackup), operation: { kind: 'restore', phase: 'error', reason_code: 'integrity_failed' } });
  assert.strictEqual(env.state.backupView.tone, 'attention');
  await env.read({ ...backupSnapshot(successfulBackup), operation: { kind: 'restore', phase: 'error', reason_code: 'cancelled' } });
  assert.strictEqual(env.state.backupView.tone, 'neutral');
  await env.read(backupSnapshot({ ...successfulBackup, last_success: { ...successfulBackup, time: 'yesterday' } }));
  assert.ok(!env.facts.querySelectorAll('dd')[3].textContent.includes(new Date(successfulBackup.time * 1000).toLocaleString()));
  await env.read(backupSnapshot({ status: 'toString', time: 1770000000 }));
  assert.strictEqual(env.state.backupView.tone, 'unavailable');
});

test('backup history qualifies the saved copy independently of the latest attempt', async () => {
  const env = createBackupEnvironment();
  const read = async (attemptMatches, copyMatches, schedule) => {
    await env.read({
      ...backupSnapshot({ ...successfulBackup, last_success: successfulBackup }),
      last_backup_current_setup: attemptMatches,
      last_success_current_setup: copyMatches,
      schedule,
    });
    return {
      copy: env.facts.querySelectorAll('dd')[3].textContent,
      schedule: env.facts.querySelector('p').textContent,
    };
  };
  const earlier = await read(true, false, { enabled: false });
  const current = await read(true, true, { enabled: true });
  const unknown = await read(true, null, null);
  assert.notStrictEqual(earlier.copy, current.copy);
  assert.notStrictEqual(unknown.copy, current.copy);
  assert.notStrictEqual(earlier.copy, unknown.copy);
  assert.strictEqual(new Set([earlier.schedule, current.schedule, unknown.schedule]).size, 3);
  assert.strictEqual(env.state.backupView.tone, 'neutral');

  await env.read({
    ...backupSnapshot({ status: 'error', time: successfulBackup.time, last_success: successfulBackup }),
    last_backup_current_setup: false,
    last_success_current_setup: false,
  });
  assert.strictEqual(env.state.backupView.tone, 'neutral');
  assert.strictEqual(env.facts.querySelectorAll('dd')[3].textContent, earlier.copy);
});

test('backup read cannot repaint a replacement workspace and the deep link reaches detail', async () => {
  const env = createBackupEnvironment();
  let finish;
  env.context.getJson = () => new Promise(resolve => { finish = resolve; });
  const pending = vm.runInContext('loadBackupStatus()', env.context);
  env.doc.body.removeChild(env.section);
  const before = env.state.backupView;
  finish(backupSnapshot(successfulBackup));
  await pending;
  assert.strictEqual(env.state.backupView, before);
  const replacement = env.doc.createElement('section');
  replacement.id = 'backup';
  env.doc.body.appendChild(replacement);
  let reads = 0;
  env.context.getJson = async () => { reads++; return backupSnapshot(successfulBackup); };
  await vm.runInContext('loadBackupStatus()', env.context);
  assert.strictEqual(reads, 0, 'a detached consumer cannot start reads against a replacement');
  env.doc.body.removeChild(replacement);
  env.doc.body.appendChild(env.section);
  env.context.window.location.hash = '#backup';
  vm.runInContext('focusBackupDetails()', env.context);
  assert.strictEqual(env.section.scrolledIntoView, true);
});

test('a proved backup problem precedes unassessed devices, and service failure still wins', async () => {
  const env = createBackupEnvironment();
  await env.read(backupSnapshot({ status: 'error', time: 1770000200 }));
  env.state.registeredClients = null;
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_BACKUP');
  env.state.crashed.set('observe', {});
  assert.strictEqual(env.verdict().key, 'HEALTH_GLANCE_SERVICE_ATTENTION');
});

if (process.env.BACKUP_STATUS_FIXTURES) {
  test('actual persisted backup status feeds the shared Health renderer', async () => {
    for (const fixture of JSON.parse(process.env.BACKUP_STATUS_FIXTURES)) {
      const env = createBackupEnvironment();
      await env.read(fixture.payload);
      assert.strictEqual(env.state.backupView.tone, fixture.tone);
      if (fixture.success_time) {
        assert.ok(env.facts.querySelectorAll('dd')[3].textContent.includes(new Date(fixture.success_time * 1000).toLocaleString()));
      }
    }
  });
}

test('registered clients card state choices', async () => {
  const env = createDeviceEnvironment();

  // A count sentence is present only when at least one client has capture_state active.
  await env.load([]);
  assert.strictEqual(env.doc.querySelector('.registered-clients-count'), null);
  await env.load([{ ...device('phone'), capture_state: 'degraded' }]);
  assert.strictEqual(env.doc.querySelector('.registered-clients-count'), null);
  await env.load([{ ...device('phone'), capture_state: 'active' }]);
  assert.ok(env.doc.querySelector('.registered-clients-count'));

  // An empty list shows no devices are paired yet. and a pair link to /app/network/#devices, and the card stays visible.
  await env.load([]);
  assert.strictEqual(env.elements.registeredClientsCard.classList.contains('hidden'), false);
  const emptyP = env.doc.querySelector('.registered-clients-empty');
  assert.ok(emptyP && emptyP.textContent.includes('no devices are paired yet.'));
  const pairLink = env.doc.querySelector('.registered-clients-manage');
  assert.ok(pairLink && pairLink.getAttribute('href') === '/app/network/#devices');

  // A failed read shows delivery status couldn't be checked., no count sentence, no empty-state sentence,
  // and the rows below show that last check. only when an attention row exists.
  // 1) When attention row exists from previous check:
  await env.load([{ ...device('phone'), capture_state: 'degraded' }]);
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 1);
  env.context.fetch = async () => ({ ok: false, status: 500 });
  await vm.runInContext('loadRegisteredClients()', env.context);
  assert.strictEqual(env.doc.querySelector('.registered-clients-count'), null);
  assert.strictEqual(env.doc.querySelector('.registered-clients-empty'), null);
  const failNotice1 = env.doc.querySelector('[data-read-state="unavailable"]');
  assert.ok(failNotice1 && failNotice1.textContent.includes("delivery status couldn't be checked."));
  assert.ok(failNotice1.textContent.includes('the rows below show that last check.'));
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 1);

  // 2) When no attention row existed (healthy read fails):
  await env.load([device('phone')]); // healthy active -> 0 attention rows
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 0);
  env.context.fetch = async () => ({ ok: false, status: 500 });
  await vm.runInContext('loadRegisteredClients()', env.context);
  assert.strictEqual(env.doc.querySelector('.registered-clients-count'), null);
  assert.strictEqual(env.doc.querySelector('.registered-clients-empty'), null);
  const failNotice2 = env.doc.querySelector('[data-read-state="unavailable"]');
  assert.ok(failNotice2 && failNotice2.textContent.includes("delivery status couldn't be checked."));
  assert.ok(!failNotice2.textContent.includes('the rows below show that last check.'));
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 0);

  // A degraded client with no rejected sources is a row, and the glance action href is still #registeredClientsCard.
  await env.load([{ ...device('laptop'), capture_state: 'degraded', failing: true, source_delivery: null }]);
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 1);
  assert.strictEqual(env.verdict().action.href, '#registeredClientsCard');

  // no_capture plus rejected uploads is a row and is not in the count.
  await env.load([{ ...device('phone', { audio: rejection() }), capture_state: 'no_capture' }]);
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 1);
  assert.strictEqual(env.doc.querySelector('.registered-clients-count'), null);

  // active plus clock_skew is in the count and is a row.
  await env.load([{ ...device('phone'), capture_state: 'active', clock_skew: { skew_ms: 5000 } }]);
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 1);
  assert.ok(env.doc.querySelector('.registered-clients-count'));

  // active with no problem is in the count and is not a row.
  await env.load([device('phone')]);
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 0);
  assert.ok(env.doc.querySelector('.registered-clients-count'));
});

function createSpeechEnvironment(applicable = true) {
  const source = loadHealthSource();
  const doc = new Document();
  doc.hidden = false;

  const brain = doc.createElement('section');
  brain.id = 'brain';
  doc.body.appendChild(brain);

  const brainHealthStatus = doc.createElement('div');
  brainHealthStatus.id = 'brainHealthStatus';
  brain.appendChild(brainHealthStatus);

  const speechSetup = doc.createElement('div');
  speechSetup.id = 'speechSetup';
  speechSetup.setAttribute('data-applicable', String(applicable));
  speechSetup.hidden = true;

  const stateP = doc.createElement('p');
  const speechLabel = doc.createElement('span');
  speechLabel.textContent = 'local speech recognition';
  const stateEl = doc.createElement('span');
  stateEl.id = 'speechSetupState';
  stateP.append(speechLabel, doc.createTextNode(': '), stateEl);
  speechSetup.appendChild(stateP);

  const expEl = doc.createElement('p');
  expEl.id = 'speechSetupExplanation';
  expEl.hidden = true;
  speechSetup.appendChild(expEl);

  const bytesEl = doc.createElement('div');
  bytesEl.id = 'speechSetupBytes';
  bytesEl.hidden = true;
  speechSetup.appendChild(bytesEl);

  const detailsEl = doc.createElement('details');
  detailsEl.id = 'speechSetupDetails';

  const summaryEl = doc.createElement('summary');
  summaryEl.id = 'speechSetupSummary';
  detailsEl.appendChild(summaryEl);

  const causeEl = doc.createElement('p');
  causeEl.id = 'speechSetupCause';
  causeEl.hidden = true;
  detailsEl.appendChild(causeEl);

  const instructionEl = doc.createElement('p');
  instructionEl.id = 'speechSetupInstruction';
  instructionEl.hidden = true;
  detailsEl.appendChild(instructionEl);

  const commandWrapper = doc.createElement('pre');
  commandWrapper.id = 'speechSetupCommandWrapper';
  commandWrapper.hidden = true;
  const commandEl = doc.createElement('code');
  commandEl.id = 'speechSetupCommand';
  commandWrapper.appendChild(commandEl);
  detailsEl.appendChild(commandWrapper);

  speechSetup.appendChild(detailsEl);
  brain.appendChild(speechSetup);

  const timers = [];
  const customSetTimeout = (fn, delay) => {
    const timer = { fn, delay, active: true };
    timers.push(timer);
    return timer;
  };
  const customClearTimeout = (timer) => {
    if (timer) timer.active = false;
  };

  const loggedErrors = [];

  const context = vm.createContext({
    reportHealthReady() {},
    document: doc,
    window: { createViewReady: () => () => {},
      location: { hash: '' },
      addEventListener: () => {},
      removeEventListener: () => {},
    },
    console: { warn() {}, error() {}, log() {} },
    AbortController,
    CustomEvent: class { constructor(type, detail) { this.type = type; this.detail = detail; } },
    setTimeout: customSetTimeout,
    clearTimeout: customClearTimeout,
    logHealthError: (err, msg) => { loggedErrors.push({ err, msg }); },
    loadRegisteredClients: () => {},
    loadBackupStatus: () => {},
    getJson: async () => ({
      applicable: true,
      visible: true,
      state: 'downloading',
      tone: 'neutral',
    }),
  });

  loadHealthCatalog(context);
  context.state = {hostCapabilities: {terminal_speech_setup: true}};
  const speechCode = sourceBlock(source, 'const SPEECH_SETUP_COPY =', 'function clearHealthStateError(');
  vm.runInContext(speechCode, context);

  return {
    doc,
    context,
    brain,
    speechSetup,
    bytesEl,
    detailsEl,
    summaryEl,
    causeEl,
    timers,
    loggedErrors,
    source,
  };
}

test('refreshStatusReads records fetch of /app/health/api/speech-setup and does not fetch /app/health/api/state', async () => {
  const env = createSpeechEnvironment();
  const fetchedUrls = [];
  env.context.getJson = async (url) => {
    fetchedUrls.push(url);
    return { applicable: false, visible: false };
  };
  env.context.fetch = async (url) => {
    fetchedUrls.push(url);
    return { ok: true, json: async () => ({}) };
  };
  await vm.runInContext('refreshStatusReads()', env.context);
  assert.ok(fetchedUrls.includes('/app/health/api/speech-setup'));
  assert.ok(!fetchedUrls.includes('/app/health/api/state'));
});

test('phone host hides terminal speech repair without reading a desktop setup path', async () => {
  const env = createSpeechEnvironment();
  env.context.state.hostCapabilities = {terminal_speech_setup: false};
  env.speechSetup.hidden = false;
  env.context.getJson = async () => { throw new Error('desktop setup must not be requested'); };
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, true);
  assert.strictEqual(env.loggedErrors.length, 0);
});

test('speech setup stage change updates in place keeping same node', async () => {
  const env = createSpeechEnvironment();
  const originalNode = env.speechSetup;

  let currentPayload = {
    applicable: true,
    visible: true,
    state: 'downloading',
    tone: 'neutral',
    bytes_received: 1000,
    bytes_total: 5000,
    cause: null,
    recovery_representable: true,
    command: null,
  };
  env.context.getJson = async () => currentPayload;

  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.doc.getElementById('speechSetup'), originalNode);
  assert.strictEqual(originalNode.hidden, false);
  assert.strictEqual(originalNode.getAttribute('data-state'), 'downloading');
  assert.ok(originalNode.classList.contains('speech-setup-tone-neutral'));

  currentPayload = {
    applicable: true,
    visible: true,
    state: 'verifying',
    tone: 'neutral',
    bytes_received: 5000,
    bytes_total: 5000,
    cause: null,
    recovery_representable: true,
    command: null,
  };
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.doc.getElementById('speechSetup'), originalNode);
  assert.strictEqual(originalNode.hidden, false);
  assert.strictEqual(originalNode.getAttribute('data-state'), 'verifying');
  assert.ok(originalNode.classList.contains('speech-setup-tone-neutral'));
});

test('speech setup visible: false with applicable: true sets hidden on the row', async () => {
  const env = createSpeechEnvironment();
  env.context.getJson = async () => ({
    applicable: true,
    visible: false,
    state: null,
    tone: null,
    bytes_received: null,
    bytes_total: null,
    cause: null,
    recovery_representable: false,
    command: null,
  });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, true);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), null);
});

test('after applicable visible success, rejected getJson shows unavailable and releases guard', async () => {
  const env = createSpeechEnvironment();
  env.context.getJson = async () => ({
    applicable: true,
    visible: true,
    state: 'downloading',
    tone: 'neutral',
    bytes_received: 1000,
    bytes_total: 5000,
    cause: null,
    recovery_representable: true,
    command: null,
  });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, false);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'downloading');
  assert.strictEqual(env.bytesEl.getAttribute('data-received'), '1000');
  assert.strictEqual(env.bytesEl.getAttribute('data-total'), '5000');

  let fetchCount = 0;
  env.context.getJson = async () => {
    fetchCount++;
    throw new Error('network down');
  };
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(fetchCount, 1);
  assert.strictEqual(env.speechSetup.hidden, false);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'unavailable');
  assert.ok(env.speechSetup.classList.contains('speech-setup-tone-unavailable'));
  assert.strictEqual(env.bytesEl.hasAttribute('data-received'), false);
  assert.strictEqual(env.bytesEl.hasAttribute('data-total'), false);
  assert.strictEqual(env.bytesEl.hidden, true);

  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(fetchCount, 2);
});

test('never settling getJson times out at 10000ms callback, releases guard, and ignores late resolve', async () => {
  const env = createSpeechEnvironment();
  env.context.getJson = async () => ({
    applicable: true,
    visible: true,
    state: 'downloading',
    tone: 'neutral',
    bytes_received: 1000,
    bytes_total: 5000,
    cause: null,
    recovery_representable: true,
    command: null,
  });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'downloading');

  let resolvePromise;
  env.context.getJson = () => new Promise(resolve => { resolvePromise = resolve; });

  const loadPromise = vm.runInContext('loadSpeechSetup()', env.context);

  const timer = env.timers.find(t => t.delay === 10000 && t.active);
  assert.ok(timer, 'timeout timer with 10000ms delay registered');

  timer.fn();

  assert.strictEqual(env.speechSetup.hidden, false);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'unavailable');
  assert.ok(env.speechSetup.classList.contains('speech-setup-tone-unavailable'));
  assert.strictEqual(env.bytesEl.hidden, true);
  assert.strictEqual(env.bytesEl.hasAttribute('data-received'), false);

  resolvePromise({
    applicable: true,
    visible: true,
    state: 'downloading',
    tone: 'neutral',
    bytes_received: 2000,
    bytes_total: 5000,
    cause: null,
    recovery_representable: true,
    command: null,
  });
  await loadPromise;
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'unavailable');

  env.context.getJson = async () => ({
    applicable: true,
    visible: true,
    state: 'verifying',
    tone: 'neutral',
    bytes_received: 5000,
    bytes_total: 5000,
    cause: null,
    recovery_representable: true,
    command: null,
  });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'verifying');
  assert.ok(env.speechSetup.classList.contains('speech-setup-tone-neutral'));
});

test('meaningful speech details keep their open state and focus across refresh', async () => {
  const env = createSpeechEnvironment();
  env.context.getJson = async () => ({
    applicable: true,
    visible: true,
    state: 'paused',
    tone: 'calm',
    bytes_received: null,
    bytes_total: null,
    cause: 'install_cancelled',
    recovery_representable: true,
    command: 'solstone journal install-provider parakeet',
  });
  await vm.runInContext('loadSpeechSetup()', env.context);

  env.detailsEl.open = true;
  env.summaryEl.focus();
  assert.strictEqual(env.detailsEl.open, true);
  assert.strictEqual(env.doc.activeElement, env.summaryEl);

  env.context.getJson = async () => ({
    applicable: true,
    visible: true,
    state: 'paused',
    tone: 'calm',
    bytes_received: null,
    bytes_total: null,
    cause: 'install_cancelled',
    recovery_representable: true,
    command: 'solstone journal install-provider parakeet',
  });
  await vm.runInContext('loadSpeechSetup()', env.context);

  assert.strictEqual(env.detailsEl.open, true);
  assert.strictEqual(env.doc.activeElement, env.summaryEl);
  assert.strictEqual(env.detailsEl.hidden, false);
});

test('applicable: false leaves row hidden and later rejection also leaves it hidden', async () => {
  const env = createSpeechEnvironment();
  env.context.getJson = async () => ({
    applicable: false,
    visible: false,
    state: null,
    tone: null,
    bytes_received: null,
    bytes_total: null,
    cause: null,
    recovery_representable: false,
    command: null,
  });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, true);

  env.context.getJson = async () => {
    throw new Error('fetch error');
  };
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, true);
});

test('mediaBacklogLine remains after speechSetup in brain card', async () => {
  const env = createSpeechEnvironment();
  const mediaBacklog = env.doc.createElement('div');
  mediaBacklog.id = 'mediaBacklogLine';
  env.brain.appendChild(mediaBacklog);

  env.context.getJson = async () => ({
    applicable: true,
    visible: true,
    state: 'downloading',
    tone: 'neutral',
    bytes_received: 1000,
    bytes_total: 5000,
    cause: null,
    recovery_representable: true,
    command: null,
  });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.nextSibling, mediaBacklog);
  const children = env.brain.children;
  assert.ok(children.indexOf(env.speechSetup) < children.indexOf(mediaBacklog));
});

test('primary Health settles before its independent media read, which renders positive and zero counts', async () => {
  const env = createSpeechEnvironment();
  const calls = [];
  let finishPrimary;
  let finishMedia;
  env.context.getJson = url => {
    calls.push(url);
    return new Promise(resolve => { finishPrimary = resolve; });
  };
  env.context.window.apiJson = url => {
    calls.push(url);
    return new Promise(resolve => { finishMedia = resolve; });
  };
  Object.assign(env.context, {
    clearHealthStateError() {}, renderWorkProgress() {}, renderBacklogState() {},
    renderAgentErrorsState() {}, renderHealthStateError() {},
  });
  vm.runInContext(sourceBlock(env.source, 'function renderMediaBacklog(', 'function renderWorkProgress('), env.context);
  vm.runInContext(sourceBlock(env.source, 'async function readHealthState()', 'let timeoutFired ='), env.context);
  const primary = vm.runInContext('readHealthState()', env.context);
  assert.deepStrictEqual(calls, ['/app/health/api/state']);
  finishPrimary({ work: {}, search_index: {}, backlog: {}, agent_errors: {} });
  await primary;
  assert.deepStrictEqual(calls, ['/app/health/api/state', '/app/health/api/media-backlog']);
  assert.strictEqual(env.doc.getElementById('mediaBacklogLine'), null);
  finishMedia({ segment_backlog: { not_sensed: 7 } });
  await Promise.resolve();
  await Promise.resolve();
  assert.ok(env.doc.getElementById('mediaBacklogLine').textContent.includes('7'));
  env.context.window.apiJson = async url => {
    calls.push(url);
    return { segment_backlog: { not_sensed: 0 } };
  };
  await vm.runInContext('loadMediaBacklog()', env.context);
  assert.strictEqual(env.doc.getElementById('mediaBacklogLine'), null);
  let reportedError = false;
  const mediaCallsBeforeFailure = calls.filter(url => url === '/app/health/api/media-backlog').length;
  env.context.getJson = async () => { throw new Error('primary failed'); };
  env.context.renderHealthStateError = () => { reportedError = true; };
  await vm.runInContext('readHealthState()', env.context);
  assert.ok(reportedError);
  assert.strictEqual(calls.filter(url => url === '/app/health/api/media-backlog').length, mediaCallsBeforeFailure + 1);
  assert.ok(!calls.includes('/api/health/summary'));
});

test('initHealth fires health:initial-ready while speech setup read is still pending', async () => {
  const env = createSpeechEnvironment();
  let initialReadyFired = false;
  env.doc.addEventListener('health:initial-ready', () => {
    initialReadyFired = true;
  });

  env.context.getJson = () => new Promise(() => {});
  env.context.requestAnimationFrame = (fn) => fn();
  env.context.healthInfoReady = Promise.resolve();
  env.context.registeredClientsReady = Promise.resolve();
  env.context.loadHealthState = () => Promise.resolve();
  env.context.loadBackupStatus = () => Promise.resolve();
  env.context.applyHealthCopy = () => {};
  env.context.focusBackupDetails = () => {};
  env.context.focusRecentErrors = () => {};
  env.context.runRecentErrorsFocus = () => {};
  env.context.initHealthRealtime = () => {};
  env.context.healthInitialized = false;

  const initHealthCode = sourceBlock(env.source, 'function initHealth()', "document.addEventListener('workspace:mounted'");
  vm.runInContext(initHealthCode, env.context);

  vm.runInContext('initHealth()', env.context);

  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));

  assert.strictEqual(initialReadyFired, true);
  assert.strictEqual(vm.runInContext('speechReadPending', env.context), true);
});

test('after successful applicable visible downloading payload, later getJson result of {} paints unavailable, clears bytes, and leaves row shown', async () => {
  const env = createSpeechEnvironment();
  env.context.getJson = async () => ({
    applicable: true,
    visible: true,
    state: 'downloading',
    tone: 'neutral',
    bytes_received: 1000,
    bytes_total: 5000,
    cause: null,
    recovery_representable: true,
    command: null,
  });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, false);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'downloading');
  assert.strictEqual(env.bytesEl.getAttribute('data-received'), '1000');
  assert.strictEqual(env.bytesEl.getAttribute('data-total'), '5000');

  env.context.getJson = async () => ({});
  await vm.runInContext('loadSpeechSetup()', env.context);

  assert.strictEqual(env.speechSetup.hidden, false);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'unavailable');
  assert.ok(env.speechSetup.classList.contains('speech-setup-tone-unavailable'));
  assert.strictEqual(env.bytesEl.hasAttribute('data-received'), false);
  assert.strictEqual(env.bytesEl.hasAttribute('data-total'), false);
  assert.strictEqual(env.bytesEl.hidden, true);
});

test('first load returning {} on an applicable host shows unavailable', async () => {
  const env = createSpeechEnvironment();
  env.context.getJson = async () => ({});
  await vm.runInContext('loadSpeechSetup()', env.context);

  assert.strictEqual(env.speechSetup.hidden, false);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'unavailable');
});

test('after successful applicable visible: false payload, later {applicable: 1} paints unavailable and does not leave row hidden', async () => {
  const env = createSpeechEnvironment();
  env.context.getJson = async () => ({
    applicable: true,
    visible: false,
    state: null,
    tone: null,
    bytes_received: null,
    bytes_total: null,
    cause: null,
    recovery_representable: false,
    command: null,
  });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, true);

  env.context.getJson = async () => ({ applicable: 1 });
  await vm.runInContext('loadSpeechSetup()', env.context);

  assert.strictEqual(env.speechSetup.hidden, false);
  assert.strictEqual(env.speechSetup.getAttribute('data-state'), 'unavailable');
  assert.ok(env.speechSetup.classList.contains('speech-setup-tone-unavailable'));
});

test('an initial rejected speech read is unavailable on an applicable host', async () => {
  const env = createSpeechEnvironment();
  env.context.getJson = async () => { throw new Error('read failed'); };
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, false);
  assert.strictEqual(env.speechSetup.dataset.state, 'unavailable');
  assert.strictEqual(env.bytesEl.hidden, true);
});

test('an initial rejected speech read stays absent on an unsupported host', async () => {
  const env = createSpeechEnvironment(false);
  env.context.getJson = async () => { throw new Error('read failed'); };
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, true);
});

test('malformed visible speech presentations clear progress and become unavailable', async () => {
  const env = createSpeechEnvironment();
  const progress = { applicable: true, visible: true, state: 'downloading', tone: 'neutral',
    bytes_received: 10, bytes_total: 20, cause: null, recovery_representable: true, command: null };
  for (const bad of [
    { ...progress, visible: null },
    { ...progress, state: 'future-state' },
    { ...progress, state: 'toString' },
    { ...progress, tone: 'future-tone' },
    { ...progress, bytes_received: -1 },
    { ...progress, command: {} },
  ]) {
    env.context.getJson = async () => progress;
    await vm.runInContext('loadSpeechSetup()', env.context);
    env.context.getJson = async () => bad;
    await vm.runInContext('loadSpeechSetup()', env.context);
    assert.strictEqual(env.speechSetup.dataset.state, 'unavailable');
    assert.strictEqual(env.bytesEl.hidden, true);
  }
});

test('empty speech details disappear and resolved setup keeps focus in processing', async () => {
  const env = createSpeechEnvironment();
  const paused = { applicable: true, visible: true, state: 'paused', tone: 'calm',
    bytes_received: null, bytes_total: null, cause: 'install_cancelled', recovery_representable: true,
    command: 'solstone journal install-provider parakeet' };
  env.context.getJson = async () => paused;
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.detailsEl.hidden, false);
  env.summaryEl.focus();
  env.context.getJson = async () => ({ ...paused, state: 'downloading', tone: 'neutral', cause: null, command: null });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.detailsEl.hidden, true);
  assert.strictEqual(env.doc.activeElement, env.doc.getElementById('speechSetupState'));
  env.context.getJson = async () => ({ ...paused, visible: false, state: null, tone: null, command: null });
  await vm.runInContext('loadSpeechSetup()', env.context);
  assert.strictEqual(env.speechSetup.hidden, true);
  assert.strictEqual(env.doc.activeElement, env.doc.getElementById('brainHealthStatus'));
});


test('local TCP programs render untrusted product names as text and replace stale rows', async () => {
  const { doc, context } = createEnvironment();
  const section = doc.createElement('section'); section.id = 'localPrograms';
  const rows = doc.createElement('div'); rows.setAttribute('data-local-programs', '');
  section.appendChild(rows); doc.body.appendChild(section);
  context.healthInfoReady = Promise.resolve(); context.state.localProgramsAvailable = true;
  const product = '<img src=x onerror=bad()>';
  context.window.apiJson = async () => ({ items: [{ program: product, count: 12, last_seen: 1 }] });
  vm.runInContext(sourceBlock(loadHealthSource(), 'async function loadLocalPrograms()', 'function refreshStatusReads()'), context);
  await vm.runInContext('loadLocalPrograms()', context);
  assert.strictEqual(section.hidden, false);
  assert.strictEqual(rows.children.length, 1);
  assert.ok(rows.children[0].textContent.includes(product));
  context.window.apiJson = async () => ({ items: [] });
  await vm.runInContext('loadLocalPrograms()', context);
  assert.strictEqual(rows.children.length, 0);
});

test('local TCP programs do not request unavailable transports and surface read failures', async () => {
  const { doc, context } = createEnvironment();
  const section = doc.createElement('section'); section.id = 'localPrograms';
  const rows = doc.createElement('div'); rows.setAttribute('data-local-programs', '');
  section.appendChild(rows); doc.body.appendChild(section);
  context.healthInfoReady = Promise.resolve(); context.state.localProgramsAvailable = false;
  let requests = 0; context.window.apiJson = async () => { requests += 1; throw new Error('offline'); };
  vm.runInContext(sourceBlock(loadHealthSource(), 'async function loadLocalPrograms()', 'function refreshStatusReads()'), context);
  await vm.runInContext('loadLocalPrograms()', context);
  assert.strictEqual(requests, 0); assert.strictEqual(section.hidden, true);
  context.state.localProgramsAvailable = true;
  await vm.runInContext('loadLocalPrograms()', context);
  assert.strictEqual(requests, 1); assert.strictEqual(section.hidden, false);
  assert.ok(rows.textContent.length > 0);
});

// A clock the cases move by hand: timers run in due order, one at a time.
function manualClock() {
  let now = 0;
  let nextId = 1;
  const timers = new Map();
  return {
    setTimeout: (fn, ms) => {
      const id = nextId++;
      timers.set(id, { fn, due: now + Math.max(0, Number(ms) || 0) });
      return id;
    },
    clearTimeout: (id) => { timers.delete(id); },
    pending: () => timers.size,
    advance(ms) {
      const until = now + ms;
      for (;;) {
        let next = null;
        for (const [id, timer] of timers) if (timer.due <= until && (!next || timer.due < next[1].due)) next = [id, timer];
        if (!next) break;
        timers.delete(next[0]);
        now = next[1].due;
        next[1].fn();
      }
      now = until;
    },
  };
}

function domEvent(type, bubbles = false) {
  return { type, bubbles, cancelBubble: false, defaultPrevented: false,
    preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.cancelBubble = true; } };
}

function listenerCount(target) {
  return Object.values(target.listeners).reduce((total, list) => total + list.length, 0);
}

// The log card's filters and lines, with the page's own change handlers and
// the shell's focus helpers over a hand-moved clock.
function logFilterEnvironment() {
  const source = loadHealthSource();
  const doc = new Document();
  const clock = manualClock();
  const requests = [];
  const ask = (url) => { requests.push(String(url)); return new Promise(() => {}); };
  const card = doc.createElement('section');
  card.setAttribute('class', 'logs-card');
  doc.body.appendChild(card);
  const controls = doc.createElement('div');
  controls.setAttribute('class', 'logs-controls');
  card.appendChild(controls);
  const select = (id, values) => {
    const el = doc.createElement('select');
    el.id = id;
    for (const value of values) {
      const option = doc.createElement('option');
      option.value = value;
      el.appendChild(option);
    }
    el.value = values[0];
    controls.appendChild(el);
    return el;
  };
  const part = (tag) => { const el = doc.createElement(tag); card.appendChild(el); return el; };
  const elements = {
    logServiceFilter: select('logServiceFilter', ['all']),
    logLevelFilter: select('logLevelFilter', ['all', 'error', 'warning', 'info']),
    logStreamFilter: select('logStreamFilter', ['all', 'stdout', 'stderr']),
    logsViewport: part('div'),
    logsAnnouncer: part('p'),
    logErrorBadge: part('span'),
    logsSummaryBadge: part('span'),
    logsCollapseIndicator: part('span'),
  };
  const window = { setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout, fetch: ask, apiJson: ask, location: { search: '' } };
  const context = vm.createContext({
    window, document: doc, elements, console, CSS: { escape: String },
    state: { logLevelFilter: 'all', serviceLogs: new Map(), logCollapsedServices: new Map(), logTotalCount: 0,
      logErrorCount: 0, lastLogTs: null, lastLogFilter: null, logsCollapsed: false, deepLinkMode: false,
      logFollow: true, pendingLogAnchor: null },
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout, requestAnimationFrame: (fn) => fn(),
    fetch: ask, getJson: ask, updateLogsBadge() {},
  });
  loadHealthCatalog(context);
  vm.runInContext(focusSource, context, { filename: 'focus.js' });
  vm.runInContext(sourceBlock(source, 'const HEALTH_LOGS_COPY =', 'const HEALTH_GLANCE_COPY ='), context);
  vm.runInContext(sourceBlock(source, 'const SERVICE_NAMES =', '// Readable names for talent ids'), context);
  vm.runInContext('let programmaticScroll = false;', context);
  vm.runInContext(sourceBlock(source, 'function isAtBottom(', 'function recentErrorGroupKey('), context);
  vm.runInContext(sourceBlock(source, 'const LOG_BUFFER_SIZE =', '// Event handlers by tract'), context);
  const logLine = (name, line, stream = 'stdout') => context.handleLogsEvent({ event: 'line', name, line, stream, ts: 1 });
  logLine('cortex', 'INFO cortex ready');
  logLine('observer', 'WARNING observer slow', 'stderr');
  vm.runInContext(sourceBlock(source, '// Log controls', "elements.logsViewport.addEventListener('scroll'"), context);
  const pick = (el, value) => {
    el.focus();
    el.value = value;
    el.dispatchEvent(domEvent('change', true));
  };
  // A phone's late blur: focus goes to the page, with nothing pressed.
  const lateBlur = (el) => {
    doc.activeElement = doc.body;
    el.dispatchEvent(domEvent('blur'));
    // focusout reaches the document too, as in a browser (this model's
    // bubbling stops at the root element).
    const out = domEvent('focusout', true);
    el.dispatchEvent(out);
    doc.dispatchEvent(out);
  };
  const listeners = () => listenerCount(doc) + Object.values(elements).reduce((total, el) => total + listenerCount(el), 0);
  return { doc, elements, clock, requests, logLine, pick, lateBlur, listeners };
}

const LOG_FILTER_PICKS = [
  { id: 'logServiceFilter', value: 'cortex', blurAt: 865 },
  { id: 'logLevelFilter', value: 'error', blurAt: 729 },
  { id: 'logStreamFilter', value: 'stderr', blurAt: 865 },
];

for (const { id, value, blurAt } of LOG_FILTER_PICKS) {
  test(`a phone's late blur after a pick from ${id} lands back on that same filter`, () => {
    const env = logFilterEnvironment();
    const filter = env.doc.getElementById(id);
    const before = env.listeners();
    env.pick(filter, value);
    assert.ok(env.doc.activeElement === filter, 'the pick leaves focus on the filter');
    env.clock.advance(400);
    // A log line from a service not seen yet adds an option to the service filter.
    const options = env.elements.logServiceFilter.children.length;
    env.logLine('importer', 'INFO importer started');
    assert.strictEqual(env.elements.logServiceFilter.children.length, options + 1);
    env.clock.advance(blurAt - 400);
    env.lateBlur(filter);
    assert.ok(env.doc.activeElement === env.doc.body, 'the blur leaves focus on the page');
    env.clock.advance(0);
    assert.ok(env.doc.activeElement === filter, 'focus is back on the filter it left');
    assert.ok(env.doc.getElementById(id) === filter, 'the filter was not drawn again');
    assert.strictEqual(env.listeners(), before);
    assert.strictEqual(env.clock.pending(), 0);
  });

  test(`a pick from ${id} with no late blur keeps focus, leaves no wait behind and asks the server for nothing`, () => {
    const env = logFilterEnvironment();
    const filter = env.doc.getElementById(id);
    const before = env.listeners();
    env.pick(filter, value);
    env.clock.advance(2000);
    assert.ok(env.doc.activeElement === filter, 'focus stays on the filter');
    assert.strictEqual(env.listeners(), before);
    assert.strictEqual(env.clock.pending(), 0);
    assert.strictEqual(env.requests.length, 0);
  });
}

test('a tap after a level pick and before the late blur leaves focus on the page', () => {
  const env = logFilterEnvironment();
  const filter = env.elements.logLevelFilter;
  const before = env.listeners();
  env.pick(filter, 'warning');
  env.clock.advance(300);
  env.doc.dispatchEvent(domEvent('pointerdown'));
  env.clock.advance(429);
  env.lateBlur(filter);
  env.clock.advance(0);
  assert.ok(env.doc.activeElement === env.doc.body, 'the tap ended the wait');
  assert.strictEqual(env.listeners(), before);
  assert.strictEqual(env.clock.pending(), 0);
});

function createIntegrityEnvironment() {
  const source = loadHealthSource();
  const doc = new Document();
  const section = doc.createElement('section');
  section.id = 'integrity';
  const badge = doc.createElement('span'); badge.id = 'integrityBadge';
  const actions = doc.createElement('div'); actions.id = 'integrityActions';
  const headline = doc.createElement('p'); headline.id = 'integrityHeadline';
  const meta = doc.createElement('p'); meta.id = 'integrityMeta';
  const checksumLine = doc.createElement('p'); checksumLine.id = 'integrityChecksumLine';
  const scopeNote = doc.createElement('p'); scopeNote.id = 'integrityScopeNote';
  const actionStatus = doc.createElement('p'); actionStatus.id = 'integrityActionStatus';
  const findings = doc.createElement('div'); findings.id = 'integrityFindings';
  const older = doc.createElement('div'); older.id = 'integrityOlderMissing';
  const coverage = doc.createElement('details'); coverage.id = 'integrityCoverage';
  section.append(badge, actions, headline, meta, checksumLine, scopeNote, actionStatus, findings, older, coverage);
  doc.body.appendChild(section);

  const posts = [];
  const confirms = [];
  let confirmAnswer = true;
  let nextResponse = { status: 200, ok: true, body: { status: 'ok', integrity: { state: 'ok', checked_at_ms: 1778760000000, days_checked: 1, days_total: 1, days_unreadable: 0, incomplete: false, findings: [], older_missing_originals: [] } } };
  let summaryUpdates = 0;

  const context = vm.createContext({
    document: doc,
    window: {
      JournalFormat: {
        day: (d) => `day-${d}`,
        timestamp: (iso) => `ts-${iso}`,
      },
      SolQuestion: {
        async ask(opts) {
          confirms.push(opts);
          return confirmAnswer;
        },
      },
    },
    state: { backupConfigured: false, integrity: null, journalDisk: null, integrityBusy: false },
    fetch: async (url, init = {}) => {
      posts.push({ url, method: init.method || 'GET', body: init.body ? JSON.parse(init.body) : null });
      const r = nextResponse;
      return {
        status: r.status,
        ok: r.ok,
        json: async () => r.body,
      };
    },
    updateStatusSummary: () => { summaryUpdates += 1; },
    logHealthError: () => {},
    console,
    encodeURIComponent,
    Date,
    Math,
    Number,
    String,
    Array,
    Object,
    Set,
  });
  loadHealthCatalog(context);
  vm.runInContext(
    sourceBlock(source, '// ========== JOURNAL INTEGRITY ==========', '// ========== END JOURNAL INTEGRITY =========='),
    context,
  );
  return {
    doc,
    context,
    section,
    badge,
    actions,
    headline,
    meta,
    checksumLine,
    scopeNote,
    actionStatus,
    findings,
    older,
    coverage,
    posts,
    confirms,
    setConfirm: (val) => { confirmAnswer = val; },
    setNextResponse: (resp) => { nextResponse = resp; },
    summaryUpdates: () => summaryUpdates,
  };
}

test('integrity card renders all 7 verdict states and synthesizes live low disk even when not_checked_yet', () => {
  const env = createIntegrityEnvironment();
  const render = (receipt, disk = null) => {
    env.context.r = receipt;
    env.context.d = disk;
    vm.runInContext('renderIntegrityCard(r, d);', env.context);
  };

  // 1. ok routine (checksums null)
  render({ state: 'ok', checked_at_ms: 1778760000000, days_checked: 10, days_total: 10, days_unreadable: 0, incomplete: false, findings: [], older_missing_originals: [], checksums: null });
  assert.strictEqual(env.section.dataset.verdict, 'ok');
  assert.strictEqual(env.badge.dataset.tone, 'ok');
  assert.strictEqual(env.scopeNote.hidden, false);
  assert.strictEqual(env.checksumLine.hidden, true);

  // 2. ok deep checksums full
  render({ state: 'ok', checked_at_ms: 1778760000000, days_checked: 10, days_total: 10, days_unreadable: 0, incomplete: false, findings: [], older_missing_originals: [], checksums: { verified_at_ms: 1778760000000, checksums_verified: 5, checksums_total_kept: 5, checksums_unavailable_older: 0, mismatches: 0, incomplete: false } });
  assert.strictEqual(env.section.dataset.verdict, 'ok');
  assert.strictEqual(env.checksumLine.hidden, false);
  assert.strictEqual(env.scopeNote.hidden, true);

  // 2b. ok checksums with 0 kept originals keeps routine headline (never claims "all kept originals match")
  render({ state: 'ok', checked_at_ms: 1778760000000, days_checked: 10, days_total: 10, days_unreadable: 0, incomplete: false, findings: [], older_missing_originals: [], checksums: { verified_at_ms: 1778760000000, checksums_verified: 0, checksums_total_kept: 0, checksums_unavailable_older: 0, mismatches: 0, incomplete: false } });
  assert.strictEqual(env.headline.textContent, env.context.window.JournalI18n.render('journal.window.health.integrity.headline.ok.routine'));
  assert.strictEqual(env.checksumLine.hidden, false);

  // 3. ok deep checksums partial
  render({ state: 'ok', checked_at_ms: 1778760000000, days_checked: 10, days_total: 10, days_unreadable: 0, incomplete: false, findings: [], older_missing_originals: [], checksums: { verified_at_ms: 1778760000000, checksums_verified: 5, checksums_total_kept: 8, checksums_unavailable_older: 3, mismatches: 0, incomplete: false } });
  assert.strictEqual(env.section.dataset.verdict, 'ok');
  assert.strictEqual(env.headline.textContent, env.context.window.JournalI18n.render('journal.window.health.integrity.headline.ok.checksums.partial'));

  // 4. incomplete (never green; offers finish-check + check-from-start actions)
  render({ state: 'incomplete', checked_at_ms: 1778760000000, days_checked: 4, days_total: 10, days_unreadable: 2, incomplete: true, findings: [], older_missing_originals: [] });
  assert.strictEqual(env.section.dataset.verdict, 'incomplete');
  assert.strictEqual(env.badge.dataset.tone, 'warn');
  assert.ok(env.actions.querySelector('[data-integrity-action="finish-check"]'));
  assert.ok(env.actions.querySelector('[data-integrity-action="check-from-start"]'));
  assert.strictEqual(env.scopeNote.hidden, false);

  // 5. not_checked_yet (calm neutral hollow ring, never green)
  render({ state: 'not_checked_yet', checked_at_ms: null, days_checked: 0, days_total: 0, days_unreadable: 0, incomplete: false, findings: [], older_missing_originals: [] });
  assert.strictEqual(env.section.dataset.verdict, 'not_checked_yet');
  assert.strictEqual(env.badge.dataset.tone, 'neutral');
  assert.ok(env.badge.querySelector('.integrity-badge-ring'));

  // 5b. not_checked_yet + live journal_disk.low promotes to needs_attention with /app/settings/#storage
  render(
    { state: 'not_checked_yet', checked_at_ms: null, days_checked: 0, days_total: 0, days_unreadable: 0, incomplete: false, findings: [], older_missing_originals: [] },
    { free_bytes: 5000000000, total_bytes: 100000000000, floor_bytes: 20000000000, percent_used: 95, low: true },
  );
  assert.strictEqual(env.section.dataset.verdict, 'needs_attention');
  assert.strictEqual(env.badge.dataset.tone, 'danger');
  assert.ok(env.findings.querySelector('[data-finding-kind="low_disk_space"] a[href="/app/settings/#storage"]'));

  // 6. unavailable (amber, never green)
  render({ state: 'unavailable', unavailable: true, checked_at_ms: null, findings: [] });
  assert.strictEqual(env.section.dataset.verdict, 'unavailable');
  assert.strictEqual(env.badge.dataset.tone, 'warn');
  assert.strictEqual(env.coverage.hidden, true);

  // 7. maintenance (0 Tier 1, 1 Tier 2 undeclared facet)
  render({ state: 'maintenance', checked_at_ms: 1778760000000, days_checked: 5, days_total: 5, days_unreadable: 0, incomplete: false, findings: [{ tier: 2, kind: 'undeclared_facet', facet: 'garden', path: 'facets/garden' }], older_missing_originals: [] });
  assert.strictEqual(env.section.dataset.verdict, 'maintenance');
  assert.strictEqual(env.badge.dataset.tone, 'warn');
});

test('integrity findings wire recovery actions, confirm dialogs, non-chronicle import safety, backup gating, and older_missing_originals', async () => {
  const env = createIntegrityEnvironment();
  env.context.state.backupConfigured = true;
  env.context.r = {
    state: 'needs_attention',
    checked_at_ms: 1778760000000,
    days_checked: 4,
    days_total: 10,
    days_unreadable: 0,
    incomplete: true,
    findings: [
      { tier: 1, kind: 'damaged_transcript_jsonl', path: 'chronicle/20261120/import.chatgpt/090000_60/conversation_transcript.jsonl', day: '20261120', stream: 'import.chatgpt', segment: '090000_60', valid_lines: 3, damaged_lines: 1, truncated_tail: true, interior_damaged: 0 },
      { tier: 1, kind: 'missing_original', path: 'chronicle/20261120/default/100000_60', day: '20261120', stream: 'default', segment: '100000_60', files: ['audio.ogg'] },
      { tier: 1, kind: 'zero_byte_original', path: 'chronicle/20261120/default/110000_60/audio.ogg', day: '20261120', stream: 'default', segment: '110000_60', file: 'audio.ogg' },
      { tier: 1, kind: 'checksum_mismatch', path: 'imports/imp1/originals/file.m4a', file: 'file.m4a' },
      { tier: 1, kind: 'quarantined_ingest', path: 'chronicle/20261120/observer/failed/bad.flac', day: '20261120', file: 'bad.flac' },
      { tier: 1, kind: 'damaged_core_record', path: 'entities/retired.json' },
      { tier: 1, kind: 'operational_heal_recurrence', heal_count: 3, logs: ['health/a.jsonl'] },
      { tier: 1, kind: 'search_index_issue', search_reason: 'retained_prune_guarded' },
      { tier: 1, kind: 'search_index_issue', search_reason: 'edges_failed' },
      { tier: 1, kind: 'search_index_issue', search_reason: 'newer_generation' },
      { tier: 1, kind: 'search_index_issue', search_reason: 'repeated_rebuild' },
      { tier: 1, kind: 'search_index_issue', search_reason: 'check_overdue' },
      { tier: 2, kind: 'undeclared_facet', facet: 'garden', path: 'facets/garden' },
    ],
    older_missing_originals: [
      { path: 'chronicle/20260701/default/080000_60', day: '20260701', stream: 'default', segment: '080000_60', files: ['audio.ogg'] },
    ],
  };
  vm.runInContext('renderIntegrityCard(r, null);', env.context);

  assert.strictEqual(env.section.dataset.verdict, 'needs_attention');
  assert.ok(!env.section.querySelector('a[href*="/app/entities"]'), 'no Entities duplicate link inside Status integrity');
  assert.ok(env.findings.querySelector('[data-finding-kind="missing_original"] a[href="/app/health/#backup"]'), 'backup link shown when configured');

  // Non-chronicle import checksum_mismatch shows /app/import/ link and NO trash-original button.
  const importMismatch = env.findings.querySelector('[data-finding-kind="checksum_mismatch"]');
  assert.ok(importMismatch.querySelector('a[href="/app/import/"]'));
  assert.strictEqual(importMismatch.querySelector('[data-integrity-action="trash-original"]'), null);

  // newer_generation and repeated_rebuild search findings have NO rebuild button; check_overdue uses search-update.
  const searchRows = env.findings.querySelectorAll('[data-finding-kind="search_index_issue"]');
  assert.strictEqual(searchRows.length, 5);
  assert.ok(searchRows[0].querySelector('[data-integrity-action="search-clear-removed"]'));
  assert.ok(searchRows[1].querySelector('[data-integrity-action="search-update"]'));
  assert.strictEqual(searchRows[2].querySelector('button'), null);
  assert.strictEqual(searchRows[3].querySelector('button'), null);
  assert.ok(searchRows[4].querySelector('[data-integrity-action="search-update"]'));

  // Set-aside button fires SolQuestion confirm and POSTs to set-aside-lines.
  const setAsideBtn = env.findings.querySelector('[data-integrity-action="set-aside-lines"]');
  assert.ok(setAsideBtn, 'conversation_transcript.jsonl is set-aside eligible');
  setAsideBtn.dispatchEvent(domEvent('click'));
  await new Promise(r => setTimeout(r, 10));
  assert.strictEqual(env.confirms.length, 1);
  assert.strictEqual(env.posts[0].url, '/app/health/api/integrity/set-aside-lines');
  assert.deepStrictEqual(env.posts[0].body, { paths: ['chronicle/20261120/import.chatgpt/090000_60/conversation_transcript.jsonl'] });
  assert.strictEqual(env.summaryUpdates(), 1);
});

test('selectGlanceSentence folds urgent, incomplete, and unavailable integrity states', () => {
  const source = loadHealthSource();
  const context = vm.createContext({
    window: { JournalI18n: null },
    brainSnapshot: null,
    connectError: false,
    STALE_MS: 30000,
    GLANCE_DEVICES_ACTION: { href: '#registeredClientsCard', label: 'view devices' },
    selectDeviceVerdict: () => null,
    heldWorkVerdict: () => null,
    serviceName: (s) => s,
    relativeTime: (ms) => `${ms}ms`,
    ageAgo: () => 'now',
    Array,
    Set,
    Map,
    Number,
    String,
  });
  loadHealthCatalog(context);
  vm.runInContext(sourceBlock(source, 'function selectIntegrityGlance(', 'function formatGlanceSentence('), context);

  const base = () => ({
    agents: new Map(),
    imports: new Map(),
    crashed: new Map(),
    clients: new Map(),
    services: new Map([['convey', {}]]),
    connected: true,
    health: {},
    backupView: { tone: 'neutral', text: '' },
    searchIndex: null,
    storagePrivacy: null,
    integrity: null,
    journalDisk: null,
  });

  context.s = { ...base(), integrity: { state: 'ok', findings: [] } };
  assert.strictEqual(vm.runInContext('selectGlanceSentence(s, 1000).key', context), 'HEALTH_GLANCE_OK');

  context.s = { ...base(), integrity: { state: 'incomplete', incomplete: true, findings: [] } };
  assert.strictEqual(vm.runInContext('selectGlanceSentence(s, 1000).key', context), 'HEALTH_GLANCE_INTEGRITY_INCOMPLETE');

  context.s = { ...base(), integrity: { state: 'unavailable', unavailable: true, findings: [] } };
  assert.strictEqual(vm.runInContext('selectGlanceSentence(s, 1000).key', context), 'HEALTH_GLANCE_INTEGRITY_UNAVAILABLE');

  context.s = { ...base(), integrity: { state: 'not_checked_yet', findings: [] }, journalDisk: { low: true } };
  assert.strictEqual(vm.runInContext('selectGlanceSentence(s, 1000).key', context), 'HEALTH_GLANCE_INTEGRITY_ATTENTION');
});

test('media backlog error renders honest message instead of silent 0', async () => {
  const doc = new Document();
  const brain = doc.createElement('section');
  brain.id = 'brain';
  doc.body.appendChild(brain);
  const context = vm.createContext({
    document: doc,
    window: {
      apiJson: async () => { throw new Error('network down'); },
    },
    fetch: async () => { throw new Error('network down'); },
    Number,
    String,
  });
  loadHealthCatalog(context);
  vm.runInContext(sourceBlock(loadHealthSource(), 'function renderMediaBacklog(', 'function renderWorkProgress('), context);
  await vm.runInContext('loadMediaBacklog()', context);
  const line = doc.getElementById('mediaBacklogLine');
  assert.ok(line, 'error line is rendered');
  assert.strictEqual(line.dataset.state, 'error');
  assert.strictEqual(line.textContent, context.window.JournalI18n.render('journal.window.health.backlog.media.error'));
});

test('processing holds hide try again for unretryable corrupt_input and no_audio_stream reasons', () => {
  const source = loadHealthSource();
  const doc = new Document();
  const section = doc.createElement('section'); section.id = 'processingHolds';
  const notice = doc.createElement('p'); notice.setAttribute('data-held-work-notice', '');
  const list = doc.createElement('details'); list.setAttribute('data-held-work-list', '');
  const rows = doc.createElement('div'); rows.setAttribute('data-held-work-rows', '');
  const pages = doc.createElement('nav'); pages.setAttribute('data-held-work-pages', '');
  const prev = doc.createElement('button'); prev.setAttribute('data-held-previous', '');
  const span = doc.createElement('span');
  const next = doc.createElement('button'); next.setAttribute('data-held-next', '');
  pages.append(prev, span, next);
  list.append(rows, pages);
  const support = doc.createElement('p'); support.setAttribute('data-held-work-support', '');
  section.append(notice, list, support);
  doc.body.appendChild(section);

  const context = vm.createContext({
    document: doc,
    window: { location: { hash: '' } },
    state: {
      work: {
        inventory: {
          as_of_ms: Date.now(),
          holds: [
            { retry_unit: 'u1', day: '20261120', stream: 'default', segment: '100000_60', reason: 'corrupt_input', count: 1 },
            { retry_unit: 'u2', day: null, stream: 'default', segment: '103000_60', reason: 'no_audio_stream', count: 1 },
            { retry_unit: 'u3', day: '20261120', stream: 'default', segment: '110000_60', reason: 'analysis_failed', count: 1 },
          ],
        },
        retry_records: [],
      },
      legacyHolds: [],
      heldWorkSnapshot: null,
      heldWorkCurrent: true,
    },
    talentName: () => '',
    scheduleName: (s) => s,
    loadHealthState: async () => {},
    requestAnimationFrame: (fn) => fn(),
    fetch: async () => ({ ok: true, json: async () => ({}) }),
    Date,
    JSON,
    Array,
    Set,
    Map,
    Number,
    String,
    Math,
    encodeURIComponent,
  });
  loadHealthCatalog(context);
  vm.runInContext(sourceBlock(source, 'function queuedWorkHolds(', 'function selectIntegrityGlance('), context);
  vm.runInContext('renderHeldWork();', context);

  const rowEls = rows.children;
  assert.strictEqual(rowEls.length, 3);
  const corruptRow = rowEls.find(r => r.dataset.key === 'unit:u1');
  const noAudioRow = rowEls.find(r => r.dataset.key === 'unit:u2');
  const retryableRow = rowEls.find(r => r.dataset.key === 'unit:u3');
  assert.strictEqual(corruptRow.querySelector('button').hidden, true, 'try again hidden for corrupt_input');
  assert.strictEqual(corruptRow.querySelector('[data-hold-guidance]').hidden, false);
  assert.ok(corruptRow.querySelector('[data-hold-guidance] a[href="/app/transcripts/20261120"]'), 'links to transcripts day');
  assert.strictEqual(noAudioRow.querySelector('button').hidden, true, 'try again hidden for no_audio_stream');
  assert.strictEqual(noAudioRow.querySelector('[data-hold-guidance]').hidden, false);
  assert.strictEqual(noAudioRow.querySelector('[data-hold-guidance] a'), null, 'no dead link when day absent');
  assert.strictEqual(retryableRow.querySelector('button').hidden, false, 'try again visible for retryable hold');
  assert.strictEqual(retryableRow.querySelector('[data-hold-guidance]').hidden, true);
});

test('backlog search note surfaces retained_prune_guarded and edges_failed with link to #integrity', () => {
  const { doc, context } = createEnvironment();
  const note = doc.createElement('p');
  note.setAttribute('class', 'backlog-search-note');
  note.hidden = true;
  doc.getElementById('backlogVerdict').appendChild(note);

  context.payload = { verdict: 'caught up' };
  context.searchIndex = {
    state: 'ok',
    text: '',
    index: { retained_prune_guarded: true, gone_days: 5, indexed_days: 12, edges_failed: true },
  };
  vm.runInContext('renderBacklogState(payload, searchIndex);', context);
  assert.strictEqual(note.hidden, false);
  assert.ok(note.querySelector('a[href="#integrity"]'));
});

test('importer card renders open-import link instead of dead retry-import POST', () => {
  const source = loadHealthSource();
  assert.ok(!source.includes('/app/health/api/retry-import'), 'dead retry-import endpoint is no longer called by health.js');
  assert.ok(source.includes('data-action", "open-import"') || source.includes("setAttribute('data-action', 'open-import')"), 'failed import card renders open-import link');
});

async function runAsyncCases() {
  for (const runCase of asyncCases) await runCase();
  console.log('DOM CASES: ' + cases + ' passed');
}

runAsyncCases().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
