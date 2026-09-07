// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const nodeAssert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

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

class ClassList {
  constructor(element) {
    this.element = element;
    this.values = new Set();
  }

  setFromString(value) {
    this.values = new Set(String(value || '').split(/\s+/).filter(Boolean));
  }

  add(...names) {
    names.forEach((name) => this.values.add(name));
  }

  remove(...names) {
    names.forEach((name) => this.values.delete(name));
  }

  toggle(name, force) {
    const present = force === undefined ? !this.values.has(name) : force;
    if (present) this.values.add(name);
    else this.values.delete(name);
    return present;
  }

  contains(name) {
    return this.values.has(name);
  }

  toString() {
    return Array.from(this.values).join(' ');
  }
}

class Element {
  constructor(id = '', dataset = {}, tag = '') {
    this.tag = String(tag || '').toLowerCase();
    this.id = id;
    this.dataset = {...dataset};
    this.hidden = false;
    this.isConnected = true;
    this.tabIndex = 0;
    this.textContent = '';
    this.classList = new ClassList(this);
    this.children = [];
    this.listeners = {};
    this.attributes = {};
    this.style = {};
    this.parent = null;
  }

  get className() {
    return this.classList.toString();
  }

  set className(value) {
    this.classList.setFromString(value);
  }

  append(...children) {
    children.forEach((child) => this.appendChild(child));
  }

  appendChild(child) {
    child.parent = this;
    this.children.push(child);
    return child;
  }

  replaceChildren(...children) {
    this.children = [];
    this.append(...children);
  }

  addEventListener(name, listener) {
    (this.listeners[name] ||= []).push(listener);
  }

  emit(name, event = {}) {
    for (const listener of this.listeners[name] || []) listener({
      preventDefault() {},
      stopPropagation() {},
      target: this,
      ...event,
    });
  }

  scrollIntoView() { this.scrolledIntoView = true; }

  focus() {
    this.focused = true;
    this.document.activeElement = this;
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }

  removeAttribute(name) {
    delete this.attributes[name];
  }

  getAttribute(name) {
    return this.attributes[name] || null;
  }

  contains(node) {
    return node === this || this.children.some((child) => child.contains(node));
  }

  querySelectorAll(selector) {
    if (selector === '[role="tab"]') return this.children.filter((child) => child.attributes.role === 'tab');
    return [];
  }
}

// Structure-independent lookups over the rendered runs view. Positional walks
// (`children[2]` for the note, `children[3]` for the table) broke this harness
// twice on ordinary render changes -- a column lifted out of the table, a note
// appearing above it -- because they encode the shape rather than ask for the
// thing. Everything below finds a node by what it is: its class, its tag, or
// its column heading.
const kids = (node) => (node && Array.isArray(node.children) ? node.children : []);
const byClass = (node, className) => kids(node).filter((child) => child.classList && child.classList.contains(className));
const oneByClass = (node, className) => byClass(node, className)[0];
const byTag = (node, tag) => kids(node).filter((child) => child.tag === tag);
const oneByTag = (node, tag) => byTag(node, tag)[0];

const runGroups = (host) => byClass(host, 'thinking-runs-group');
const groupTitle = (section) => oneByTag(section, 'h3').textContent;
const groupBody = (section) => oneByClass(section, 'thinking-runs-group-detail');
const groupSummary = (section) => oneByTag(groupBody(section), 'summary').textContent;
const groupExactId = (section) => oneByClass(groupBody(section), 'thinking-runs-group-id').textContent;
const groupNote = (section) => oneByClass(groupBody(section), 'thinking-runs-group-note');
const groupNoteText = (section) => {
  const note = groupNote(section);
  return note ? note.textContent : null;
};
const groupTable = (section) => oneByClass(groupBody(section), 'thinking-runs-table');
const groupControls = (section) => byClass(groupBody(section), 'thinking-runs-control');
const tableColumns = (table) => kids(oneByTag(oneByTag(table, 'thead'), 'tr')).map((cell) => cell.textContent);
const tableRows = (table) => byTag(oneByTag(table, 'tbody'), 'tr');
const columnValues = (table, label) => {
  const index = tableColumns(table).indexOf(label);
  if (index < 0) throw new Error(`the runs table has no ${label} column: ${tableColumns(table).join(', ')}`);
  return tableRows(table).map((row) => row.children[index].textContent);
};
// The run-log control lives in the last cell of every row, which is the one
// column the renderer guarantees is always present and always last.
const runControls = (table) => tableRows(table).map((row) => kids(row.children[row.children.length - 1])[0]);
const runCards = (host) => kids(oneByClass(host, 'thinking-runs-cards'));

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return {promise, resolve, reject};
}

function settle() {
  return new Promise((resolve) => setImmediate(resolve));
}

