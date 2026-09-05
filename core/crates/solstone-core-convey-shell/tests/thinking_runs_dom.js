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

class Element {
  constructor(id = '', dataset = {}) {
    this.id = id;
    this.dataset = {...dataset};
    this.hidden = false;
    this.tabIndex = 0;
    this.textContent = '';
    this.className = '';
    this.children = [];
    this.listeners = {};
    this.attributes = {};
    this.style = {};
    this.parent = null;
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
    createElement() {
      const node = new Element();
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
  const heterogeneousRows = heterogeneousRuns.children[0].children[1].children;
  assert.strictEqual(heterogeneousRows.length, 4, 'heterogeneous run table renders every row');
  heterogeneousRows.forEach((row) => {
    const control = row.children[row.children.length - 1].children[0];
    assert.strictEqual(control.className, 'thinking-runs-run-control', 'every run row exposes an explicit control');
  });
  heterogeneousRuns.children[1].children.forEach((card) => {
    assert.strictEqual(card.children[card.children.length - 1].className, 'thinking-runs-run-control', 'every run card exposes an explicit control');
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
  thinking.routeThinkingHash('reload');
  assert.strictEqual(window.location.hash, '#main', 'absent hash canonicalizes to main');

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
  console.log(`DOM CASES: ${passedCases}/${executedCases} passed`);
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
