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
  set href(value) { this.setAttribute('href', value); }
  get className() { return this.getAttribute('class') || ''; }
  set className(value) { this.setAttribute('class', value); }
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
  scrollIntoView() { this.scrolledIntoView = true; }
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
  getElementById(id) { return this.querySelector('#' + id); }
  querySelector(selector) {
    if (selector === 'body') return this.body;
    return this.documentElement.querySelector(selector);
  }
  querySelectorAll(selector) { return this.documentElement.querySelectorAll(selector); }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
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
    document: doc,
    window: {
      JournalFormat: { day: (d) => `Apr 3, 2026` },
      location: { href: '' },
    },
    console,
    encodeURIComponent,
    Number,
    String,
    Array,
    Object,
  });

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
  const start = source.indexOf('function selectGlanceSentence(');
  const end = source.indexOf('function formatGlanceSentence(');
  assert.ok(start >= 0 && end > start, 'selectGlanceSentence found in health.js');
  return source.slice(start, end);
}

function glanceFor(crashed, staleHeartbeats, searchIndex = null, attention = {}) {
  const context = vm.createContext({ Array, Object, String, Number, Math, Set, Map });
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
      connected: true,
    };
    ${extractGlanceFunction(loadHealthSource())}
    result = selectGlanceSentence(state, 0);
  `, context);
  return context.result;
}

test('a stale check-in is not counted as a failing service', async () => {
  const both = glanceFor(['observe'], ['work-laptop (/home/owner/journal)']);
  assert.strictEqual(both.key, 'HEALTH_GLANCE_SERVICE_ATTENTION');
  assert.strictEqual(both.vars.n, '1');
  assert.ok(!both.vars.service_names.includes('laptop'), both.vars.service_names);

  const staleOnly = glanceFor([], ['work-laptop (/home/owner/journal)']);
  assert.strictEqual(staleOnly.key, 'HEALTH_GLANCE_CHECKIN_STALE');
  assert.strictEqual(staleOnly.vars.name, 'work-laptop (/home/owner/journal)');

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

test('loaded search status reaches the headline and survives a later state-read error', async () => {
  const { context } = createEnvironment();
  const source = loadHealthSource();
  const loaderStart = source.indexOf('async function loadHealthState(');
  const loaderEnd = source.indexOf('let timeoutFired', loaderStart);
  const errorsStart = source.indexOf('function renderAgentErrorsState(');
  const errorsEnd = source.indexOf('function renderMediaBacklog(', errorsStart);
  assert.ok(loaderStart >= 0 && loaderEnd > loaderStart && errorsStart >= 0 && errorsEnd > errorsStart);
  context.state = {};
  context.elements = {};
  context.loadMediaBacklog = () => {};
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
    clients: new Map(), localHost: 'local', agents: new Map(), imports: new Map(),
    services: new Map([['observe', {}]]), crashed: new Map(), health: {}, connected: true,
  };
  const context = vm.createContext({
    document: doc, elements, state, Date: ControlledDate, clock, console: { warn() {} },
    window: { location: { hash: '' }, JournalFormat: { sinceDay: () => 'synthetic day' } },
    requestAnimationFrame: fn => fn(), CustomEvent: class { constructor(type) { this.type = type; } },
    STALE_MS: 30000, brainSnapshot: null, connectError: false,
    serviceName: value => value, relativeTime: value => String(value), ageAgo: value => String(value),
    fetch: async () => ({ ok: true, json: async () => ({ clients: [] }) }),
  });
  context.updateStatusSummary = () => {
    context.selection = vm.runInContext('selectGlanceSentence(state, Date.now())', context);
  };
  vm.runInContext([
    sourceBlock(source, 'const HEALTH_GLANCE_COPY =', 'let brainSnapshot ='),
    sourceBlock(source, 'function registeredClientName(', 'function formatGlanceSentence('),
    sourceBlock(source, 'const sinceDay =', 'function requestBacklogReprocess('),
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
  assert.strictEqual(env.verdict().vars.n, '2');
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
  assert.strictEqual(env.doc.querySelectorAll('.registered-client-row').length, 1);
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
  assert.ok(env.elements.observeSourceNote.textContent.includes('remote'));
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

async function runAsyncCases() {
  for (const runCase of asyncCases) await runCase();
  console.log('DOM CASES: ' + cases + ' passed');
}

runAsyncCases().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