async function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) throw new Error('manifest directory required');
  let source = fs.readFileSync(path.join(manifestDir, 'assets/thinking/thinking.js'), 'utf8');
  source = source.replace(
    '  init();\n})();',
    `  window.__thinkingRuns = {
    state,
    bind,
    bindThinkingSectionTabs,
    bindThinkingRuns,
    routeThinkingHash,
    parseThinkingHash,
    thinkingRunsHash,
    activateThinkingSectionTab,
    runContextFromRecord,
    renderThinkingRunList,
    loadThinkingRuns,
    loadThinkingRun,
    loadThinkingOutput,
    openThinkingPrompt,
    navigateThinkingRunsDay,
    currentRunsSelectionKey,
  };
})();`,
  );

  const nodes = new Map();
  const documentListeners = {};
  const document = {
    activeElement: null,
    getElementById(id) { return nodes.get(id) || null; },
    createTextNode(text) { const node = new Element(); node.textContent = text; return node; },
    createElement(tag) {
      const node = new Element('', {}, tag);
      node.document = document;
      return node;
    },
    querySelectorAll(selector) {
      if (selector === '#providers [data-view]') return views;
      if (selector === '[data-thinking-section]') return panels;
      return [];
    },
    addEventListener(name, listener) {
      (documentListeners[name] ||= []).push(listener);
    },
    removeEventListener(name, listener) {
      documentListeners[name] = (documentListeners[name] || []).filter((candidate) => candidate !== listener);
    },
    emit(name, event = {}) {
      for (const listener of documentListeners[name] || []) listener({
        preventDefault() {},
        target: document,
        ...event,
      });
    },
  };
  const make = (id, dataset = {}) => {
    const node = new Element(id, dataset);
    node.document = document;
    nodes.set(id, node);
    return node;
  };
  const tablist = make('thinkingSectionTabs');
  const setupTab = make('thinkingSetupTab');
  const runsTab = make('thinkingRunsTab');
  for (const tab of [setupTab, runsTab]) {
    tab.setAttribute('role', 'tab');
    tablist.appendChild(tab);
  }
  const setupPanel = make('thinkingSetupPanel');
  const runsPanel = make('thinkingRunsPanel', {thinkingSection: 'runs'});
  const panels = [runsPanel];
  const views = [
    setupPanel,
    make('thinkingByoSetup', {view: 'byo-setup'}),
    make('thinkingConfidentialSetup', {view: 'confidential-setup'}),
    make('thinkingLocalSetup', {view: 'local-setup'}),
    make('thinkingLaneSwitch', {view: 'lane-switch'}),
  ];
  setupPanel.dataset.view = 'main';
  make('thinkingHeading');
  make('thinkingRunsHeading');
  make('thinkingRunsStatus');
  make('thinkingRunsDate');
  make('thinkingRunsPrevious');
  make('thinkingRunsNext');
  make('thinkingRunsFacet');
  make('thinkingRunsUpdated');
  make('thinkingRunsSummary');
  make('thinkingRunsContent');
  make('thinkingRunsDetail');
  make('thinkingRunsDetailHeading');
  const detailIds = make('thinkingRunsDetailIds');
  detailIds.hidden = true;
  make('thinkingRunsDetailIdsText');
  make('thinkingRunsDetailFacts');
  const noOutput = make('thinkingRunsNoOutput');
  noOutput.hidden = true;
  noOutput.textContent = "this run doesn't have a saved output.";
  make('thinkingRunsPrompt');
  const detailTabs = make('thinkingRunsDetailTabs');
  const logTab = make('thinkingRunsLogTab');
  const outputTab = make('thinkingRunsOutputTab');
  outputTab.hidden = true;
  for (const tab of [logTab, outputTab]) {
    tab.setAttribute('role', 'tab');
    detailTabs.appendChild(tab);
  }
  make('thinkingRunsLogPanel');
  make('thinkingRunsOutputPanel');
  const promptModal = make('thinkingRunsPromptModal');
  promptModal.hidden = true;
  make('thinkingRunsPromptClose');
  make('thinkingRunsPromptContent');

  const requests = [];
  const dayResponses = [];
  const runResponses = [];
  const promptResponses = [];
  const outputResponses = [];
  const updatedResponses = [];
  const hashListeners = [];
  const window = {
    location: {hash: ''},
    history: {
      pushed: [],
      replaced: [],
      pushState(_state, _title, hash) {
        this.pushed.push(hash);
        window.location.hash = hash;
      },
      replaceState(_state, _title, hash) {
        this.replaced.push(hash);
        window.location.hash = hash;
      },
    },
    addEventListener(name, listener) {
      if (name === 'hashchange') hashListeners.push(listener);
    },
    apiJson(url) {
      requests.push(url);
      if (url.startsWith('/app/thinking/api/talents/')) return dayResponses.shift() || Promise.resolve({uses: [], facets: []});
      if (url === '/app/thinking/api/updated-days') return updatedResponses.shift() || Promise.resolve([]);
      if (url.startsWith('/app/thinking/api/run/')) return runResponses.shift() || Promise.resolve({id: 'use-id', name: 'talent', day: '20260815', events: []});
      if (url.startsWith('/app/thinking/api/preview/')) return promptResponses.shift() || Promise.resolve({content: ''});
      if (url.startsWith('/app/thinking/api/output/')) return outputResponses.shift() || Promise.resolve({content: ''});
      throw new Error(`unexpected URL: ${url}`);
    },
    logError() {},
  };
  window.window = window;
  const context = {
    window,
    document,
    console,
    Date,
    Map,
    Set,
    Promise,
    URLSearchParams,
    fetch() { throw new Error('unexpected fetch'); },
    setTimeout,
    requestAnimationFrame: (callback) => callback(),
    clearTimeout,
  };
  vm.runInNewContext(fs.readFileSync(path.join(manifestDir, 'assets/static/date_format.js'), 'utf8'), context);
  vm.runInNewContext(source, context, {filename: 'thinking.js'});
  const format = window.JournalFormat;
  assert.strictEqual(format.segmentTime('125903_60'), '12:59:03');
  assert.strictEqual(format.segmentTime('246099_60'), 'time unavailable');
  assert.strictEqual(format.duration(59.6), '1 min 0 sec');
  assert.strictEqual(format.compactTokens(999500), '1M');
  assert.strictEqual(format.compactTokens(12000), '12K');
  assert.strictEqual(format.timestamp(null), 'time unavailable');
  assert.strictEqual(format.stream('import.chatgpt'), 'import chatgpt');
  const thinking = window.__thinkingRuns;
  assert(thinking, 'test exports present');
  assert.strictEqual(source.includes('window.selectedFacet'), false, 'Thinking does not read the shared selected facet');
  assert.strictEqual(source.includes('facet.switch'), false, 'Thinking does not register the retired facet event');
  thinking.bind();
  thinking.bindThinkingSectionTabs();
  thinking.bindThinkingRuns();

  const heterogeneousRuns = make('heterogeneousRuns');
  thinking.renderThinkingRunList(heterogeneousRuns, [
    {id: 'output-run', name: 'output run', output_file: 'saved.txt'},
    {id: 'no-output-run', name: 'no output run'},
    {id: 'failed-run', name: 'failed run', failed: true},
    {id: 'completed-run', name: 'completed run', failed: false},
  ]);
  const heterogeneousTable = oneByClass(heterogeneousRuns, 'thinking-runs-table');
  assert.strictEqual(tableRows(heterogeneousTable).length, 4, 'heterogeneous run table renders every row');
  runControls(heterogeneousTable).forEach((control) => {
    assert.ok(control.classList.contains('thinking-runs-run-control'), 'every run row exposes an explicit control');
  });
  runCards(heterogeneousRuns).forEach((card) => {
    assert.ok(card.children[card.children.length - 1].classList.contains('thinking-runs-run-control'), 'every run card exposes an explicit control');
  });

  // G2-38: a run whose log can only ever be empty is dimmed before the click.
  const eventCountRuns = make('eventCountRuns');
  thinking.renderThinkingRunList(eventCountRuns, [
    {id: 'silent-run', name: 'silent run', thinking_count: 0, tool_count: 0},
    {id: 'thinking-run', name: 'thinking run', thinking_count: 3, tool_count: 0},
    {id: 'tool-run', name: 'tool run', thinking_count: 0, tool_count: 2},
  ]);
  const eventCountControls = runControls(oneByClass(eventCountRuns, 'thinking-runs-table'));
  assert.ok(eventCountControls[0].classList.contains('thinking-runs-run-control-empty'), 'a run with no thinking events and no tool calls is dimmed');
  // X-06: "recorded" is a never-list verb aimed at the owner's own material.
  assert.strictEqual(eventCountControls[0].title, 'this run left no log', 'the dimmed control explains itself');
  assert.strictEqual(eventCountControls[1].classList.contains('thinking-runs-run-control-empty'), false, 'a run with thinking events is not dimmed');
  assert.strictEqual(eventCountControls[2].classList.contains('thinking-runs-run-control-empty'), false, 'a run with tool calls is not dimmed');
  eventCountControls.forEach((control) => {
    assert.ok(control.classList.contains('thinking-runs-run-control'), 'the base control class survives the empty-run marker');
  });

  const setupHashes = ['#main', '#byo-setup', '#confidential-setup', '#local-setup', '#lane-switch'];
  for (const hash of setupHashes) {
    thinking.state.pendingSwitchTarget = hash === '#lane-switch' ? 'byo' : '';
    window.location.hash = hash;
    thinking.routeThinkingHash('history');
    assert.strictEqual(window.location.hash, hash, `setup hash preserved: ${hash}`);
    assert.strictEqual(setupTab.attributes['aria-selected'], 'true', `setup tab selected: ${hash}`);
  }

  window.location.hash = '';
  document.activeElement = null;
  nodes.get('thinkingHeading').focused = false;
  thinking.routeThinkingHash('reload');
  assert.strictEqual(window.location.hash, '', 'a plain hashless load leaves the address hash-free');
  assert.strictEqual(nodes.get('thinkingHeading').focused, false, 'a plain load does not focus the panel heading');
  assert.strictEqual(document.activeElement, null, 'a plain load moves focus nowhere');

  nodes.get('thinkingRunsHeading').focused = false;
  window.location.hash = '#runs/20260815';
  thinking.routeThinkingHash('reload');
  assert.strictEqual(nodes.get('thinkingRunsHeading').focused, false, 'a plain load of a runs route does not focus the runs heading');

  window.location.hash = '#main';
  thinking.routeThinkingHash('reload');
  assert.strictEqual(window.location.hash, '#main', 'an inbound #main link stays on the setup route token');

  window.location.hash = '#runs';
  thinking.routeThinkingHash('history');
  assert.match(window.location.hash, /^#runs\/\d{8}$/, 'runs root canonicalizes to today');
  assert.strictEqual(runsPanel.hidden, false, 'runs panel shown');
  assert.strictEqual(runsTab.attributes['aria-selected'], 'true', 'runs tab selected');

  for (const hash of [
    '#runs/20260815',
    '#runs/20260815/talent',
    '#runs/20260815/talent/use-id',
    '#runs/run/use-id',
  ]) {
    window.location.hash = hash;
    thinking.routeThinkingHash('history');
    assert.strictEqual(window.location.hash, hash, `well-formed hash remains contextual: ${hash}`);
    assert.strictEqual(runsPanel.hidden, false, `runs panel remains visible: ${hash}`);
  }

  const encoded = thinking.thinkingRunsHash({
    kind: 'runs', day: '20260815', talent: 'talent/with #', useId: 'use/id?#', key: 'encoded',
  });
  window.location.hash = encoded;
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(thinking.parseThinkingHash())),
    {
      kind: 'runs', day: '20260815', talent: 'talent/with #', useId: 'use/id?#',
      facet: '', facetExplicit: false, key: 'runs:20260815:talent/with #:use/id?#',
    },
    'dynamic hash segments round-trip independently',
  );

  window.location.hash = '#runs/not-a-day';
  thinking.routeThinkingHash('history');
  assert.match(window.location.hash, /^#runs\/\d{8}$/, 'invalid runs hash canonicalizes to today');
  assert.strictEqual(nodes.get('thinkingRunsStatus').textContent, "that talent run isn't available.");

  const contextual = thinking.runContextFromRecord(
    {kind: 'run-id', useId: 'old', key: 'run:old'},
    {id: 'actual/id', day: '20260815', name: 'talent/name'},
  );
  assert.strictEqual(contextual.day, '20260815');
  assert.strictEqual(window.location.hash, '#runs/20260815/talent%2Fname/actual%2Fid', 'record provenance wins');

  const mismatchedDay = deferred();
  const correctedDay = deferred();
  dayResponses.push(mismatchedDay.promise, correctedDay.promise);
  thinking.state.runsCache.run.set('run:cached-id', {id: 'cached-id', day: '20260111', name: 'actual-talent', events: []});
  window.location.hash = '#runs/20260110/requested-talent/cached-id';
  thinking.routeThinkingHash('history');
  await settle();
  assert.strictEqual(window.location.hash, '#runs/20260111/actual-talent/cached-id', 'cached record provenance rewrites the hash');
  assert.strictEqual(nodes.get('thinkingRunsDetailHeading').textContent, 'actual-talent', 'cached record renders under its source talent');
  assert.strictEqual(requests.filter((url) => url === '/app/thinking/api/talents/20260111').length, 1, 'cached provenance reloads the corrected day');
  correctedDay.resolve({uses: [{id: 'contextual-day', name: 'actual-talent'}], facets: []});
  await settle();
  assert.strictEqual(nodes.get('thinkingRunsDate').value, '2026-01-11', 'corrected day controls render from cached-record provenance');
  assert.strictEqual(nodes.get('thinkingRunsContent').children[1].children[0].textContent, 'actual-talent', 'corrected day content replaces the mismatched context');
  mismatchedDay.resolve({uses: [{id: 'stale-context', name: 'stale-day'}], facets: []});
  await settle();

  document.activeElement = new Element('outside');
  document.activeElement.document = document;
  window.location.hash = '#main';
  thinking.state.runsLastHash = '';
  thinking.routeThinkingHash('history');
  setupTab.emit('keydown', {key: 'End'});
  assert.match(window.location.hash, /^#runs\/\d{8}$/, 'End activates final tab');
  assert.strictEqual(document.activeElement, runsTab, 'keyboard activation keeps focus on selected tab');
  runsTab.emit('keydown', {key: 'ArrowLeft'});
  assert.strictEqual(window.location.hash, '#main', 'arrow activation enters setup');
  assert.strictEqual(document.activeElement, setupTab, 'arrow activation keeps focus on selected tab');
  thinking.state.runsFacet = 'work';
  thinking.state.runsFacetExplicit = true;
  thinking.state.runsCache.run.set('run:use-id', {
    id: 'use-id', day: '20260310', name: 'talent', events: [],
  });
  window.location.hash = '#runs/20260310/talent/use-id?facet=work';
  thinking.routeThinkingHash('history');
  await settle();
  setupTab.emit('click');
  assert.strictEqual(window.location.hash, '#main', 'pointer activation pushes setup');
  assert.strictEqual(document.activeElement, setupTab, 'pointer activation keeps focus on selected tab');
  runsTab.emit('click');
  await settle();
  assert.strictEqual(window.location.hash, '#runs/20260310/talent/use-id?facet=work', 'setup round-trip restores the prior Runs drill-down');
  assert.strictEqual(thinking.state.runsFacet, 'work', 'setup round-trip retains the explicit facet');
  assert.strictEqual(nodes.get('thinkingRunsFacet').value, 'work', 'setup round-trip restores the facet control');
  setupTab.emit('click');
  assert.strictEqual(window.location.hash, '#main', 'setup remains available after a Runs round-trip');

  setupTab.focus();
  nodes.get('thinkingRunsHeading').focused = false;
  window.location.hash = '#runs/20260815';
  hashListeners.forEach((listener) => listener());
  assert.strictEqual(nodes.get('thinkingRunsHeading').focused, false, 'history keeps tablist focus intact');

  thinking.state.runsFacet = '';
  thinking.state.runsFacetExplicit = false;
  dayResponses.push(Promise.resolve({
    uses: [],
    facets: {work: {title: 'Work'}, verona: {title: 'Verona'}},
  }));
  const firstDayRequest = requests.length;
  window.location.hash = '#runs/20260101';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(requests[firstDayRequest], '/app/thinking/api/talents/20260101', 'first day request has no facet when none is selected');
  assert.strictEqual(nodes.get('thinkingRunsSummary').children[0].textContent, '0 runs', 'day summary includes the run total');
  const facetControl = nodes.get('thinkingRunsFacet');
  assert.strictEqual(
    JSON.stringify(facetControl.children.map((option) => [option.value, option.textContent])),
    JSON.stringify([['', 'all'], ['work', 'Work'], ['verona', 'Verona']]),
    'the native facet object map populates distinct selector values and labels',
  );
  facetControl.value = 'work';
  facetControl.emit('change');
  await settle();
  assert.strictEqual(document.cookie, undefined, 'explicit facet selection no longer writes a selectedFacet cookie');
  assert.strictEqual(window.location.hash, '#runs/20260101?facet=work', 'explicit facet is encoded in the Runs hash');
  assert(requests.includes('/app/thinking/api/talents/20260101?facet=work'), 'explicit facet is sent after selection');

  thinking.state.runsFacet = '';
  thinking.state.runsFacetExplicit = false;
  thinking.routeThinkingHash('reload');
  assert.strictEqual(thinking.state.runsFacet, 'work', 'Runs hash restores the selected facet on reload');
  assert.strictEqual(thinking.state.runsFacetExplicit, true, 'Runs hash restores explicit facet state on reload');
  assert.strictEqual(thinking.currentRunsSelectionKey().includes('cookie'), false, 'Runs selection keys have no cookie sentinel');

  thinking.navigateThinkingRunsDay(1);
  assert.strictEqual(window.location.hash, '#runs/20260102?facet=work', 'next day preserves the facet hash');
  thinking.navigateThinkingRunsDay(-1);
  assert.strictEqual(window.location.hash, '#runs/20260101?facet=work', 'previous day preserves the facet hash');

  const dayFailure = deferred();
  dayResponses.push(dayFailure.promise);
  window.location.hash = '#runs/20260103';
  thinking.routeThinkingHash('history');
  dayFailure.reject(new Error('day failure'));
  await settle();
  await settle();
  assert.strictEqual(nodes.get('thinkingRunsContent').children[0].textContent, "couldn't load talent runs", 'day failure replaces only the Runs body');
  const retry = nodes.get('thinkingRunsContent').children[1];
  retry.emit('click');
  await settle();
  assert(requests.filter((url) => url.startsWith('/app/thinking/api/talents/20260103')).length >= 2, 'retry starts a new day request');

  const runFailure = deferred();
  runResponses.push(runFailure.promise);
  window.location.hash = '#runs/20260103/talent/missing';
  thinking.routeThinkingHash('history');
  runFailure.reject(new Error('run failure'));
  await settle();
  await settle();
  assert.strictEqual(nodes.get('thinkingRunsLogPanel').children[0].textContent, "couldn't load that run", 'run failure remains inside detail context');

  const noOutputDay = deferred();
  dayResponses.push(noOutputDay.promise);
  runResponses.push(Promise.resolve({id: 'without-output', day: '20260112', name: 'talent', events: []}));
  window.location.hash = '#runs/20260112/talent/without-output';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(noOutput.hidden, false, 'completed run without output explains the missing output');
  assert.strictEqual(noOutput.textContent, "this run doesn't have a saved output.");
  noOutputDay.resolve({uses: [], facets: []});
  await settle();
  assert.strictEqual(noOutput.hidden, false, 'late day render preserves the selected no-output state');

  const deepDay = deferred();
  dayResponses.push(deepDay.promise);
  outputResponses.push(Promise.resolve({content: 'saved output'}));
  runResponses.push(Promise.resolve({id: 'with-output', day: '20260109', name: 'talent', output_file: 'saved.txt', events: []}));
  const deepDayRequests = requests.filter((url) => url.startsWith('/app/thinking/api/talents/20260109')).length;
  window.location.hash = '#runs/20260109/talent/with-output';
  thinking.routeThinkingHash('history');
  assert.strictEqual(noOutput.hidden, true, 'a subsequent run load clears an earlier no-output notice');
  await settle();
  assert.strictEqual(
    requests.filter((url) => url.startsWith('/app/thinking/api/talents/20260109')).length,
    deepDayRequests + 1,
    'matching deep run detail starts one day read',
  );
  deepDay.resolve({uses: [], facets: []});
  await settle();
  assert.strictEqual(outputTab.hidden, false, 'run output tab becomes visible after binding');
  assert.strictEqual(noOutput.hidden, true, 'a rendered output hides the no-output notice');
  outputTab.emit('click');
  await settle();
  assert.strictEqual(nodes.get('thinkingRunsOutputPanel').textContent, 'saved output', 'newly visible output tab activates its panel');
  outputTab.emit('keydown', {key: 'ArrowLeft'});
  assert.strictEqual(logTab.attributes['aria-selected'], 'true', 'detail tabs rove after output becomes visible');

  window.location.hash = '#runs/20260113';
  thinking.routeThinkingHash('history');
  assert.strictEqual(thinking.state.runsDetail, null, 'day-only route clears the selected run');
  assert.strictEqual(nodes.get('thinkingRunsDetail').hidden, true, 'day-only route hides prior run detail');
  assert.strictEqual(outputTab.hidden, true, 'day-only route hides prior output tab');
  assert.strictEqual(noOutput.hidden, true, 'day-only route hides prior no-output notice');

  runResponses.push(Promise.resolve({id: 'same-run', day: '20260113', name: 'talent-a', output_file: 'same.txt', events: []}));
  window.location.hash = '#runs/20260113/talent-a/same-run';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  const sameRunRequests = requests.filter((url) => url === '/app/thinking/api/run/same-run').length;
  nodes.get('thinkingRunsOutputPanel').textContent = 'same run output';
  thinking.routeThinkingHash('history');
  await settle();
  assert.strictEqual(thinking.state.runsDetail.id, 'same-run', 'same-run re-entry retains its selected record');
  assert.strictEqual(requests.filter((url) => url === '/app/thinking/api/run/same-run').length, sameRunRequests, 'same-run re-entry uses the run cache without a refetch');
  assert.strictEqual(nodes.get('thinkingRunsOutputPanel').textContent, 'same run output', 'same-run re-entry does not clear selected output state');

  window.location.hash = '#runs/20260113/talent-a';
  thinking.routeThinkingHash('history');
  assert.strictEqual(thinking.state.runsDetail, null, 'talent-only route clears the selected run');
  assert.strictEqual(nodes.get('thinkingRunsDetail').hidden, true, 'talent-only route hides prior run detail');
  assert.strictEqual(outputTab.hidden, true, 'talent-only route hides prior output tab');
  assert.strictEqual(noOutput.hidden, true, 'talent-only route hides prior no-output notice');

  window.location.hash = '#runs/20260113/talent-a/same-run';
  thinking.routeThinkingHash('history');
  await settle();
  nodes.get('thinkingRunsPrompt').focus();
  thinking.openThinkingPrompt();
  assert.strictEqual(promptModal.hidden, false, 'selected run opens its prompt before a selection change');
  const runB = deferred();
  runResponses.push(runB.promise);
  window.location.hash = '#runs/20260114/talent-b/run-b';
  thinking.routeThinkingHash('history');
  assert.strictEqual(promptModal.hidden, true, 'changing runs closes the prior run prompt');
  assert.strictEqual((documentListeners.keydown || []).length, 0, 'changing runs removes the prompt Escape listener');
  assert.strictEqual(thinking.state.runsDetail, null, 'changing runs clears the prior run detail before loading');
  runB.resolve({id: 'run-b', day: '20260114', name: 'talent-b', events: []});
  await settle();
  await settle();
  assert.strictEqual(thinking.state.runsDetail.id, 'run-b', 'new run detail renders after the prior prompt closes');

  const identityRequestsBefore = requests.filter((url) => url === '/app/thinking/api/identity').length;
  window.location.hash = '#identity';
  thinking.routeThinkingHash('history');
  assert.strictEqual(window.location.hash, '#main', '#identity falls through to setup');
  assert.strictEqual(setupPanel.hidden, false, '#identity shows the setup view');
  assert.strictEqual(runsPanel.hidden, true, '#identity does not show the runs panel');
  assert.strictEqual(
    requests.filter((url) => url === '/app/thinking/api/identity').length,
    identityRequestsBefore,
    '#identity does not fetch the deleted identity API',
  );

  runResponses.push(Promise.resolve({reason_code: 'talent_run_pending'}));
  window.location.hash = '#runs/20260103/talent/active';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(nodes.get('thinkingRunsLogPanel').children[0].textContent, 'this run is still in progress.', 'active run renders progress instead of an empty detail');
  assert.strictEqual(nodes.get('thinkingRunsLogPanel').children[1].textContent, 'check back soon.');
  assert.strictEqual(thinking.state.runsCache.run.has('run:active'), false, 'active response is not cached as a completed run');
  assert.strictEqual(thinking.state.runsDetail, null, 'active run clears the prior completed-run selection');
  nodes.get('thinkingRunsPrompt').emit('click');
  assert.strictEqual(promptModal.hidden, true, 'pending run cannot open the prior run prompt');

  const promptButton = nodes.get('thinkingRunsPrompt');
  promptButton.focus();
  thinking.state.runsDetail = {name: 'prompt talent'};
  assert.strictEqual((documentListeners.keydown || []).length, 0, 'closed prompt has no document Escape listener');
  thinking.openThinkingPrompt();
  assert.strictEqual(promptModal.hidden, false, 'prompt modal opens');
  assert.strictEqual((documentListeners.keydown || []).length, 1, 'open prompt installs one Escape listener');
  document.emit('keydown', {key: 'Escape'});
  assert.strictEqual(promptModal.hidden, true, 'Escape closes the prompt modal');
  assert.strictEqual((documentListeners.keydown || []).length, 0, 'closing prompt removes its Escape listener');
  assert.strictEqual(document.activeElement, promptButton, 'closing prompt restores focus to its opener');
  thinking.openThinkingPrompt();
  assert.strictEqual((documentListeners.keydown || []).length, 1, 'reopening prompt installs a fresh Escape listener');
  nodes.get('thinkingRunsPromptClose').emit('click');
  assert.strictEqual((documentListeners.keydown || []).length, 0, 'close button removes the Escape listener');

  runResponses.push(Promise.resolve({id: 'output-before-failure', day: '20260103', name: 'talent', output_file: 'old.txt', events: []}));
  window.location.hash = '#runs/20260103/talent/output-before-failure';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(outputTab.hidden, false, 'completed output run exposes its output tab');
  const runLoadFailure = deferred();
  runResponses.push(runLoadFailure.promise);
  window.location.hash = '#runs/20260103/talent/failing-run';
  thinking.routeThinkingHash('history');
  assert.strictEqual(outputTab.hidden, true, 'subsequent run load hides the prior output tab');
  assert.strictEqual(noOutput.hidden, true, 'subsequent run load hides the prior no-output notice');
  runLoadFailure.reject(new Error('run load failure'));
  await settle();
  await settle();
  assert.strictEqual(nodes.get('thinkingRunsLogPanel').children[0].textContent, "couldn't load that run", 'failed run does not restore prior run details');
  assert.strictEqual(outputTab.hidden, true, 'failed run leaves no prior output tab');
  assert.strictEqual(noOutput.hidden, true, 'failed run leaves no no-output notice');

  const first = deferred();
  const second = deferred();
  dayResponses.push(first.promise, second.promise);
  window.location.hash = '#runs/20260104';
  thinking.routeThinkingHash('history');
  window.location.hash = '#runs/20260105';
  thinking.routeThinkingHash('history');
  second.resolve({uses: [{id: 'new', name: 'current-day'}], facets: []});
  await settle();
  first.resolve({uses: [{id: 'old', name: 'stale-day'}], facets: []});
  await settle();
  await settle();
  assert.strictEqual(thinking.state.runsCache.day.has('day:20260104:facet:'), false, 'stale day response is not cached');
  assert.strictEqual(thinking.state.runsCache.day.has('day:20260105:facet:'), true, 'current day response is cached without a cookie sentinel');
  assert.strictEqual(nodes.get('thinkingRunsContent').children[1].children[0].textContent, 'current-day', 'stale day response does not replace the current render');

  const firstRun = deferred();
  const secondRun = deferred();
  runResponses.push(firstRun.promise, secondRun.promise);
  window.location.hash = '#runs/20260106/talent/first';
  thinking.routeThinkingHash('history');
  window.location.hash = '#runs/20260107/talent/second';
  thinking.routeThinkingHash('history');
  secondRun.resolve({id: 'second', day: '20260107', name: 'current-run', events: []});
  await settle();
  firstRun.resolve({id: 'first', day: '20260106', name: 'stale-run', events: []});
  await settle();
  await settle();
  assert.strictEqual(thinking.state.runsCache.run.has('run:first'), false, 'stale run response is not cached');
  assert.strictEqual(thinking.state.runsCache.run.has('run:second'), true, 'current run response is cached');
  assert.strictEqual(nodes.get('thinkingRunsDetailHeading').textContent, 'current-run', 'stale run response does not replace the current render');

  const firstPrompt = deferred();
  const secondPrompt = deferred();
  promptResponses.push(firstPrompt.promise, secondPrompt.promise);
  window.location.hash = '#runs/20260107/first%20prompt/second';
  thinking.routeThinkingHash('history');
  await settle();
  thinking.state.runsDetail = {name: 'first prompt'};
  thinking.openThinkingPrompt();
  window.location.hash = '#runs/20260107/second%20prompt/second';
  thinking.routeThinkingHash('history');
  await settle();
  thinking.state.runsDetail = {name: 'second prompt'};
  thinking.openThinkingPrompt();
  secondPrompt.resolve({content: 'current prompt'});
  await settle();
  firstPrompt.resolve({content: 'stale prompt'});
  await settle();
  await settle();
  assert.strictEqual(thinking.state.runsCache.prompt.has('prompt:first prompt'), false, 'stale prompt response is not cached');
  assert.strictEqual(thinking.state.runsCache.prompt.get('prompt:second prompt').content, 'current prompt', 'current prompt response is cached');
  assert.strictEqual(nodes.get('thinkingRunsPromptContent').textContent, 'current prompt', 'stale prompt response does not replace the current render');

  const firstOutput = deferred();
  const secondOutput = deferred();
  outputResponses.push(firstOutput.promise, secondOutput.promise);
  window.location.hash = '#runs/20260107/talent/second';
  thinking.routeThinkingHash('history');
  await settle();
  thinking.state.runsDetail = {day: '20260107', output_file: 'first.txt'};
  thinking.loadThinkingOutput();
  window.location.hash = '#runs/20260108/talent/second';
  thinking.routeThinkingHash('history');
  await settle();
  thinking.state.runsDetail = {day: '20260108', output_file: 'second.txt'};
  thinking.loadThinkingOutput();
  secondOutput.resolve({content: 'current output'});
  await settle();
  firstOutput.resolve({content: 'stale output'});
  await settle();
  await settle();
  assert.strictEqual(thinking.state.runsCache.output.has('output:20260107:first.txt'), false, 'stale output response is not cached');
  assert.strictEqual(thinking.state.runsCache.output.get('output:20260108:second.txt').content, 'current output', 'current output response is cached');
  assert.strictEqual(nodes.get('thinkingRunsOutputPanel').textContent, 'current output', 'stale output response does not replace the current render');
  // --- night repairs: grouped + paged runs, readable labels, honest day and log ---
  thinking.state.runsFacet = '';
  thinking.state.runsFacetExplicit = false;
  thinking.state.runsFailuresOnly = false;

  // A whitespace-only model id is truthy in the model column but trims to
  // nothing for the sentence. The note must not lift a column it went on to
  // say nothing about.
  dayResponses.push(Promise.resolve({uses: [
    {id: 'blank-1', name: 'blank', start: 1788662697014, status: 'completed', provider: 'local', model: '   '},
    {id: 'blank-2', name: 'blank', start: 1788662697014, status: 'completed', provider: 'local', model: '   '},
  ], facets: []}));
  window.location.hash = '#runs/20260207';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  const blankModelGroup = runGroups(nodes.get('thinkingRunsContent'))[0];
  assert.strictEqual(
    groupNoteText(blankModelGroup),
    '2 blank runs completed.',
    'a model id that is only whitespace is not named in the note',
  );
  assert.deepStrictEqual(
    tableColumns(groupTable(blankModelGroup)),
    ['ran', 'model', 'provider', 'runtime', 'thinking events', 'tool calls', 'run log'],
    'a column the note did not speak for is never lifted out of the table',
  );
  const bulkRuns = [];
  for (let index = 0; index < 120; index += 1) {
    bulkRuns.push({
      id: `bulk-${index}`, name: 'entities:detection', start: 1788662697014,
      status: 'completed', failed: index === 3,
    });
  }
  bulkRuns.push({id: 'solo', name: 'speaker_attribution', start: 1788662697014, status: 'completed'});
  dayResponses.push(Promise.resolve({uses: bulkRuns, facets: []}));
  window.location.hash = '#runs/20260204';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  const groupSections = runGroups(nodes.get('thinkingRunsContent'));
  assert.strictEqual(groupSections.length, 2, 'a busy day renders one group per talent');
  assert.strictEqual(groupTitle(groupSections[0]), 'entity detection', 'a raw talent id renders as a readable label');
  assert.strictEqual(groupTitle(groupSections[1]), 'speaker attribution', 'an unmapped talent id humanizes');
  const busyGroup = groupSections[0];
  assert.strictEqual(groupSummary(busyGroup), '120 runs \u00b7 1 failed', 'the group summary carries its counts');
  assert.strictEqual(groupBody(busyGroup).open, false, 'a busy day collapses each talent group');
  assert.strictEqual(groupExactId(busyGroup), 'exact id: entities:detection', 'the exact talent id stays in the disclosure');
  // G2-46 / prior partial: one of these 120 runs failed, so no clause is true
  // of every run -- the model and provider are blank on every row, so there is
  // nothing for a note to say. The status column stays because nothing else is
  // carrying it; model and provider go because they are blank, not because the
  // note spoke for them.
  assert.strictEqual(groupNoteText(busyGroup), null, 'a group with nothing true of every run gets no note');
  const busyTable = groupTable(busyGroup);
  assert.deepStrictEqual(
    tableColumns(busyTable),
    ['ran', 'status', 'runtime', 'thinking events', 'tool calls', 'run log'],
    'the run time column says when the run executed and the last column is headed for the control it holds',
  );
  assert.strictEqual(tableRows(busyTable).length, 50, 'a group pages its runs 50 at a time');
  const showMore = groupControls(busyGroup)[groupControls(busyGroup).length - 1];
  assert.strictEqual(showMore.textContent, 'show 50 more runs \u00b7 70 left', 'the group offers the next page');
  showMore.emit('click');
  const pagedGroup = runGroups(nodes.get('thinkingRunsContent'))[0];
  assert.strictEqual(tableRows(groupTable(pagedGroup)).length, 100, 'showing more extends the same group');
  assert.strictEqual(groupBody(pagedGroup).open, true, 'showing more keeps the group open');
  const smallGroup = runGroups(nodes.get('thinkingRunsContent'))[1];
  assert.strictEqual(groupExactId(smallGroup), 'exact id: speaker_attribution', 'the small group keeps its exact id');
  assert.strictEqual(
    groupNoteText(smallGroup),
    '1 speaker attribution run completed.',
    'the group note never names a model the run did not record',
  );
  assert.deepStrictEqual(
    tableColumns(groupTable(smallGroup)),
    ['ran', 'runtime', 'thinking events', 'tool calls', 'run log'],
    'the note lifts out the status column, and the blank model and provider columns are dropped',
  );
  assert.strictEqual(tableRows(groupTable(smallGroup)).length, 1, 'a small group still renders in full');

  // G2-46: a group that is uniform on real values states them once, and only
  // then do those columns leave the table.
  dayResponses.push(Promise.resolve({uses: [
    {id: 'pulse-1', name: 'pulse', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: 20},
    {id: 'pulse-2', name: 'pulse', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: 20},
    {id: 'brief-1', name: 'briefing', start: 1788662697014, status: 'completed', provider: 'local', model: null, runtime_seconds: null},
    {id: 'brief-2', name: 'briefing', start: 1788662697014, status: 'completed', provider: 'local', model: null, runtime_seconds: null},
    {id: 'part-1', name: 'partial', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: 20},
    {id: 'part-2', name: 'partial', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: null},
    {id: 'spread-1', name: 'spread', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: 2},
    {id: 'spread-2', name: 'spread', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: 38},
  ], facets: []}));
  window.location.hash = '#runs/20260205';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  const dayGroups = runGroups(nodes.get('thinkingRunsContent'));
  const uniformGroup = dayGroups[0];
  assert.strictEqual(
    groupNoteText(uniformGroup),
    '2 pulse runs completed on Qwen 3.5 4B (local), 20 sec each.',
    'a uniform group states its status, model and typical runtime in one sentence',
  );
  assert.deepStrictEqual(
    tableColumns(groupTable(uniformGroup)),
    ['ran', 'provider', 'runtime', 'thinking events', 'tool calls', 'run log'],
    'the note lifts out only what it says, so the provider column it never names stays',
  );

  // An unrecorded runtime arrives as null, and Number(null) is 0 — the note
  // used to average that in and report "about 0 sec each" for runs whose own
  // runtime column reads "duration unavailable". With no model recorded the
  // clause names the provider instead, because the note is what lifts the
  // provider column out of the table.
  const nullRuntimeGroup = dayGroups[1];
  assert.strictEqual(
    groupNoteText(nullRuntimeGroup),
    '2 briefing runs completed.',
    'an unknown runtime is left out of the note instead of averaging in as zero',
  );
  assert.deepStrictEqual(
    tableColumns(groupTable(nullRuntimeGroup)),
    ['ran', 'provider', 'runtime', 'thinking events', 'tool calls', 'run log'],
    'with no model to name, the note says nothing about the lane and the provider column stays',
  );
  assert.deepStrictEqual(
    columnValues(groupTable(nullRuntimeGroup), 'runtime'),
    ['duration unavailable', 'duration unavailable'],
    'the runtime column and the group note agree that the runtime is unknown',
  );

  // The average is only true of the runs that recorded a runtime, but "each"
  // distributes it across the whole group — so the clause is stated only when
  // the whole group recorded one.
  const partialRuntimeGroup = dayGroups[2];
  assert.strictEqual(
    groupNoteText(partialRuntimeGroup),
    '2 partial runs completed on Qwen 3.5 4B (local).',
    'a runtime known for only part of the group is not reported as the time each run took',
  );

  // Recorded on every run, but 2 sec and 38 sec average to a figure true of
  // neither — "each" only holds when every run reads the same.
  const spreadRuntimeGroup = dayGroups[3];
  assert.strictEqual(
    groupNoteText(spreadRuntimeGroup),
    '2 spread runs completed on Qwen 3.5 4B (local).',
    'runtimes that disagree are not averaged into a time each run took',
  );
  assert.deepStrictEqual(
    columnValues(groupTable(spreadRuntimeGroup), 'runtime'),
    ['2 sec', '38 sec'],
    'the runtime column still carries each run\'s own time',
  );

  // The count stands in front of a *filtered* set: failed-runs-only narrows
  // the group, and a facet narrows the day. A talent that mostly succeeded
  // must not have its failures described as the whole of it — which is why
  // the sentence carries no "all".
  thinking.state.runsFailuresOnly = true;
  dayResponses.push(Promise.resolve({uses: [
    {id: 'mixed-1', name: 'mixed', start: 1788662697014, status: 'failed', failed: true, provider: 'local', model: 'local/qwen3.5-4b'},
    {id: 'mixed-2', name: 'mixed', start: 1788662697014, status: 'failed', failed: true, provider: 'local', model: 'local/qwen3.5-4b'},
    {id: 'mixed-3', name: 'mixed', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b'},
    {id: 'mixed-4', name: 'mixed', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b'},
    {id: 'mixed-5', name: 'mixed', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b'},
  ], facets: []}));
  window.location.hash = '#runs/20260206';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(
    groupNoteText(runGroups(nodes.get('thinkingRunsContent'))[0]),
    '2 mixed runs failed on Qwen 3.5 4B (local).',
    'the note counts the runs it stands in front of, never calling a filtered view the whole of a talent',
  );
  thinking.state.runsFailuresOnly = false;

  // G2-B12 / X-05: two columns that were saying more than they knew. A run that
  // started and has not finished has no runtime to report yet, which is not the
  // same fact as a runtime the app failed to get; and third-party brands keep
  // their case in the canon, while the local lane is not a brand.
  const inFlightRuns = make('inFlightRuns');
  thinking.renderThinkingRunList(inFlightRuns, [
    {id: 'running-run', name: 'pulse', status: 'running', provider: 'local'},
    {id: 'done-run', name: 'pulse', status: 'completed', provider: 'local', runtime_seconds: 30},
    {id: 'unrecorded-run', name: 'pulse', status: 'completed', provider: 'anthropic'},
    {id: 'failed-in-flight', name: 'pulse', status: 'running', failed: true, provider: 'openai'},
  ]);
  const inFlightTable = oneByClass(inFlightRuns, 'thinking-runs-table');
  assert.deepStrictEqual(
    columnValues(inFlightTable, 'runtime'),
    ['still running', '30 sec', 'duration unavailable', 'duration unavailable'],
    'a run in flight has no runtime yet, and a finished run that recorded none still says so',
  );
  assert.deepStrictEqual(
    columnValues(inFlightTable, 'provider'),
    ['local', 'local', 'Claude', 'GPT'],
    'third-party brands keep their case and the local lane stays lowercase',
  );

  // Prior G2-46 partial + X-04. One run still in flight is enough to make a
  // group's statuses differ, which used to suppress the whole note -- so a
  // group repeating one model down every row kept the column with nothing
  // saying it once. Each clause now earns its place on its own columns.
  // The headings come from the talents the payload describes: an authored
  // title is the owner's name for a talent, an id-shaped title is no title at
  // all, and a title carrying retired vocabulary loses to the app's own name.
  dayResponses.push(Promise.resolve({
    uses: [
      {id: 'flight-1', name: 'pulse', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: 20},
      {id: 'flight-2', name: 'pulse', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: 20},
      {id: 'flight-3', name: 'pulse', start: 1788662697014, status: 'running', provider: 'local', model: 'local/qwen3.5-4b'},
      {id: 'partner-1', name: 'partner', start: 1788662697014, status: 'completed'},
      {id: 'observer-1', name: 'entities:entity_observer', start: 1788662697014, status: 'completed'},
      {id: 'untitled-1', name: 'untitled_talent', start: 1788662697014, status: 'completed'},
    ],
    facets: [{name: 'work', title: 'work life'}],
    talents: {
      pulse: {title: 'Pulse'},
      partner: {title: 'your profile'},
      'entities:entity_observer': {title: 'Entity Observer'},
      untitled_talent: {title: 'untitled_talent'},
    },
  }));
  window.location.hash = '#runs/20260208';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  const titledGroups = runGroups(nodes.get('thinkingRunsContent'));
  assert.strictEqual(
    groupNoteText(titledGroups[0]),
    '3 pulse runs on Qwen 3.5 4B (local).',
    'a group whose statuses differ still says the model every run in it shares',
  );
  assert.deepStrictEqual(
    tableColumns(groupTable(titledGroups[0])),
    ['ran', 'status', 'provider', 'runtime', 'thinking events', 'tool calls', 'run log'],
    'the model column leaves because the note says it, and status stays because the note cannot',
  );
  assert.deepStrictEqual(
    columnValues(groupTable(titledGroups[0]), 'runtime'),
    ['20 sec', '20 sec', 'still running'],
    'the run in flight reads as running beside the runs that finished',
  );
  assert.strictEqual(groupTitle(titledGroups[1]), 'your profile', "a talent's authored title is the owner's name for it");
  assert.strictEqual(groupTitle(titledGroups[2]), 'entity facts', 'a title carrying retired vocabulary never reaches the owner');
  assert.strictEqual(groupTitle(titledGroups[3]), 'untitled talent', 'a title that is only the id is no title, and the id humanizes');

  // G2-B02: the facet filter's empty state used to fall through to "no talent
  // runs on this day" on a day that plainly had them. The server narrows the
  // day payload to the picked facet, so the day's own count is the one carried
  // over from the last unfiltered read.
  dayResponses.push(Promise.resolve({uses: [], facets: [{name: 'work', title: 'work life'}]}));
  nodes.get('thinkingRunsFacet').emit('change', {target: {value: 'work'}});
  await settle();
  await settle();
  const facetHost = nodes.get('thinkingRunsContent');
  assert.deepStrictEqual(
    byTag(facetHost, 'p').map((child) => child.textContent),
    ['no runs in this facet on this day', '6 runs ran on this day, none in work life.'],
    'picking a facet says what the filter did, not that the day had no runs',
  );
  const facetReset = byTag(facetHost, 'button').filter((child) => child.textContent === 'show all facets');
  assert.strictEqual(facetReset.length, 1, 'the facet empty state offers a way back instead of stranding the owner');
  facetReset[0].emit('click');
  await settle();
  await settle();
  assert.strictEqual(thinking.state.runsFacet, '', 'showing all facets clears the picked facet');
  assert.strictEqual(runGroups(nodes.get('thinkingRunsContent')).length, 4, 'clearing the facet brings the day back');

  const now = new Date();
  const todayDay = `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
  dayResponses.push(Promise.resolve({uses: [], facets: []}));
  window.location.hash = `#runs/${todayDay}`;
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(nodes.get('thinkingRunsNext').disabled, true, "next day is disabled once the day is today");
  assert.strictEqual(nodes.get('thinkingRunsDate').max, nodes.get('thinkingRunsDate').value, 'the day picker stops at today');
  thinking.navigateThinkingRunsDay(1);
  assert.strictEqual(window.location.hash, `#runs/${todayDay}`, 'next day never walks past today');
  thinking.navigateThinkingRunsDay(-1);
  assert.strictEqual(nodes.get('thinkingRunsNext').disabled, false, 'next day is available again on an earlier day');

  runResponses.push(Promise.resolve({
    id: 'quiet-run', day: '20260204', name: 'entities:detection', status: 'completed',
    failed: false, provider: 'local', model: 'local/qwen3.5-4b',
    events: [{event: 'tool_start'}, {event: 'thinking'}],
  }));
  window.location.hash = '#runs/20260204/entities%3Adetection/quiet-run';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(
    nodes.get('thinkingRunsLogPanel').children[0].children[0].textContent,
    'no detail was recorded for this step (×2)',
    "a completed run's empty step is not reported as a failure",
  );
  assert.strictEqual(nodes.get('thinkingRunsDetailHeading').textContent, 'entity detection', 'the run detail heading reads a label');
  assert.strictEqual(
    nodes.get('thinkingRunsDetailIdsText').textContent,
    'entities:detection \u00b7 local \u00b7 local/qwen3.5-4b',
    'the exact run ids stay in the disclosure',
  );
  assert.strictEqual(detailIds.hidden, false, 'the exact-ids disclosure is present for a selected run');

  runResponses.push(Promise.resolve({
    id: 'broken-run', day: '20260204', name: 'talent', status: 'failed', failed: true,
    events: [{event: 'tool_start'}],
  }));
  window.location.hash = '#runs/20260204/talent/broken-run';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(
    nodes.get('thinkingRunsLogPanel').children[0].children[0].textContent,
    'tool call did not complete',
    'a failed run keeps the incomplete-step wording',
  );

  console.log(`DOM CASES: ${passedCases}/${executedCases} passed`);
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
