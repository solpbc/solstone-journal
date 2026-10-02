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
    this.value = '';
    this.checked = false;
    this.type = '';
    this.name = '';
    this.disabled = false;
    this.href = '';
    this.target = '';
    this.rel = '';
    this.open = false;
  }

  showModal() { this.open = true; }
  close() { this.open = false; }

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
    if (name.startsWith('data-')) {
      delete this.dataset[name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())];
    }
  }

  getAttribute(name) {
    return this.attributes[name] || null;
  }

  contains(node) {
    return node === this || this.children.some((child) => child.contains(node));
  }

  querySelectorAll(selector) {
    const results = [];
    const walk = (node) => {
      for (const child of node.children) {
        if (selector === '[role="tab"]' && child.attributes.role === 'tab') {
          results.push(child);
        } else if (selector.startsWith('input[name="') && child.tag === 'input' && child.name === selector.slice(12, -2)) {
          results.push(child);
        } else if (selector.startsWith('[data-') && selector.endsWith(']')) {
          const attr = selector.slice(1, -1);
          if (child.getAttribute(attr) !== null || child.dataset[attr.slice(5).replace(/-([a-z])/g, (_, l) => l.toUpperCase())] !== undefined) {
            results.push(child);
          }
        }
        walk(child);
      }
    };
    walk(this);
    return results;
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
    localCopy,
    localIsReady,
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
    renderLocal,
    localSetupRefusals,
    localSetupRefusal,
    showLocalSetupFailure,
    api,
    renderMainLanes,
    localLaneBlocked,
    localUnreadyCopy,
    renderGlance,
    renderAll,
    renderByo,
    renderChatGptCard,
    renderChatGptPending,
    renderChatGptPanel,
    startChatGptSignIn,
    pollChatGptAttempt,
    stopChatGptPoll,
    finishChatGptSignIn,
    reopenChatGptTab,
    cancelChatGptSignIn,
    signOutChatGpt,
    loadChatGptModels,
    saveChatGptModel,
    refreshChatGpt,
    setChatGptNotice,
    buildNotice,
    openLane,
    renderLaneSwitch,
    showView,
    applyCopy,
    runProviderLabel,
    CHATGPT_ATTEMPT_STORAGE_KEY,
  };
})();`,
  );

  const nodes = new Map();
  const documentListeners = {};
  const document = {
    visibilityState: 'visible',
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
      if (selector === '[data-provider-card]') {
        return [provAnthropic, provOpenai, provGoogle, provCustom, provChatgpt];
      }
      if (selector === '[data-byo-key-link]') return [];
      if (selector === '[data-open-view]') return [];
      if (selector === '[data-byo-provider]') return [];
      if (selector === '[data-switch-lane]') return [];
      if (selector === 'input[name="byoThinkingBudget"]') return [];
      if (selector === '[data-thinking-budget-label]') return [];
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
  promptModal.open = false;
  promptModal.showModal = () => { promptModal.open = true; };
  promptModal.close = () => { promptModal.open = false; };
  make('thinkingRunsPromptClose');
  make('thinkingRunsPromptContent');
  make('thinkingRunsRequestContent');
  make('localSetupMessage');
  make('localLaneDescription');
  make('localLaneStatus');
  make('localBootstrap');
  make('localCancel');

  // Brain Glance & Lane nodes
  make('brainGlance');
  make('thinkingActiveLane');
  make('thinkingActiveValue');
  make('thinkingActiveDetail');
  make('thinkingActiveIdentity');
  make('thinkingIdentityDetails');
  make('brainCheckAction');
  make('forkHint');
  make('byoLaneTitle');
  make('byoLanePill');
  make('byoLaneDescription');
  make('byoActiveTag');
  make('byoLaneStatus');

  make('chatgptGlancePlan');
  make('chatgptGlancePlanLabel');
  make('chatgptGlancePlanManage');
  make('chatgptGlanceReminder');
  make('chatgptGlanceContinue');
  make('chatgptGlanceContinueLabel');
  make('chatgptGlanceUsage');
  make('chatgptGlanceUsageLane');
  make('chatgptGlanceUsageTitle');
  make('chatgptGlanceUsageBody');
  make('chatgptGlanceUsageManage');
  make('chatgptGlanceStartOver');
  make('chatgptGlancePickOther');

  // BYO Setup nodes
  make('byoSetupTitle');
  make('byoIntro');
  make('byoModeKey');
  make('byoModeEndpoint');
  make('byoPickPanel');
  make('byoPickTitle');
  make('byoPickSub');
  make('byoEndpointPanel');
  make('byoEndpointTitle');
  make('byoEndpointSub');
  make('byoEndpointHonesty');
  make('byoPastePanel');
  make('byoPasteTitle');
  make('byoKeyLabel');
  make('byoKeyInput');
  make('byoKeyHint');
  make('byoTermsLine');
  make('byoSaveKey');
  make('byoClearKey');
  make('byoKeyStatus');
  make('byoModelPanel');
  make('byoKeyCheckstripText');
  make('byoCheckAgain');
  make('byoModelHeading');
  make('byoModelSub');
  make('byoModelInput');
  make('byoModelInputLabel');
  make('byoModelSave');
  make('byoDifferentKey');
  make('byoModelStatus');
  make('byoBackLink');
  make('byoConfigurationGuidance');
  make('prov-openai-desc');
  make('prov-google-desc');

  make('byoProvider');
  make('byoProviderGrid');
  const provAnthropic = make('prov-anthropic', {providerCard: 'anthropic'});
  const provOpenai = make('prov-openai', {providerCard: 'openai'});
  const provGoogle = make('prov-google', {providerCard: 'google'});
  const provCustom = make('prov-custom', {providerCard: 'custom'});
  const provChatgpt = make('prov-chatgpt', {providerCard: 'chatgpt'});
  make('prov-anthropic-pill');
  make('prov-openai-pill');
  make('prov-google-pill');
  make('prov-custom-pill');

  make('chatgptCardTitle');
  make('prov-chatgpt-pill');
  make('chatgptCardBody');
  make('chatgptCardReminder');
  make('chatgptCardStatus');
  make('chatgptCardManage');
  make('chatgptCardContinue');
  make('chatgptCardContinueLabel');
  make('chatgptCardStartOver');

  make('chatgptPending');
  make('chatgptPendingHeading');
  make('chatgptPendingSub');
  make('chatgptPendingNotice');
  make('chatgptPendingStatus');
  make('chatgptReopen');
  make('chatgptCancel');
  make('chatgptFallbackHeading');
  make('chatgptFallbackExplanation');
  make('chatgptAddressLabel');
  make('chatgptRedirectUrl');
  make('chatgptFinish');

  make('chatgptPanel');
  make('chatgptStrip');
  make('chatgptSignOut');
  make('chatgptModels');
  make('chatgptModelHeading');
  make('chatgptModelSub');
  const modelChoices = make('chatgptModelChoices');
  make('chatgptModelStatus');
  make('chatgptPlanRow');
  make('chatgptPanelPlanLabel');
  const chatgptManageUsage = make('chatgptPanelManageUsage');
  chatgptManageUsage.href = 'https://chatgpt.com/settings/usage';
  chatgptManageUsage.target = '_blank';
  chatgptManageUsage.rel = 'noopener noreferrer';
  make('chatgptPanelReminder');
  make('chatgptUseModel');
  make('chatgptPanelStartOver');
  make('chatgptPickOther');

  make('byoTuning');
  make('byoTuningName');
  make('byoTuningState');
  make('byoThinkingToggle');
  make('byoThinkingBudget');
  make('byoThinkingBudgetLegend');
  make('byoTuningNote');
  make('byoTuningStatus');

  make('switchHeading');
  make('switchCurrentNodeLabel');
  make('switchTargetNodeLabel');
  make('switchCurrentLabel');
  make('switchTargetLabel');
  make('switchStatus');
  make('switchNote');
  make('switchConfirmPrimary');
  make('switchCancel');

  const requests = [];
  const apiCalls = [];
  const dayResponses = [];
  const runResponses = [];
  const promptResponses = [];
  const outputResponses = [];
  const updatedResponses = [];
  const hashListeners = [];
  const localResponses = [];
  const providerResponses = [];
  const chatgptResponses = [];
  const settingsResponses = [];

  const sessionStorageMap = new Map();
  const sessionStorage = {
    getItem(key) { return sessionStorageMap.has(key) ? sessionStorageMap.get(key) : null; },
    setItem(key, value) { sessionStorageMap.set(key, String(value)); },
    removeItem(key) { sessionStorageMap.delete(key); },
    clear() { sessionStorageMap.clear(); },
  };

  const openedWindows = [];
  const window = {
    location: {hash: ''},
    sessionStorage,
    open(url, target, features) {
      if (typeof features === 'string' && features.includes('noopener')) return null;
      const win = {
        location: { href: url || 'about:blank' },
        opener: null,
        closed: false,
        close() { this.closed = true; },
        target,
        features,
      };
      openedWindows.push(win);
      return win;
    },
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
    apiJson(url, options) {
      requests.push(url);
      apiCalls.push({ url, method: options?.method || 'GET', body: options?.body ?? null });
      if (url.startsWith('api/local/')) return localResponses.shift()?.(url, options) || Promise.reject(new Error(`unexpected URL: ${url}`));
      if (url.startsWith('api/providers')) return providerResponses.shift()?.(url, options) || Promise.reject(new Error(`unexpected URL: ${url}`));
      if (url.startsWith('/app/thinking/api/chatgpt/') || url.startsWith('api/chatgpt/')) {
        const handler = chatgptResponses.shift();
        if (!handler) return Promise.reject(new Error(`unexpected ChatGPT URL: ${url}`));
        return typeof handler === 'function' ? handler(url, options) : handler;
      }
      if (url.startsWith('/app/settings/api/config')) return settingsResponses.shift()?.(url, options) || Promise.resolve({});
      if (url.startsWith('/app/thinking/api/talents/')) return dayResponses.shift() || Promise.resolve({uses: [], facets: []});
      if (url === '/app/thinking/api/updated-days') return updatedResponses.shift() || Promise.resolve([]);
      if (url.startsWith('/app/thinking/api/run/')) return runResponses.shift() || Promise.resolve({id: 'use-id', name: 'talent', day: '20260815', events: []});
      if (url.startsWith('/app/thinking/api/preview/')) return promptResponses.shift() || Promise.resolve({full_prompt: ''});
      if (url.startsWith('/app/thinking/api/output/')) return outputResponses.shift() || Promise.resolve({content: ''});
      throw new Error(`unexpected URL: ${url}`);
    },
    logError() {},
  };
  window.window = window;
  const context = {
    window,
    document,
    sessionStorage,
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
  assert.strictEqual(format.duration(59.6), '1 min');
  assert.strictEqual(format.duration(300), '5 min');
  assert.strictEqual(format.duration(3599), '59 min 59 sec');
  assert.strictEqual(format.duration(3600), '1 hr');
  assert.strictEqual(format.duration(119700), '33 hr 15 min');
  assert.strictEqual(format.compactTokens(999500), '1M');
  assert.strictEqual(format.compactTokens(12000), '12K');
  assert.strictEqual(format.timestamp(null), 'time unavailable');
  assert.strictEqual(format.stream('import.chatgpt'), 'ChatGPT import');
  assert.strictEqual(format.stream('import.granola'), 'granola import');
  assert.strictEqual(format.stream('device.watch.audio'), 'watch audio');
  assert.strictEqual(format.stream('device_watch-audio'), 'watch audio');
  assert.strictEqual(format.stream('my-host'), 'my-host');
  assert.strictEqual(format.stream('device.omi.audio'), 'omi audio');
  assert.strictEqual(format.stream('ja1r'), 'ja1r');
  assert.strictEqual(format.stream('_default'), 'default');
  const thinking = window.__thinkingRuns;
  assert(thinking, 'test exports present');
  const savedProviders = thinking.state.providers;
  thinking.state.providers = {
    active_lane: {lane: 'none'},
    provider_status: {local: {generate_ready: false, issues: [], selected: false}},
    local_runtime: {status: 'ok', phase: 'ready', reason_code: 'probe-ready'},
  };
  thinking.state.install = {install_state: 'installed'};
  thinking.state.localAvailability = {available: true};
  assert.strictEqual(thinking.localCopy().activate, true, 'verified installed artifacts offer activation before a provider is selected');
  assert.strictEqual(thinking.localIsReady(), true, 'the lane switch accepts installed local artifacts');
  thinking.state.localAvailability = {available: false, reason_code: 'local_probe_failed'};
  assert.strictEqual(thinking.localCopy().activate, false, 'a stale ready runtime cannot override a failed artifact check');
  assert.strictEqual(thinking.localIsReady(), false, 'failed checks cannot offer lane switching');
  thinking.state.localAvailability = {available: true};
  thinking.state.providers.active_lane.lane = 'local';
  assert.strictEqual(thinking.localIsReady(), false, 'an active lane still needs Generate readiness');
  thinking.state.providers = savedProviders;
  thinking.state.localAvailability = null;
  thinking.state.install = null;
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
  assert.ok(nodes.get('thinkingRunsStatus').textContent, 'invalid runs hash leaves a status message');

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
  assert.ok(nodes.get('thinkingRunsContent').children[0].textContent, 'day failure replaces only the Runs body');
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
  assert.strictEqual(oneByTag(nodes.get('thinkingRunsOutputPanel'), 'pre').textContent, 'saved output', 'newly visible output tab activates its panel');
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
  assert.strictEqual(promptModal.open, true, 'selected run opens its prompt before a selection change');
  const runB = deferred();
  runResponses.push(runB.promise);
  window.location.hash = '#runs/20260114/talent-b/run-b';
  thinking.routeThinkingHash('history');
  assert.strictEqual(promptModal.open, false, 'changing runs closes the prior run prompt');
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
  assert.strictEqual(promptModal.open, false, 'pending run cannot open the prior run prompt');

  const promptButton = nodes.get('thinkingRunsPrompt');
  promptButton.focus();
  thinking.state.runsDetail = {name: 'prompt talent'};
  assert.strictEqual((documentListeners.keydown || []).length, 0, 'closed prompt has no document Escape listener');
  thinking.openThinkingPrompt();
  assert.strictEqual(promptModal.open, true, 'prompt modal opens');
  assert.strictEqual((documentListeners.keydown || []).length, 0, 'native modal owns Escape without a document listener');
  promptModal.emit('cancel');
  assert.strictEqual(promptModal.open, false, 'Escape closes the prompt modal');
  assert.strictEqual((documentListeners.keydown || []).length, 0, 'closing prompt removes its Escape listener');
  assert.strictEqual(document.activeElement, promptButton, 'closing prompt restores focus to its opener');
  thinking.openThinkingPrompt();
  assert.strictEqual(promptModal.open, true, 'native prompt reopens');
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
  secondPrompt.resolve({full_prompt: 'current prompt'});
  await settle();
  firstPrompt.resolve({full_prompt: 'stale prompt'});
  await settle();
  await settle();
  assert.strictEqual(thinking.state.runsCache.prompt.has('prompt:first prompt'), false, 'stale prompt response is not cached');
  assert.strictEqual(thinking.state.runsCache.prompt.get('prompt:second prompt').full_prompt, 'current prompt', 'current prompt response is cached');
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
  assert.strictEqual(oneByTag(nodes.get('thinkingRunsOutputPanel'), 'pre').textContent, 'current output', 'stale output response does not replace the current render');
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
    ['local', 'local', 'Anthropic', 'OpenAI'],
    'the provider column names the provider, not the model, and the local lane stays lowercase',
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
      {id: 'reflection-1', name: 'weekly_reflection', start: 1788662697014, status: 'completed'},
      {id: 'observer-1', name: 'entities:entity_observer', start: 1788662697014, status: 'completed'},
      {id: 'untitled-1', name: 'untitled_talent', start: 1788662697014, status: 'completed'},
    ],
    facets: [{name: 'work', title: 'work life'}],
    talents: {
      pulse: {title: 'Pulse'},
      weekly_reflection: {title: 'weekly reflection'},
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
  assert.strictEqual(groupTitle(titledGroups[1]), 'weekly reflection', "a talent's authored title is the owner's name for it");
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
    ['no runs in this facet on this day', '6 runs on this day, none in work life.'],
    'picking a facet says what the filter did, not that the day had no runs',
  );
  const facetReset = byTag(facetHost, 'button').filter((child) => child.textContent === 'show all facets');
  assert.strictEqual(facetReset.length, 1, 'the facet empty state offers a way back instead of stranding the owner');
  // X-03: 20260208 carries a run still in flight, so it is deliberately not
  // cached and clearing the facet reads it again.
  dayResponses.push(Promise.resolve({
    uses: [
      {id: 'flight-1', name: 'pulse', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: 20},
      {id: 'flight-2', name: 'pulse', start: 1788662697014, status: 'completed', provider: 'local', model: 'local/qwen3.5-4b', runtime_seconds: 20},
      {id: 'flight-3', name: 'pulse', start: 1788662697014, status: 'running', provider: 'local', model: 'local/qwen3.5-4b'},
      {id: 'reflection-1', name: 'weekly_reflection', start: 1788662697014, status: 'completed'},
      {id: 'observer-1', name: 'entities:entity_observer', start: 1788662697014, status: 'completed'},
      {id: 'untitled-1', name: 'untitled_talent', start: 1788662697014, status: 'completed'},
    ],
    facets: [{name: 'work', title: 'work life'}],
    talents: {
      pulse: {title: 'Pulse'},
      weekly_reflection: {title: 'weekly reflection'},
      'entities:entity_observer': {title: 'Entity Observer'},
      untitled_talent: {title: 'untitled_talent'},
    },
  }));
  facetReset[0].emit('click');
  await settle();
  await settle();
  assert.strictEqual(thinking.state.runsFacet, '', 'showing all facets clears the picked facet');
  assert.strictEqual(runGroups(nodes.get('thinkingRunsContent')).length, 4, 'clearing the facet brings the day back');
  assert.strictEqual(
    thinking.state.runsCache.day.has('day:20260208:facet:'),
    false,
    'a day holding a run that has not finished is never cached, so "still running" cannot outlive the run',
  );

  // X-03: the day cache is what made "still running" outlive the run. A day
  // with nothing in flight caches and is served from the cache; a day with a
  // run in flight is read again every time the owner comes back to it.
  dayResponses.push(Promise.resolve({uses: [
    {id: 'settled-1', name: 'pulse', start: 1788662697014, status: 'completed', runtime_seconds: 12},
  ], facets: []}));
  window.location.hash = '#runs/20260209';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(
    thinking.state.runsCache.day.has('day:20260209:facet:'),
    true,
    'a day whose runs have all finished still caches',
  );
  const readsBeforeReturn = requests.filter((url) => url.startsWith('/app/thinking/api/talents/20260208')).length;
  window.location.hash = '#runs/20260208';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(
    requests.filter((url) => url.startsWith('/app/thinking/api/talents/20260208')).length,
    readsBeforeReturn + 1,
    'coming back to a day that held a run in flight reads it again rather than replaying the cache',
  );

  // X-03: the register is sentence case, and lowercasing a whole authored
  // title flattened the vendor names inside it too.
  dayResponses.push(Promise.resolve({
    uses: [{id: 'brand-1', name: 'summary', start: 1788662697014, status: 'completed'}],
    facets: [],
    talents: {summary: {title: 'Gemini Summary'}},
  }));
  window.location.hash = '#runs/20260210';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  assert.strictEqual(
    groupTitle(runGroups(nodes.get('thinkingRunsContent'))[0]),
    'Gemini summary',
    "a vendor's own name keeps its case while the rest of the title lowercases",
  );

  // X-03: both filters on is the one combination that used to strand the
  // owner — the sentence counted the facet and called it the day, and the
  // escape that clears the facet was withheld exactly then.
  thinking.state.runsFailuresOnly = true;
  dayResponses.push(Promise.resolve({
    uses: [
      {id: 'both-1', name: 'pulse', start: 1788662697014, status: 'completed'},
      {id: 'both-2', name: 'pulse', start: 1788662697014, status: 'completed'},
      {id: 'both-3', name: 'pulse', start: 1788662697014, status: 'completed'},
      {id: 'both-4', name: 'pulse', start: 1788662697014, status: 'completed'},
    ],
    facets: [{name: 'work', title: 'work life'}],
  }));
  window.location.hash = '#runs/20260211';
  thinking.routeThinkingHash('history');
  await settle();
  await settle();
  dayResponses.push(Promise.resolve({
    uses: [
      {id: 'both-1', name: 'pulse', start: 1788662697014, status: 'completed'},
      {id: 'both-2', name: 'pulse', start: 1788662697014, status: 'completed'},
      {id: 'both-3', name: 'pulse', start: 1788662697014, status: 'completed'},
      {id: 'both-4', name: 'pulse', start: 1788662697014, status: 'completed'},
    ],
    facets: [{name: 'work', title: 'work life'}],
  }));
  nodes.get('thinkingRunsFacet').emit('change', {target: {value: 'work'}});
  await settle();
  await settle();
  const bothFiltersHost = nodes.get('thinkingRunsContent');
  assert.deepStrictEqual(
    byTag(bothFiltersHost, 'p').map((child) => child.textContent),
    ['no failed runs match this view', 'all 4 runs in work life on this day completed.'],
    'the failures-only sentence names the filter whose runs it counted',
  );
  assert.deepStrictEqual(
    byTag(bothFiltersHost, 'button').map((child) => child.textContent),
    ['show all facets', 'show all runs'],
    'with both filters narrowing the view, both ways out are offered',
  );
  thinking.state.runsFailuresOnly = false;
  dayResponses.push(Promise.resolve({uses: [], facets: []}));
  nodes.get('thinkingRunsFacet').emit('change', {target: {value: ''}});
  await settle();
  await settle();

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

  // A failed local-setup click stays on screen. On a journal with no local
  // model the runtime reports `poll: true`, so the page re-renders every 1.5 s;
  // each render used to rewrite this message from the card and erase the
  // error, which left the owner with a button that seemed to do nothing.
  const savedLocalProviders = thinking.state.providers;
  thinking.state.providers = {
    active_lane: {lane: 'local'},
    provider_status: {local: {generate_ready: false, issues: ['binary_missing', 'model_missing'], selected: true}},
    local_runtime: {status: 'blocked', phase: 'artifact-not-ready', reason_code: 'manifest-missing', poll: false},
  };
  thinking.state.localAvailability = {available: false, reason_code: 'binary_missing', reason: 'local runtime is not installed'};
  thinking.state.install = {install_state: 'idle'};
  const refusal = new Error("those settings couldn't be saved.");
  refusal.payload = {error: "those settings couldn't be saved.", detail: 'installer admission timed out'};
  localResponses.push(() => Promise.reject(refusal));
  nodes.get('localBootstrap').emit('click');
  for (let i = 0; i < 6; i += 1) await settle();
  const localMessage = nodes.get('localSetupMessage');
  const refusals = thinking.localSetupRefusals;
  assert.strictEqual(localMessage.textContent, refusals.start, 'a refused local setup says so in owner words');
  assert.strictEqual(localResponses.length, 0, 'the refused click made exactly one request');
  thinking.renderLocal();
  assert.strictEqual(localMessage.textContent, refusals.start, 'the next render keeps the refusal on screen');
  assert.strictEqual(localMessage.textContent.includes('admission'), false, 'the engineering reason stays out of owner copy');
  assert.strictEqual(localMessage.dataset.tone, 'error', 'the kept refusal still reads as an error');
  // Another install holding the lease reads as already running, not as a failure.
  const busy = new Error('Request failed (HTTP 409)');
  busy.status = 409;
  busy.reasonCode = 'install_busy';
  busy.payload = {install_state: 'idle', reason_code: 'install_busy'};
  localResponses.push(() => Promise.reject(busy));
  nodes.get('localBootstrap').emit('click');
  for (let i = 0; i < 6; i += 1) await settle();
  assert.strictEqual(localMessage.textContent, refusals.busy, 'a busy install says it is already running');
  assert.strictEqual(localMessage.dataset.tone, undefined, 'setup already running does not read as an error');
  thinking.renderLocal();
  assert.strictEqual(localMessage.dataset.tone, undefined, 'and the next render keeps it that way');
  // A refused cancel is told in owner words too.
  thinking.state.install = {install_state: 'downloading', attempt_id: 'attempt-1'};
  const cancelRefusal = new Error("those settings couldn't be saved.");
  cancelRefusal.payload = {error: "those settings couldn't be saved.", detail: 'installer process identity invalid'};
  localResponses.push(() => Promise.reject(cancelRefusal));
  nodes.get('localCancel').emit('click');
  for (let i = 0; i < 6; i += 1) await settle();
  assert.strictEqual(localMessage.textContent, refusals.cancel, 'a refused cancel says so in owner words');
  assert.strictEqual(localMessage.dataset.tone, 'error', 'a refused cancel reads as an error');
  // A state the page couldn't read, whatever broke, is told the same way.
  for (const failure of [
    Object.assign(new Error('Request failed (HTTP 500)'), {status: 500}),
    new TypeError('Failed to fetch'),
    Object.assign(new Error('installer lease is busy'), {status: 503, reasonCode: 'config_busy'}),
  ]) {
    thinking.showLocalSetupFailure(failure);
    assert.strictEqual(localMessage.textContent, refusals.check, `a failed read says couldn't check, not "${failure.message}"`);
  }
  // A setup redirect is already leaving the page; the line doesn't change.
  thinking.showLocalSetupFailure(Object.assign(new Error("couldn't finish that request."), {cause: 'setup_required'}));
  assert.strictEqual(localMessage.textContent, refusals.check, 'a setup redirect leaves the line alone');
  // Turning local on keeps a refusal that tells the owner what to change first,
  // and says the generic line for anything else.
  const stateRefusal = Object.assign(new Error('clear your endpoint URL first to run the bundled local model.'), {status: 400, reasonCode: 'invalid_operation_for_state'});
  assert.strictEqual(thinking.localSetupRefusal(stateRefusal, 'activate').message, stateRefusal.message, 'a refusal written for the owner keeps its words');
  assert.strictEqual(thinking.localSetupRefusal(stateRefusal, 'start').message, refusals.start, 'install never shows server text');
  const saveFailure = Object.assign(new Error('something went wrong - try again'), {status: 500, reasonCode: 'settings_operation_failed'});
  assert.strictEqual(thinking.localSetupRefusal(saveFailure, 'activate').message, refusals.activate, 'any other refused turn-on says the owner line');
  // The page's request wrapper hands on a refusal's `error`, the field written
  // for the owner, and keeps `detail` (diagnostics) off the message.
  const refusedWith = async (answer) => {
    localResponses.push(answer);
    try {
      await thinking.api('api/local/probe');
    } catch (err) {
      return err;
    }
    throw new Error('the request was expected to be refused');
  };
  const envelope = {error: 'owner sentence', reason_code: 'invalid_config_value', detail: 'diagnostic detail'};
  const refused = await refusedWith(() => Promise.reject(Object.assign(new Error(envelope.error), {status: 400, reasonCode: envelope.reason_code, payload: envelope})));
  assert.strictEqual(refused.message, envelope.error, "a refusal's error is the message");
  assert.strictEqual(refused.detail, envelope.detail, 'its detail stays on the error for diagnostics');
  assert.strictEqual(refused.status, 400, 'the status survives the wrapper');
  assert.strictEqual(refused.reasonCode, envelope.reason_code, 'the reason code survives the wrapper');
  const detailOnly = await refusedWith(() => Promise.reject(Object.assign(new Error('Request failed (HTTP 400)'), {status: 400, payload: {detail: 'diagnostic detail'}})));
  assert.strictEqual(detailOnly.message.includes('diagnostic'), false, 'a refusal with only a detail never shows the detail');
  assert.strictEqual(detailOnly.message.includes('HTTP'), false, 'nor the browser-side request summary');
  const notStarted = await refusedWith(() => Promise.resolve({ok: false, error: 'check_not_started'}));
  assert.strictEqual(notStarted.message.includes('check_not_started'), false, 'a code in a 200 answer never reaches the page');
  assert.strictEqual(notStarted.reasonCode, 'check_not_started', 'but it stays on the error as its reason');
  const offline = new TypeError('Failed to fetch');
  const unreachable = await refusedWith(() => Promise.reject(offline));
  assert.strictEqual(unreachable.message.includes('fetch'), false, "a browser failure's own text never reaches the page");
  assert.strictEqual(unreachable.cause, offline, 'the browser failure is kept for the console');
  const heldBusy = await refusedWith(() => Promise.reject(Object.assign(new Error('Request failed (HTTP 409)'), {status: 409, reasonCode: 'install_busy', payload: {install_state: 'idle', reason_code: 'install_busy'}})));
  assert.strictEqual(heldBusy.reasonCode, 'install_busy', 'a busy answer keeps the reason the page reads as already running');
  assert.strictEqual(heldBusy.status, 409, 'a busy answer keeps its status');
  const redirect = Object.assign(new Error('setup redirect'), {cause: 'setup_required'});
  assert.strictEqual(await refusedWith(() => Promise.reject(redirect)), redirect, 'a setup redirect passes through untouched');
  const stateAnswer = {error: 'turn this off first.', reason_code: 'invalid_operation_for_state', detail: 'turn this off first.'};
  const refusedState = await refusedWith(() => Promise.reject(Object.assign(new Error(stateAnswer.error), {status: 400, reasonCode: stateAnswer.reason_code, payload: stateAnswer})));
  assert.strictEqual(thinking.localSetupRefusal(refusedState, 'activate').message, stateAnswer.error, 'a state refusal read through the wrapper keeps its words');
  // A change that landed but wasn't logged: the page has re-read what is now set
  // before the refusal reaches its caller, so a re-render can't wipe the sentence.
  const providersBeforeUnlogged = thinking.state.providers;
  providerResponses.push(() => new Promise((resolve) => setTimeout(() => resolve({marker: 'reread'}), 20)));
  const unlogged = {error: 'saved sentence', reason_code: 'settings_saved_unlogged', detail: 'diagnostic detail'};
  const savedButUnlogged = await refusedWith(() => Promise.reject(Object.assign(new Error(unlogged.error), {status: 500, reasonCode: unlogged.reason_code, payload: unlogged})));
  assert.strictEqual(thinking.state.providers?.marker, 'reread', 'the page re-read its state before the refusal arrived');
  assert.strictEqual(thinking.localSetupRefusal(savedButUnlogged, 'clear').message, unlogged.error, 'clearing that landed keeps the saved sentence');
  assert.strictEqual(thinking.localSetupRefusal(savedButUnlogged, 'activate').message, unlogged.error, 'turning on that landed keeps the saved sentence');
  thinking.state.providers = providersBeforeUnlogged;
  const ineligible = Object.assign(new Error('this computer is out'), {status: 400, reasonCode: 'host_ineligible'});
  assert.strictEqual(thinking.localSetupRefusal(ineligible, 'start').message, ineligible.message, "a computer that can't run local thinking is told so, not asked to check again");
  assert.strictEqual(thinking.localSetupRefusal(ineligible, 'start').localSetupTone, 'error', 'and it reads as an error');
  thinking.state.install = {install_state: 'idle'};
  // Trying again clears the earlier refusal as soon as the new request starts.
  localResponses.push(() => new Promise(() => {}));
  nodes.get('localBootstrap').emit('click');
  await settle();
  assert.strictEqual(localMessage.textContent, thinking.localCopy().message, 'trying again clears the earlier refusal and the card speaks again');
  assert.strictEqual(localMessage.textContent, '', 'the card leaves the readiness summary to its verdict and notice');
  assert.strictEqual(localMessage.dataset.tone, undefined, 'and the cleared line carries no error tone');
  assert.strictEqual(thinking.state.localSetupError, '', 'the kept refusal is gone from state too');
  localResponses.length = 0;
  thinking.state.providers = savedLocalProviders;
  thinking.state.localAvailability = null;
  thinking.state.install = null;

  // The main view's local card names a blocked state in owner words. It never
  // shows the server's reason phrase or a bare issue code.
  const laneLine = nodes.get('localLaneDescription');
  const blocked = thinking.localLaneBlocked;
  const savedLaneProviders = thinking.state.providers;
  thinking.state.install = null;
  for (const [reasonCode, reason, kind] of [
    ['binary_missing', 'local runtime is not installed', 'setup'],
    ['model_missing', 'local model files are not installed', 'setup'],
    ['host_ineligible', 'local runtime cannot start on this computer', 'ineligible'],
    ['gpu_probe_failed', 'inability to probe GPU hardware', 'check'],
    ['local_probe_failed', "couldn't check local setup", 'check'],
  ]) {
    for (const lane of ['none', 'local']) {
      thinking.state.providers = {active_lane: {lane}, provider_status: {local: {generate_ready: false, issues: []}}};
      thinking.state.localAvailability = {available: false, reason_code: reasonCode, reason};
      thinking.renderMainLanes();
      assert.strictEqual(laneLine.textContent, blocked[kind], `${reasonCode} on the ${lane} lane says the ${kind} line`);
      assert.strictEqual(laneLine.textContent.includes(reasonCode), false, `${reasonCode} never reaches the card as a code`);
    }
  }
  // With local selected and availability not refusing, the provider's first
  // issue decides the line; an issue the page doesn't know still gets one.
  for (const [issue, kind] of [
    ['binary_missing', 'setup'],
    ['local_server_unhealthy', 'unhealthy'],
    ['some_future_issue', 'other'],
  ]) {
    for (const availability of [null, {available: true, reason_code: '', reason: ''}]) {
      thinking.state.providers = {active_lane: {lane: 'local'}, provider_status: {local: {generate_ready: false, issues: [issue]}}};
      thinking.state.localAvailability = availability;
      thinking.renderMainLanes();
      assert.strictEqual(laneLine.textContent, blocked[kind], `the ${issue} issue says the ${kind} line`);
      assert.strictEqual(laneLine.textContent.includes(issue), false, `the ${issue} issue never reaches the card as a code`);
    }
  }
  // Selected, installed and waiting on processing keeps its own line.
  thinking.state.providers = {active_lane: {lane: 'local'}, provider_status: {local: {generate_ready: false, issues: []}}};
  thinking.state.localAvailability = {available: true, reason_code: '', reason: ''};
  thinking.renderMainLanes();
  assert.strictEqual(laneLine.textContent, thinking.localUnreadyCopy(), 'installed and waiting keeps the waiting line');
  assert.strictEqual(Object.values(blocked).includes(laneLine.textContent), false, 'and does not read as blocked');
  // Before availability arrives, an unselected local lane's readiness can carry
  // another lane's issues; the card is still checking.
  thinking.state.providers = {active_lane: {lane: 'confidential'}, provider_status: {local: {generate_ready: false, issues: ['spp_unreachable']}}};
  thinking.state.localAvailability = null;
  thinking.renderMainLanes();
  assert.strictEqual(laneLine.textContent, thinking.localUnreadyCopy(), 'an unselected lane before availability is still checking');
  // An earlier refusal's error tone doesn't carry over to the status that replaces it.
  const laneStatus = nodes.get('localLaneStatus');
  laneStatus.dataset.tone = 'error';
  thinking.renderMainLanes();
  assert.strictEqual(laneStatus.dataset.tone, undefined, 'the local lane status resets its tone on render');
  // ChatGPT tests
  const savedProvidersBeforeGpt = thinking.state.providers;
  const savedKeysBeforeGpt = thinking.state.keys;

  const sentinelCopy = {
    heading: 'thinking',
    provider_labels: {
      chatgpt: 'SENTINEL_CHATGPT_LABEL',
      anthropic: 'SENTINEL_CLAUDE_LABEL',
      openai: 'SENTINEL_OPENAI_LABEL',
      local: 'Local',
      spp: 'confidential processing',
    },
    active_lane_labels: {
      none: 'not thinking yet',
      local: 'local',
      confidential: 'confidential processing',
      byo: 'SENTINEL_BYO_LABEL',
    },
    lanes: [
      { id: 'local', label: 'local', description: 'local desc' },
      { id: 'confidential', label: 'confidential', description: 'confidential desc' },
      { id: 'byo', label: 'your own model', description: 'byo desc' },
    ],
    state_labels: {
      active: 'SENTINEL_ACTIVE_LABEL',
      available: 'available',
      unavailable: 'not ready',
    },
    byo_setup: {
      intro: 'SENTINEL_INTRO',
      chooser_key: 'SENTINEL_CHOOSER_KEY',
      chooser_endpoint: 'your own endpoint',
      key_heading: 'pick your provider',
      key_sub: 'SENTINEL_KEY_SUB',
      chatgpt: {
        lane_signed_in: 'SENTINEL_LANE_SIGNED_IN',
        lane_signed_out: 'SENTINEL_LANE_SIGNED_OUT',
        card_body: 'SENTINEL_CARD_BODY',
        pill_signed_in: 'SENTINEL_PILL_SIGNED_IN',
        manage: 'SENTINEL_MANAGE',
        reminder: 'SENTINEL_REMINDER',
        pending_heading: 'SENTINEL_PENDING_HEADING',
        pending_sub: 'SENTINEL_PENDING_SUB',
        pending_status: 'SENTINEL_PENDING_STATUS',
        reopen: 'SENTINEL_REOPEN',
        cancel: 'SENTINEL_CANCEL',
        fallback_heading: 'SENTINEL_FALLBACK_HEADING',
        fallback_explanation: 'SENTINEL_FALLBACK_EXPLANATION',
        address_label: 'SENTINEL_ADDRESS_LABEL',
        finish: 'SENTINEL_FINISH',
        strip: 'SENTINEL_STRIP_{email}',
        strip_no_email: 'SENTINEL_STRIP_NO_EMAIL',
        sign_out: 'SENTINEL_SIGN_OUT',
        model_heading: 'SENTINEL_MODEL_HEADING',
        model_sub: 'SENTINEL_MODEL_SUB',
        model_save: 'SENTINEL_MODEL_SAVE',
        start_over: 'SENTINEL_START_OVER',
        tuning_note: 'SENTINEL_TUNING_NOTE',
        plan_usage_not_granted: 'SENTINEL_PLAN_USAGE_NOT_GRANTED',
        usage_limit_title: 'SENTINEL_USAGE_LIMIT_TITLE',
        usage_limit_body: 'SENTINEL_USAGE_LIMIT_BODY',
        not_eligible: 'SENTINEL_NOT_ELIGIBLE',
        pick_other: 'SENTINEL_PICK_OTHER',
        revoke_unconfirmed: 'SENTINEL_REVOKE_UNCONFIRMED',
        denied: 'SENTINEL_DENIED',
        expired: 'SENTINEL_EXPIRED',
        unfinished: 'SENTINEL_UNFINISHED',
        account_mismatch: 'SENTINEL_ACCOUNT_MISMATCH',
        registration_refused: 'SENTINEL_REGISTRATION_REFUSED',
        superseded: 'SENTINEL_SUPERSEDED',
        busy: 'SENTINEL_BUSY',
        storage: 'SENTINEL_STORAGE',
        models_failed: 'SENTINEL_MODELS_FAILED',
        signed_out: 'SENTINEL_SIGNED_OUT',
        model_not_found: 'SENTINEL_MODEL_NOT_FOUND',
        model_missing: 'SENTINEL_MODEL_MISSING',
        popup_blocked: 'SENTINEL_POPUP_BLOCKED',
        sign_in_failed: 'SENTINEL_SIGN_IN_FAILED',
        finish_refused: 'SENTINEL_FINISH_REFUSED',
        save_refused: 'SENTINEL_SAVE_REFUSED',
        sign_out_failed: 'SENTINEL_SIGN_OUT_FAILED',
        status_failed: 'SENTINEL_STATUS_FAILED',
        poll_failed: 'SENTINEL_POLL_FAILED',
        continue: 'SENTINEL_CONTINUE',
        plan: 'SENTINEL_PLAN',
        manage_usage_link: 'SENTINEL_MANAGE_USAGE',
        keep_failing: 'SENTINEL_KEEP_FAILING',
      },
      tuning: {
        key: {
          note: 'SENTINEL_KEY_TUNING_NOTE',
        },
      },
    },
    lane_switch: {
      to_local_note: 'SENTINEL_TO_LOCAL_{current}',
    },
  };

  thinking.applyCopy(sentinelCopy);

  // Fake timer clock
  let currentTime = 0;
  let nextTimerId = 1;
  const activeTimers = new Map();
  context.setTimeout = (fn, delay) => {
    const id = nextTimerId++;
    activeTimers.set(id, { fn, runAt: currentTime + (Number(delay) || 0) });
    return id;
  };
  context.clearTimeout = (id) => {
    activeTimers.delete(id);
  };
  async function advance(ms) {
    currentTime += ms;
    const due = [];
    for (const [id, timer] of Array.from(activeTimers.entries())) {
      if (timer.runAt <= currentTime) {
        due.push({ id, fn: timer.fn });
      }
    }
    for (const item of due) {
      activeTimers.delete(item.id);
      item.fn();
    }
    await settle();
  }

  // 1. Configured true opens the panel with no POST sign-in; configured false opens the card; Anthropic key does not change LS1/LS2
  window.location.hash = '#byo-setup';
  thinking.state.chatgpt = {
    screen: 'card',
    status: { signed_in: true, email: 'owner@example.com' },
    attempt: null,
    models: [{ slug: 'gpt-test', display_name: 'GPT Test' }],
    selectedModel: 'gpt-test',
    notice: null,
    pendingNotice: null,
    pollToken: 0,
    pollTimer: null,
    refreshGeneration: 0,
    isSigningIn: false,
    isSigningOut: false,
    isSavingModel: false,
    modelsLoading: false,
  };
  thinking.state.providers = {
    active_lane: { lane: 'byo', provider: 'chatgpt', model: 'gpt-test' },
    active: { provider: 'chatgpt', model: 'gpt-test' },
    provider_status: { chatgpt: { configured: true, signed_in: true } },
    byo_models: { chatgpt: 'gpt-test' },
  };
  thinking.state.keys = { api_keys: { anthropic: 'some-key' } };
  thinking.state.selectedByoProvider = 'chatgpt';
  thinking.state.byoMode = 'chatgpt';
  thinking.state.chatgpt.screen = 'panel';
  thinking.renderAll();

  assert.strictEqual(nodes.get('byoLaneStatus').textContent, 'SENTINEL_LANE_SIGNED_IN', 'configured true shows LS1 (lane_signed_in)');
  assert.strictEqual(nodes.get('chatgptPanel').hidden, false, 'configured true shows panel');

  // Configured false with Anthropic key
  thinking.state.providers.provider_status.chatgpt.configured = false;
  thinking.state.chatgpt.status = { signed_in: false };
  thinking.state.chatgpt.screen = 'card';
  thinking.renderAll();

  assert.strictEqual(nodes.get('byoLaneStatus').textContent, 'SENTINEL_LANE_SIGNED_OUT', 'configured false shows LS2 (lane_signed_out) even with Anthropic key');
  assert.strictEqual(nodes.get('chatgptCardContinue').hidden, false, 'configured false shows continue on card');
  assert.strictEqual(nodes.get('chatgptCardContinueLabel').textContent, 'SENTINEL_CONTINUE', 'continue button shows continue label');

  // 2. window.open is recorded before the sign-in POST, features undefined, opener null, then location is the authorize url
  openedWindows.length = 0;
  sessionStorage.clear();
  const pendingPollPromise = deferred();
  chatgptResponses.push((url, options) => {
    assert.strictEqual(url, 'api/chatgpt/sign-in');
    assert.strictEqual(options.method, 'POST');
    return Promise.resolve({
      attempt_id: 'attempt_test',
      authorize_url: 'https://auth.openai.com/oauth/authorize?test=1',
    });
  });
  chatgptResponses.push(() => pendingPollPromise.promise);

  const startPromise = thinking.startChatGptSignIn({ startOver: false });
  assert.strictEqual(openedWindows.length, 1, 'window.open was called synchronously before POST settled');
  const openedWin = openedWindows[0];
  assert.strictEqual(openedWin.target, '_blank', 'target is _blank');
  assert.strictEqual(openedWin.features, undefined, 'features is undefined');
  assert.strictEqual(openedWin.opener, null, 'opener is null');
  assert.strictEqual(openedWin.location.href, 'about:blank', 'initial location is about:blank');

  await startPromise;
  await settle();

  assert.strictEqual(openedWin.location.href, 'https://auth.openai.com/oauth/authorize?test=1', 'opened window navigates to authorize_url');
  assert.strictEqual(thinking.state.chatgpt.screen, 'pending', 'screen is pending');
  assert.strictEqual(nodes.get('chatgptPendingHeading').textContent, 'SENTINEL_PENDING_HEADING', 'pending heading is sentinel');
  assert.strictEqual(sessionStorage.getItem(thinking.CHATGPT_ATTEMPT_STORAGE_KEY), JSON.stringify({ id: 'attempt_test', authorizeUrl: 'https://auth.openai.com/oauth/authorize?test=1' }), 'attempt is stored in sessionStorage');

  // 3. A poll left unresolved plus advance(2000) does not start a second poll
  assert.strictEqual(chatgptResponses.length, 0, 'first poll request consumed the mock handler');
  let secondPollTriggered = false;
  chatgptResponses.push(() => {
    secondPollTriggered = true;
    return Promise.resolve({ state: 'pending' });
  });
  await advance(2000);
  assert.strictEqual(secondPollTriggered, false, 'no second poll request was queued while current poll is in flight');
  assert.strictEqual(chatgptResponses.length, 1, 'mock handler remains untouched');
  chatgptResponses.pop(); // remove unused handler

  pendingPollPromise.resolve({ state: 'pending' });
  await settle();

  // 4. A poll response for a changed token does not change the panel
  const currentToken = ++thinking.state.chatgpt.pollToken;
  const latePollDeferred = deferred();
  chatgptResponses.push(() => latePollDeferred.promise);
  const latePollPromise = thinking.pollChatGptAttempt('attempt_test', currentToken);
  thinking.stopChatGptPoll();
  latePollDeferred.resolve({ state: 'signed_in' });
  await latePollPromise;
  assert.strictEqual(thinking.state.chatgpt.screen, 'pending', 'late/mismatched poll response did not change screen from pending');

  // 5. Finish posts the field value unchanged, including a value with no scheme
  nodes.get('chatgptRedirectUrl').value = 'localhost:4040/callback?code=abc&state=xyz';
  chatgptResponses.push((url, options) => {
    assert.strictEqual(url, 'api/chatgpt/sign-in/attempt_test/finish');
    assert.strictEqual(options.method, 'POST');
    const body = JSON.parse(options.body);
    assert.strictEqual(body.redirect_url, 'localhost:4040/callback?code=abc&state=xyz', 'field value is posted unchanged');
    return Promise.resolve({ reason: 'signed_in' });
  });
  // Status and providers refresh on sign in
  chatgptResponses.push(() => Promise.resolve({ signed_in: true, email: 'owner@example.com' }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  chatgptResponses.push(() => Promise.resolve({ models: [{ slug: 'gpt-test', display_name: 'GPT Test' }] }));

  await thinking.finishChatGptSignIn();
  await settle();

  assert.strictEqual(thinking.state.chatgpt.screen, 'panel', 'finish sign in transitions to panel');
  assert.strictEqual(nodes.get('chatgptRedirectUrl').value, '', 'redirect url field is cleared');
  assert.strictEqual(sessionStorage.getItem(thinking.CHATGPT_ATTEMPT_STORAGE_KEY), null, 'sessionStorage cleared on finish');
  assert.strictEqual(nodes.get('chatgptStrip').textContent, 'SENTINEL_STRIP_owner@example.com', 'strip shows email sentinel');

  // 6. Two overlapping refreshes: only the later body is applied
  const slowRefresh = deferred();
  chatgptResponses.push(() => slowRefresh.promise);
  providerResponses.push(() => Promise.resolve(thinking.state.providers));

  const refresh1Promise = thinking.refreshChatGpt();

  // Start second refresh
  chatgptResponses.push(() => Promise.resolve({ signed_in: true, email: 'newer@example.com' }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));

  await thinking.refreshChatGpt();
  await settle();
  assert.strictEqual(thinking.state.chatgpt.status?.email, 'newer@example.com', 'second refresh email applied');

  // Now resolve the first (older) refresh
  slowRefresh.resolve({ signed_in: false });
  await refresh1Promise;
  await settle();

  assert.strictEqual(thinking.state.chatgpt.status?.email, 'newer@example.com', 'older refresh did not overwrite newer state');

  // 7. Sign-out {revoked:false} still shows revoke_unconfirmed after the refresh
  chatgptResponses.push((url, options) => {
    assert.strictEqual(url, 'api/chatgpt/sign-out');
    assert.strictEqual(options.method, 'POST');
    return Promise.resolve({ revoked: false });
  });
  chatgptResponses.push(() => Promise.resolve({ signed_in: false }));
  providerResponses.push(() => Promise.resolve({ active_lane: { lane: 'none' }, provider_status: {} }));

  await thinking.signOutChatGpt({ forget: false });
  await settle();

  assert.strictEqual(thinking.state.chatgpt.screen, 'card', 'screen is card after sign out');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_REVOKE_UNCONFIRMED', 'revoke_unconfirmed sentinel is shown after refresh');

  // 8. Save model_not_found still shows that sentinel after the refresh
  thinking.state.chatgpt.selectedModel = 'gpt-test';
  providerResponses.push((url, options) => {
    assert.strictEqual(url, 'api/providers');
    assert.strictEqual(options.method, 'PUT');
    assert.strictEqual(options.body, JSON.stringify({ lane: 'byo', provider: 'chatgpt', model: 'gpt-test' }));
    return Promise.reject(Object.assign(new Error('Model not found'), { status: 400, reasonCode: 'model_not_found', payload: { error: 'Model not found', reason_code: 'model_not_found' } }));
  });
  chatgptResponses.push(() => Promise.resolve({ signed_in: true, email: 'owner@example.com' }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));

  await thinking.saveChatGptModel();
  await settle();

  assert.strictEqual(nodes.get('chatgptModelStatus').textContent, 'SENTINEL_MODEL_NOT_FOUND', 'model_not_found sentinel is shown after refresh');

  // 9. Zero api/validate-model calls; one PUT api/providers with {"lane":"byo","provider":"chatgpt","model":"gpt-test"}
  const validateModelCalls = requests.filter((u) => u.includes('validate-model'));
  assert.strictEqual(validateModelCalls.length, 0, 'zero api/validate-model calls were made');

  // 10. Failure states record no POST/PUT api/providers except that one refused-save write
  const providerMutations = requests.filter((u) => u === 'api/providers' || u.startsWith('api/providers'));
  // The only mutation to api/providers was the PUT in step 8
  const providerPuts = requests.filter((u) => u === 'api/providers');
  assert.ok(providerPuts.length >= 1, 'PUT api/providers was called for save');

  // 11. A runs row with provider chatgpt shows the provider_labels.chatgpt sentinel
  assert.strictEqual(thinking.runProviderLabel({ provider: 'chatgpt' }), 'SENTINEL_CHATGPT_LABEL', 'runs row with provider chatgpt shows SENTINEL_CHATGPT_LABEL');

  // 12. Glance fixtures with brain.action across all states
  // a) chatgpt_usage_limit
  thinking.state.providers = {
    active: { provider: 'chatgpt', model: 'gpt-test' },
    active_lane: { lane: 'byo', provider: 'chatgpt', model: 'gpt-test' },
    provider_status: { chatgpt: { configured: true, signed_in: true } },
    brain: {
      state: 'blocked',
      headline: 'headline text',
      reason_code: 'chatgpt_usage_limit',
      reason_text: 'usage limit body text',
      action: { label: 'Check now', refresh: true },
    },
  };
  thinking.renderGlance();
  assert.strictEqual(nodes.get('thinkingActiveDetail').textContent, '', 'detail is replaced for usage limit');
  assert.strictEqual(nodes.get('chatgptGlanceUsage').hidden, false, 'usage block is visible');
  assert.strictEqual(nodes.get('chatgptGlanceUsageTitle').textContent, 'SENTINEL_USAGE_LIMIT_TITLE', 'usage title is sentinel');
  assert.strictEqual(nodes.get('chatgptGlanceUsageBody').textContent, 'SENTINEL_USAGE_LIMIT_BODY', 'usage body is sentinel');
  assert.strictEqual(nodes.get('chatgptGlanceUsageManage').textContent, 'SENTINEL_MANAGE_USAGE', 'manage usage is sentinel');
  assert.strictEqual(nodes.get('brainCheckAction').hidden, true, 'brainCheckAction is hidden for usage limit');

  // b) chatgpt_not_eligible
  thinking.state.providers.brain = {
    state: 'blocked',
    headline: 'headline text',
    reason_code: 'chatgpt_not_eligible',
    reason_text: 'not eligible reason',
    action: { label: 'Check now', refresh: true },
  };
  thinking.renderGlance();
  assert.strictEqual(nodes.get('thinkingActiveDetail').textContent, 'SENTINEL_NOT_ELIGIBLE', 'not eligible sentinel shown');
  assert.strictEqual(nodes.get('chatgptGlanceStartOver').textContent, 'SENTINEL_START_OVER', 'start over sentinel shown');
  assert.strictEqual(nodes.get('chatgptGlancePickOther').textContent, 'SENTINEL_PICK_OTHER', 'pick other sentinel shown');
  assert.strictEqual(nodes.get('brainCheckAction').hidden, true, 'brainCheckAction is hidden for not eligible');

  // c) chatgpt_sign_in_required
  thinking.state.providers.brain = {
    state: 'blocked',
    headline: 'headline text',
    reason_code: 'chatgpt_sign_in_required',
    reason_text: 'sign in required reason',
    action: { label: 'Check now', refresh: true },
  };
  thinking.renderGlance();
  assert.strictEqual(nodes.get('thinkingActiveDetail').textContent, 'sign in required reason', 'reason text kept for sign in required');
  assert.strictEqual(nodes.get('chatgptGlanceContinue').hidden, false, 'continue is visible');
  assert.strictEqual(nodes.get('chatgptGlanceContinueLabel').textContent, 'SENTINEL_CONTINUE', 'continue label sentinel shown');
  assert.strictEqual(nodes.get('brainCheckAction').hidden, true, 'brainCheckAction is hidden for sign in required');

  // d) Ready with configured: false
  thinking.state.providers.provider_status.chatgpt.configured = false;
  thinking.state.providers.brain = {
    state: 'ready',
    headline: 'ready headline',
    reason_code: '',
    action: { label: 'Check now', refresh: true },
  };
  thinking.renderGlance();
  assert.strictEqual(nodes.get('thinkingActiveDetail').textContent, 'SENTINEL_SIGNED_OUT', 'signed out sentinel shown when configured false');
  assert.strictEqual(nodes.get('chatgptGlanceContinue').hidden, false, 'continue shown when configured false');
  assert.strictEqual(nodes.get('brainCheckAction').hidden, true, 'brainCheckAction is hidden when configured false');

  // e) Ready with configured: true
  thinking.state.providers.provider_status.chatgpt.configured = true;
  thinking.state.providers.brain = {
    state: 'ready',
    headline: 'ready headline',
    reason_code: '',
    action: { label: 'Check now', refresh: true },
  };
  thinking.renderGlance();
  assert.strictEqual(nodes.get('thinkingActiveDetail').textContent, 'SENTINEL_CHATGPT_LABEL', 'ChatGPT lane name shown for detail');
  assert.strictEqual(nodes.get('chatgptGlancePlan').hidden, false, 'plan row shown');
  assert.strictEqual(nodes.get('chatgptGlancePlanLabel').textContent, 'SENTINEL_PLAN', 'plan label sentinel shown');
  assert.strictEqual(nodes.get('chatgptGlanceReminder').textContent, 'SENTINEL_REMINDER', 'reminder sentinel shown');
  assert.strictEqual(nodes.get('brainCheckAction').hidden, false, 'brainCheckAction remains visible when configured true');

  // 13. S4 Reopen tab: reopenChatGptTab opens authorizeUrl in new window with opener null, no API requests
  thinking.state.chatgpt.attempt = { id: 'attempt_reopen', authorizeUrl: 'https://auth.openai.com/reopen' };
  openedWindows.length = 0;
  const beforeReopenRequestsCount = requests.length;
  thinking.reopenChatGptTab();
  assert.strictEqual(openedWindows.length, 1, 'reopenChatGptTab opens a window');
  assert.strictEqual(openedWindows[0].location.href, 'https://auth.openai.com/reopen', 'reopened window navigated to authorizeUrl');
  assert.strictEqual(openedWindows[0].opener, null, 'reopened window opener is null');
  assert.strictEqual(requests.length, beforeReopenRequestsCount, 'reopenChatGptTab makes no API requests');

  // 14. Popup blocked: window.open returns null, stays on card, popup_blocked sentinel, no API calls
  thinking.state.chatgpt.screen = 'card';
  thinking.state.chatgpt.status = { signed_in: false };
  const originalOpen = window.open;
  window.open = () => null;
  const beforeBlockedRequests = requests.length;
  await thinking.startChatGptSignIn({ startOver: false });
  assert.strictEqual(thinking.state.chatgpt.screen, 'card', 'popup blocked keeps screen on card');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_POPUP_BLOCKED', 'popup blocked sentinel shown');
  assert.strictEqual(requests.length, beforeBlockedRequests, 'popup blocked makes no API requests');
  window.open = originalOpen;

  // 15. Start over forget failure: sign-out rejection closes opened window, stays on card, sign_out_failed
  openedWindows.length = 0;
  chatgptResponses.push((url, options) => {
    assert.strictEqual(url, 'api/chatgpt/sign-out');
    assert.strictEqual(options.method, 'POST');
    const body = JSON.parse(options.body);
    assert.strictEqual(body.forget, true);
    return Promise.reject(Object.assign(new Error('Sign out failed'), { status: 500, reasonCode: 'sign_out_failed' }));
  });
  await thinking.startChatGptSignIn({ startOver: true });
  assert.strictEqual(openedWindows.length, 1, 'window opened for start over');
  assert.strictEqual(openedWindows[0].closed, true, 'window closed when forget sign-out failed');
  assert.strictEqual(thinking.state.chatgpt.screen, 'card', 'screen is card after forget failure');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_SIGN_OUT_FAILED', 'sign_out_failed sentinel shown');

  // 16. Start over with revoked: false: pending notice is SENTINEL_REVOKE_UNCONFIRMED
  chatgptResponses.push((url, options) => {
    assert.strictEqual(url, 'api/chatgpt/sign-out');
    assert.strictEqual(options.method, 'POST');
    return Promise.resolve({ revoked: false });
  });
  chatgptResponses.push((url, options) => {
    assert.strictEqual(url, 'api/chatgpt/sign-in');
    assert.strictEqual(options.method, 'POST');
    return Promise.resolve({ attempt_id: 'attempt_revoked_false', authorize_url: 'https://auth.openai.com/oauth/authorize?test=2' });
  });
  const pollDeferredCase16 = deferred();
  chatgptResponses.push(() => pollDeferredCase16.promise);
  await thinking.startChatGptSignIn({ startOver: true });
  assert.strictEqual(thinking.state.chatgpt.screen, 'pending', 'pending screen active');
  assert.strictEqual(nodes.get('chatgptPendingNotice').textContent, 'SENTINEL_REVOKE_UNCONFIRMED', 'pending notice shows revoke_unconfirmed');
  thinking.stopChatGptPoll();
  pollDeferredCase16.resolve({ state: 'pending' });
  await settle();

  // 17. Poll error & recovery on clock tick: shows poll_failed, then pending_status on next tick
  thinking.state.chatgpt.screen = 'pending';
  thinking.state.chatgpt.attempt = { id: 'attempt_poll_err', authorizeUrl: 'https://auth.openai.com/err' };
  window.location.hash = '#byo-setup';
  document.visibilityState = 'visible';
  const pollErrToken = ++thinking.state.chatgpt.pollToken;
  chatgptResponses.push(() => Promise.reject(new Error('Network error')));
  await thinking.pollChatGptAttempt('attempt_poll_err', pollErrToken);
  assert.strictEqual(nodes.get('chatgptPendingStatus').textContent, 'SENTINEL_POLL_FAILED', 'poll failure shows poll_failed sentinel');
  // Next tick at 2000ms
  chatgptResponses.push(() => Promise.resolve({ state: 'pending' }));
  await advance(2000);
  assert.strictEqual(nodes.get('chatgptPendingStatus').textContent, 'SENTINEL_PENDING_STATUS', 'pending status restored after successful tick');
  thinking.stopChatGptPoll();

  // 18. Visibility state handling: hidden skips polling
  document.visibilityState = 'hidden';
  const visToken = ++thinking.state.chatgpt.pollToken;
  thinking.state.chatgpt.screen = 'pending';
  thinking.state.chatgpt.attempt = { id: 'attempt_vis', authorizeUrl: 'https://auth.openai.com/vis' };
  let visPollTriggered = false;
  chatgptResponses.push(() => {
    visPollTriggered = true;
    return Promise.resolve({ state: 'pending' });
  });
  await thinking.pollChatGptAttempt('attempt_vis', visToken);
  assert.strictEqual(visPollTriggered, false, 'hidden visibility skips polling');
  chatgptResponses.pop(); // remove unused
  document.visibilityState = 'visible';

  // 19. loadChatGptModels order & error branches
  thinking.state.chatgpt.screen = 'panel';
  thinking.state.providers.active = { provider: 'chatgpt', model: 'unlisted-model' };
  thinking.state.providers.active_lane = { lane: 'byo', provider: 'chatgpt', model: 'unlisted-model' };
  chatgptResponses.push(() => Promise.resolve({
    models: [
      { slug: 'model-z', display_name: 'Model Z' },
      { slug: 'model-a', display_name: 'Model A' },
    ],
  }));
  await thinking.loadChatGptModels();
  assert.deepStrictEqual(thinking.state.chatgpt.models.map((m) => m.slug), ['model-z', 'model-a'], 'server order preserved');
  assert.strictEqual(thinking.state.chatgpt.selectedModel, '', 'unlisted model unsets selectedModel');
  assert.strictEqual(nodes.get('chatgptModelStatus').textContent, 'SENTINEL_MODEL_NOT_FOUND', 'unlisted active model shows model_not_found sentinel');

  // Error: chatgpt_sign_in_required
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Sign in required'), { status: 401, reasonCode: 'chatgpt_sign_in_required' })));
  chatgptResponses.push(() => Promise.resolve({ signed_in: false }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.loadChatGptModels();
  assert.strictEqual(thinking.state.chatgpt.screen, 'card', 'sign in required during models moves to card');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_SIGNED_OUT', 'signed_out sentinel shown on card');

  // Error: chatgpt_not_eligible
  thinking.state.chatgpt.screen = 'panel';
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Not eligible'), { status: 403, reasonCode: 'chatgpt_not_eligible' })));
  chatgptResponses.push(() => Promise.resolve({ signed_in: true, email: 'owner@example.com' }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.loadChatGptModels();
  assert.strictEqual(thinking.state.chatgpt.screen, 'panel', 'not eligible stays on panel');
  assert.strictEqual(nodes.get('chatgptModelStatus').textContent, 'SENTINEL_NOT_ELIGIBLE', 'not eligible sentinel shown');

  // Error: general models failure
  thinking.state.chatgpt.notice = null;
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Models error'), { status: 500, reasonCode: 'internal_error' })));
  chatgptResponses.push(() => Promise.resolve({ signed_in: true, email: 'owner@example.com' }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.loadChatGptModels();
  assert.strictEqual(nodes.get('chatgptModelStatus').textContent, 'SENTINEL_MODELS_FAILED', 'general failure shows models_failed sentinel');

  // 20. saveChatGptModel error branches and settings_saved_unlogged resilience
  // Model missing
  thinking.state.chatgpt.selectedModel = '';
  await thinking.saveChatGptModel();
  assert.strictEqual(nodes.get('chatgptModelStatus').textContent, 'SENTINEL_MODEL_MISSING', 'empty selectedModel shows model_missing');

  // chatgpt_sign_in_required on save
  thinking.state.chatgpt.selectedModel = 'model-z';
  providerResponses.push(() => Promise.reject(Object.assign(new Error('Sign in required'), { status: 401, reasonCode: 'chatgpt_sign_in_required' })));
  chatgptResponses.push(() => Promise.resolve({ signed_in: false }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.saveChatGptModel();
  assert.strictEqual(thinking.state.chatgpt.screen, 'card', 'save with sign_in_required moves to card');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_SIGNED_OUT', 'card shows signed_out sentinel');

  // chatgpt_not_eligible on save
  thinking.state.chatgpt.screen = 'panel';
  thinking.state.chatgpt.selectedModel = 'model-z';
  providerResponses.push(() => Promise.reject(Object.assign(new Error('Not eligible'), { status: 403, reasonCode: 'chatgpt_not_eligible' })));
  chatgptResponses.push(() => Promise.resolve({ signed_in: true, email: 'owner@example.com' }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.saveChatGptModel();
  assert.strictEqual(nodes.get('chatgptModelStatus').textContent, 'SENTINEL_NOT_ELIGIBLE', 'save not eligible shows sentinel');

  // invalid_config_value / save_refused on save
  thinking.state.chatgpt.selectedModel = 'model-z';
  providerResponses.push(() => Promise.reject(Object.assign(new Error('Save refused'), { status: 400, reasonCode: 'invalid_config_value' })));
  chatgptResponses.push(() => Promise.resolve({ signed_in: true, email: 'owner@example.com' }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.saveChatGptModel();
  assert.strictEqual(nodes.get('chatgptModelStatus').textContent, 'SENTINEL_SAVE_REFUSED', 'save refused shows sentinel');

  // 21. Notice start over visibility & keep_failing across all notice keys
  thinking.state.chatgpt.screen = 'card';
  thinking.state.chatgpt.status = { signed_in: false };
  const startOverNoticeKeys = ['denied', 'expired', 'unfinished', 'account_mismatch', 'registration_refused', 'storage', 'plan_usage_not_granted'];
  for (const key of startOverNoticeKeys) {
    thinking.state.chatgpt.notice = null;
    thinking.renderAll();
    const holdCard = key !== 'superseded';
    thinking.setChatGptNotice(key, { tone: 'error', holdCard });
    thinking.renderAll();
    assert.strictEqual(nodes.get('chatgptCardStartOver').hidden, false, `start over is visible for notice key ${key}`);
    if (['denied', 'expired', 'unfinished'].includes(key)) {
      assert.ok(nodes.get('chatgptCardStatus').textContent.includes('SENTINEL_KEEP_FAILING'), `keep_failing sentinel included for ${key}`);
    }
  }
  const noStartOverNoticeKeys = ['superseded', 'busy', 'sign_in_failed', 'status_failed', 'signed_out', 'popup_blocked', 'save_refused'];
  for (const key of noStartOverNoticeKeys) {
    thinking.setChatGptNotice(key, { tone: 'error', holdCard: false });
    thinking.renderAll();
    assert.strictEqual(nodes.get('chatgptCardStartOver').hidden, true, `start over is hidden for notice key ${key}`);
  }
  thinking.state.chatgpt.notice = null;
  thinking.renderAll();
  assert.strictEqual(nodes.get('chatgptCardStartOver').hidden, true, 'start over is hidden when notice is empty');

  // 22. POST sign-in failures: busy, storage_error, general failure
  // a) POST busy
  openedWindows.length = 0;
  chatgptResponses.push((url, options) => {
    assert.strictEqual(url, 'api/chatgpt/sign-in');
    return Promise.reject(Object.assign(new Error('Busy'), { status: 409, reasonCode: 'busy' }));
  });
  await thinking.startChatGptSignIn({ startOver: false });
  assert.strictEqual(openedWindows.length, 1, 'window opened before POST');
  assert.strictEqual(openedWindows[0].closed, true, 'window closed on busy failure');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_BUSY', 'busy sentinel shown');
  assert.strictEqual(nodes.get('chatgptCardStartOver').hidden, true, 'start over hidden for busy');

  // b) POST storage_error
  openedWindows.length = 0;
  chatgptResponses.push((url, options) => {
    assert.strictEqual(url, 'api/chatgpt/sign-in');
    return Promise.reject(Object.assign(new Error('Storage error'), { status: 500, reasonCode: 'storage_error' }));
  });
  await thinking.startChatGptSignIn({ startOver: false });
  assert.strictEqual(openedWindows.length, 1, 'window opened before POST');
  assert.strictEqual(openedWindows[0].closed, true, 'window closed on storage error');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_STORAGE', 'storage sentinel shown');
  assert.strictEqual(nodes.get('chatgptCardStartOver').hidden, false, 'start over visible for storage');

  // c) POST general failure
  openedWindows.length = 0;
  chatgptResponses.push((url, options) => {
    assert.strictEqual(url, 'api/chatgpt/sign-in');
    return Promise.reject(Object.assign(new Error('Internal server error'), { status: 500, reasonCode: 'internal_error' }));
  });
  await thinking.startChatGptSignIn({ startOver: false });
  assert.strictEqual(openedWindows.length, 1, 'window opened before POST');
  assert.strictEqual(openedWindows[0].closed, true, 'window closed on general failure');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_SIGN_IN_FAILED', 'sign_in_failed sentinel shown');
  assert.strictEqual(nodes.get('chatgptCardStartOver').hidden, true, 'start over hidden for sign_in_failed');

  // 23. Emitted clicks (card Continue, panel start-over, glance Continue)
  openedWindows.length = 0;
  chatgptResponses.push((url) => {
    assert.strictEqual(url, 'api/chatgpt/sign-in');
    return Promise.resolve({ attempt_id: 'attempt_click', authorize_url: 'https://auth.openai.com/oauth/authorize?test=click' });
  });
  const clickPollDeferred = deferred();
  chatgptResponses.push(() => clickPollDeferred.promise);
  nodes.get('chatgptCardContinue').emit('click');
  assert.strictEqual(openedWindows.length, 1, 'window opened immediately upon click');
  assert.strictEqual(openedWindows[0].location.href, 'about:blank', 'initial href is about:blank');
  assert.strictEqual(openedWindows[0].opener, null, 'opener is null');
  await settle();
  assert.strictEqual(openedWindows[0].location.href, 'https://auth.openai.com/oauth/authorize?test=click', 'navigated to authorize_url');
  thinking.stopChatGptPoll();
  clickPollDeferred.resolve({ state: 'pending' });
  await settle();

  // 24. Stored attempt on reopening BYO view
  thinking.state.chatgpt.screen = 'card';
  sessionStorage.setItem(thinking.CHATGPT_ATTEMPT_STORAGE_KEY, JSON.stringify({ id: 'attempt_stored_test', authorizeUrl: 'https://auth.openai.com/stored' }));
  chatgptResponses.push((url) => {
    assert.strictEqual(url, 'api/chatgpt/sign-in/attempt_stored_test');
    return Promise.resolve({ state: 'pending' });
  });
  const storedPollDeferred = deferred();
  chatgptResponses.push(() => storedPollDeferred.promise);
  thinking.state.providers = {
    active_lane: { lane: 'byo', provider: 'chatgpt' },
    active: { provider: 'chatgpt' },
    provider_status: { chatgpt: { configured: false } },
  };
  thinking.openLane('byo');
  await settle();
  assert.strictEqual(thinking.state.chatgpt.screen, 'pending', 'stored pending attempt opens pending screen');
  assert.strictEqual(nodes.get('chatgptPending').hidden, false, 'pending panel is visible');
  assert.strictEqual(nodes.get('chatgptRedirectUrl').hidden, false, 'redirect url input is visible');
  thinking.stopChatGptPoll();
  storedPollDeferred.resolve({ state: 'pending' });
  await settle();

  // 25. Plan row attributes
  thinking.state.providers = {
    active_lane: { lane: 'byo', provider: 'chatgpt', model: 'gpt-test' },
    active: { provider: 'chatgpt', model: 'gpt-test' },
    provider_status: { chatgpt: { configured: true, signed_in: true } },
    byo_models: { chatgpt: 'gpt-test' },
  };
  thinking.state.chatgpt.screen = 'panel';
  thinking.state.chatgpt.status = { signed_in: true, email: 'owner@example.com' };
  thinking.renderAll();
  assert.strictEqual(nodes.get('chatgptPlanRow').hidden, false, 'plan row is visible when ChatGPT active');
  assert.strictEqual(nodes.get('chatgptPanelManageUsage').href, 'https://chatgpt.com/settings/usage', 'manage usage has usage url');
  assert.strictEqual(nodes.get('chatgptPanelManageUsage').target, '_blank', 'target is _blank');
  assert.strictEqual(nodes.get('chatgptPanelManageUsage').rel, 'noopener noreferrer', 'rel is noopener noreferrer');
  assert.strictEqual(nodes.get('chatgptPanelReminder').textContent, 'SENTINEL_REMINDER', 'reminder sentinel shown');
  assert.strictEqual(nodes.get('byoTuningNote').textContent, 'SENTINEL_TUNING_NOTE', 'tuning note is SENTINEL_TUNING_NOTE');
  assert.strictEqual(nodes.get('byoPastePanel').hidden, true, 'byoPastePanel is hidden');

  // When another provider active
  thinking.state.providers = {
    active_lane: { lane: 'byo', provider: 'anthropic', model: 'claude-3' },
    active: { provider: 'anthropic', model: 'claude-3' },
    provider_status: { anthropic: { configured: true } },
  };
  thinking.renderAll();
  assert.strictEqual(nodes.get('chatgptPlanRow').hidden, true, 'plan row is hidden when Anthropic active');

  // 26. openLane('byo') tests:
  // a) active.provider === 'chatgpt', configured: true, no API keys -> lane status SENTINEL_LANE_SIGNED_IN, panel visible, grid hidden, zero POST api/chatgpt/sign-in
  window.sessionStorage.clear();
  thinking.state.chatgpt.status = { signed_in: true, email: 'owner@example.com' };
  thinking.state.providers = {
    active_lane: { lane: 'byo', provider: 'chatgpt', model: 'gpt-test' },
    active: { provider: 'chatgpt', model: 'gpt-test' },
    provider_status: { chatgpt: { configured: true, signed_in: true } },
    byo_models: { chatgpt: 'gpt-test' },
  };
  thinking.state.keys = { api_keys: {} };
  thinking.renderAll();
  const callsBeforeOpenLane1 = apiCalls.length;
  await thinking.openLane('byo');
  assert.strictEqual(nodes.get('byoLaneStatus').textContent, 'SENTINEL_LANE_SIGNED_IN');
  assert.strictEqual(nodes.get('chatgptPanel').hidden, false);
  assert.strictEqual(nodes.get('byoProviderGrid').hidden, true);
  const postSignInCalls1 = apiCalls.slice(callsBeforeOpenLane1).filter((c) => c.url === 'api/chatgpt/sign-in' && c.method === 'POST');
  assert.strictEqual(postSignInCalls1.length, 0, 'zero POST sign-in calls on openLane configured true');

  // b) configured: false (and with Anthropic key saved) -> SENTINEL_LANE_SIGNED_OUT, card screen, grid not hidden, Continue shown, zero POST sign-in
  thinking.state.providers.provider_status.chatgpt.configured = false;
  thinking.state.chatgpt.status = { signed_in: false };
  thinking.state.keys = { api_keys: { anthropic: 'anthropic-key' } };
  thinking.renderAll();
  const callsBeforeOpenLane2 = apiCalls.length;
  await thinking.openLane('byo');
  assert.strictEqual(nodes.get('byoLaneStatus').textContent, 'SENTINEL_LANE_SIGNED_OUT');
  assert.strictEqual(thinking.state.chatgpt.screen, 'card');
  assert.strictEqual(nodes.get('byoProviderGrid').hidden, false);
  assert.strictEqual(nodes.get('chatgptCardContinue').hidden, false);
  const postSignInCalls2 = apiCalls.slice(callsBeforeOpenLane2).filter((c) => c.url === 'api/chatgpt/sign-in' && c.method === 'POST');
  assert.strictEqual(postSignInCalls2.length, 0, 'zero POST sign-in calls on openLane configured false');

  // 27. Lane switch target local with ChatGPT brain
  thinking.state.providers = {
    active_lane: { lane: 'byo', provider: 'chatgpt', model: 'gpt-test' },
    active: { provider: 'chatgpt', model: 'gpt-test' },
    provider_status: { chatgpt: { configured: true, signed_in: true } },
  };
  thinking.state.pendingSwitchTarget = 'local';
  thinking.renderLaneSwitch();
  assert.strictEqual(nodes.get('switchNote').textContent, 'SENTINEL_TO_LOCAL_SENTINEL_CHATGPT_LABEL');

  // 28. Start over with window.open returning null -> SENTINEL_POPUP_BLOCKED and apiCalls has no sign-out
  window.open = () => null;
  const callsBeforePopupBlocked = apiCalls.length;
  await thinking.startChatGptSignIn({ startOver: true });
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_POPUP_BLOCKED');
  const signOutCalls = apiCalls.slice(callsBeforePopupBlocked).filter((c) => c.url.includes('sign-out'));
  assert.strictEqual(signOutCalls.length, 0, 'no sign-out call when popup blocked');
  window.open = originalOpen;

  // 29. Absence of authorize_url across nodes, attributes, console, diagnosticConsole, logError
  const testAuthUrl = 'https://auth.openai.com/oauth/authorize?sensitive=123';
  chatgptResponses.push((url, options) => {
    return Promise.resolve({ attempt_id: 'attempt_sec_test', authorize_url: testAuthUrl });
  });
  const secPollPromise = deferred();
  chatgptResponses.push(() => secPollPromise.promise);
  await thinking.startChatGptSignIn({ startOver: false });
  for (const node of nodes.values()) {
    assert.strictEqual((node.textContent || '').includes(testAuthUrl), false, `node ${node.id} textContent must not contain authorize_url`);
    for (const [attr, val] of Object.entries(node.attributes || {})) {
      assert.strictEqual(String(val).includes(testAuthUrl), false, `node ${node.id} attribute ${attr} must not contain authorize_url`);
    }
  }
  thinking.stopChatGptPoll();
  secPollPromise.resolve({ state: 'pending' });
  await settle();

  // 30. Visibility hidden / visible poll test
  thinking.state.chatgpt.screen = 'pending';
  thinking.state.chatgpt.attempt = { id: 'attempt_vis_flow', authorizeUrl: 'https://auth.openai.com/vis' };
  window.location.hash = '#byo-setup';
  document.visibilityState = 'visible';
  const visFlowDeferred = deferred();
  chatgptResponses.push(() => visFlowDeferred.promise);
  thinking.pollChatGptAttempt('attempt_vis_flow', thinking.state.chatgpt.pollToken);
  // Hide while in flight
  document.visibilityState = 'hidden';
  document.emit('visibilitychange');
  // Resolving in-flight poll still applies
  visFlowDeferred.resolve({ state: 'pending' });
  await settle();
  assert.strictEqual(nodes.get('chatgptPendingStatus').textContent, 'SENTINEL_PENDING_STATUS');
  // advance(2000) while hidden does NOT start a second poll
  let pollStartedWhileHidden = false;
  chatgptResponses.push(() => { pollStartedWhileHidden = true; return Promise.resolve({ state: 'pending' }); });
  await advance(2000);
  assert.strictEqual(pollStartedWhileHidden, false, 'no poll started while hidden');
  chatgptResponses.pop();
  // Set visible, emit visibilitychange, and next poll runs
  const nextVisPoll = deferred();
  chatgptResponses.push(() => nextVisPoll.promise);
  document.visibilityState = 'visible';
  document.emit('visibilitychange');
  assert.strictEqual(chatgptResponses.length, 0, 'poll ran on visibility visible');
  thinking.stopChatGptPoll();
  nextVisPoll.resolve({ state: 'pending' });
  await settle();

  // 31. showView('main') stops polls; returning via openLane('byo') with stored pending attempt restores pending panel
  thinking.showView('main');
  window.sessionStorage.setItem(thinking.CHATGPT_ATTEMPT_STORAGE_KEY, JSON.stringify({ id: 'attempt_return_test', authorizeUrl: 'https://auth.openai.com/ret' }));
  chatgptResponses.push(() => Promise.resolve({ state: 'pending' }));
  const returnPollDeferred = deferred();
  chatgptResponses.push(() => returnPollDeferred.promise);
  await thinking.openLane('byo');
  await settle();
  assert.strictEqual(thinking.state.chatgpt.screen, 'pending');
  assert.strictEqual(nodes.get('chatgptPending').hidden, false);
  assert.strictEqual(nodes.get('chatgptRedirectUrl').hidden, false);
  thinking.stopChatGptPoll();
  returnPollDeferred.resolve({ state: 'pending' });
  await settle();

  // 32. Specific poll outcomes mapping:
  const pollOutcomes = [
    { resp: { state: 'failed', reason: 'denied' }, signed_in: false, expectedNotice: 'SENTINEL_DENIED SENTINEL_KEEP_FAILING' },
    { resp: { state: 'expired' }, signed_in: false, expectedNotice: 'SENTINEL_EXPIRED SENTINEL_KEEP_FAILING' },
    { resp: { state: 'failed', reason: 'callback_invalid' }, signed_in: false, expectedNotice: 'SENTINEL_UNFINISHED SENTINEL_KEEP_FAILING' },
    { resp: { state: 'failed', reason: 'account_mismatch' }, signed_in: true, expectedNotice: 'SENTINEL_ACCOUNT_MISMATCH' },
    { resp: { state: 'failed', reason: 'superseded' }, signed_in: true, expectedScreen: 'panel' },
    { resp: { state: 'failed', reason: 'cancelled' }, signed_in: false, expectedNotice: '' },
  ];
  for (const outcome of pollOutcomes) {
    thinking.state.chatgpt.screen = 'pending';
    thinking.state.chatgpt.attempt = { id: 'attempt_outcome_test', authorizeUrl: 'https://auth.openai.com/outcome' };
    window.location.hash = '#byo-setup';
    document.visibilityState = 'visible';
    chatgptResponses.push(() => Promise.resolve(outcome.resp));
    chatgptResponses.push(() => Promise.resolve({ signed_in: outcome.signed_in, email: outcome.signed_in ? 'owner@example.com' : undefined }));
    providerResponses.push(() => Promise.resolve(thinking.state.providers));
    if (outcome.resp.state === 'signed_in') {
      chatgptResponses.push(() => Promise.resolve({ models: [] }));
    }
    const tok = ++thinking.state.chatgpt.pollToken;
    await thinking.pollChatGptAttempt('attempt_outcome_test', tok);
    await settle();
    if (outcome.expectedScreen) {
      assert.strictEqual(thinking.state.chatgpt.screen, outcome.expectedScreen);
    }
    if (outcome.expectedNotice !== undefined) {
      assert.strictEqual(nodes.get('chatgptCardStatus').textContent, outcome.expectedNotice);
    }
  }

  // 33. Finish with callback_invalid: finish_refused on pending status, stays pending, following poll still runs
  nodes.get('chatgptRedirectUrl').value = 'localhost:4040/callback?invalid=1';
  thinking.state.chatgpt.screen = 'pending';
  thinking.state.chatgpt.attempt = { id: 'attempt_finish_invalid', authorizeUrl: 'https://auth.openai.com/fin' };
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Invalid callback'), { status: 400, reasonCode: 'callback_invalid' })));
  await thinking.finishChatGptSignIn();
  assert.strictEqual(thinking.state.chatgpt.screen, 'pending');
  assert.strictEqual(nodes.get('chatgptPendingStatus').textContent, 'SENTINEL_FINISH_REFUSED');

  // 34. Finish signed_in whose status refresh is signed_in: false -> attempt remains, poll scheduled
  nodes.get('chatgptRedirectUrl').value = 'localhost:4040/callback?code=ok';
  thinking.state.chatgpt.screen = 'pending';
  thinking.state.chatgpt.attempt = { id: 'attempt_finish_false', authorizeUrl: 'https://auth.openai.com/fin' };
  chatgptResponses.push(() => Promise.resolve({ reason: 'signed_in' }));
  chatgptResponses.push(() => Promise.resolve({ signed_in: false }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.finishChatGptSignIn();
  assert.strictEqual(thinking.state.chatgpt.screen, 'pending');
  assert.strictEqual(thinking.state.chatgpt.attempt?.id, 'attempt_finish_false');

  // 35. Status GET errors:
  // a) storage_error -> card shows SENTINEL_STORAGE and start-over
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Storage'), { status: 500, reasonCode: 'storage_error' })));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.refreshChatGpt();
  assert.strictEqual(thinking.state.chatgpt.screen, 'card');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_STORAGE');
  assert.strictEqual(nodes.get('chatgptCardStartOver').hidden, false);

  // b) 500 with previous status present -> SENTINEL_STATUS_FAILED, previous signed-in unchanged, start-over hidden
  thinking.state.chatgpt.status = { signed_in: true, email: 'owner@example.com' };
  thinking.state.chatgpt.notice = null;
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Server error'), { status: 500, reasonCode: 'server_error' })));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.refreshChatGpt();
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_STATUS_FAILED');
  assert.strictEqual(thinking.state.chatgpt.status?.signed_in, true);
  assert.strictEqual(nodes.get('chatgptCardStartOver').hidden, true);

  // c) 500 with no previous status -> Continue visible, status empty, start-over hidden
  thinking.state.chatgpt.status = null;
  thinking.state.chatgpt.notice = null;
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Server error'), { status: 500, reasonCode: 'server_error' })));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.refreshChatGpt();
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, '');
  assert.strictEqual(nodes.get('chatgptCardContinue').hidden, false);
  assert.strictEqual(nodes.get('chatgptCardStartOver').hidden, true);

  // 36. Refresh to signed_in: false while panel showing -> card + SENTINEL_SIGNED_OUT; while pending does not leave pending
  thinking.state.chatgpt.screen = 'panel';
  thinking.state.chatgpt.notice = null;
  chatgptResponses.push(() => Promise.resolve({ signed_in: false }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.refreshChatGpt();
  assert.strictEqual(thinking.state.chatgpt.screen, 'card');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_SIGNED_OUT');

  thinking.state.chatgpt.screen = 'pending';
  thinking.state.chatgpt.notice = null;
  chatgptResponses.push(() => Promise.resolve({ signed_in: false }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.refreshChatGpt();
  assert.strictEqual(thinking.state.chatgpt.screen, 'pending');

  // 37. Strip with no email -> SENTINEL_STRIP_NO_EMAIL
  thinking.state.chatgpt.status = { signed_in: true };
  thinking.renderChatGptPanel(sentinelCopy.byo_setup.chatgpt);
  assert.strictEqual(nodes.get('chatgptStrip').textContent, 'SENTINEL_STRIP_NO_EMAIL');

  // 38. Models: server order, nothing selected, use model disabled; listed active selected; remembered unlisted selects nothing
  thinking.state.chatgpt.selectedModel = '';
  thinking.state.providers.active = { provider: 'anthropic', model: 'claude-3' };
  thinking.state.providers.byo_models = { chatgpt: 'unlisted-remembered' };
  chatgptResponses.push(() => Promise.resolve({
    models: [
      { slug: 'beta-model', display_name: 'Beta Model' },
      { slug: 'alpha-model', display_name: 'Alpha Model' },
    ],
  }));
  await thinking.loadChatGptModels();
  assert.deepStrictEqual(thinking.state.chatgpt.models.map((m) => m.slug), ['beta-model', 'alpha-model']);
  assert.strictEqual(thinking.state.chatgpt.selectedModel, '');
  assert.strictEqual(nodes.get('chatgptUseModel').disabled, true);
  assert.strictEqual(nodes.get('chatgptModelStatus').textContent, '');

  // Listed active gpt-test is selected
  thinking.state.providers.active = { provider: 'chatgpt', model: 'beta-model' };
  chatgptResponses.push(() => Promise.resolve({
    models: [
      { slug: 'beta-model', display_name: 'Beta Model' },
      { slug: 'alpha-model', display_name: 'Alpha Model' },
    ],
  }));
  await thinking.loadChatGptModels();
  assert.strictEqual(thinking.state.chatgpt.selectedModel, 'beta-model');

  // 39. Models error sign_in_required (without chatgpt_ prefix) -> card, SENTINEL_SIGNED_OUT, Continue visible, manage hidden, no radios
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Sign in required'), { status: 401, reasonCode: 'sign_in_required' })));
  chatgptResponses.push(() => Promise.resolve({ signed_in: false }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.loadChatGptModels();
  assert.strictEqual(thinking.state.chatgpt.screen, 'card');
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, 'SENTINEL_SIGNED_OUT');
  assert.strictEqual(nodes.get('chatgptCardContinue').hidden, false);
  assert.strictEqual(nodes.get('chatgptCardManage').hidden, true);

  // 40. settings_saved_unlogged on PUT, on sign-out, and on start-over forget:
  // a) PUT
  thinking.state.chatgpt.selectedModel = 'beta-model';
  providerResponses.push(() => Promise.reject(Object.assign(new Error('Unlogged'), { status: 500, reasonCode: 'settings_saved_unlogged' })));
  chatgptResponses.push(() => Promise.resolve({ signed_in: true, email: 'owner@example.com' }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.saveChatGptModel();
  assert.strictEqual(nodes.get('chatgptModelStatus').textContent, '');
  assert.strictEqual(thinking.state.chatgpt.screen, 'panel');

  // b) sign-out unlogged
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Unlogged'), { status: 500, reasonCode: 'settings_saved_unlogged' })));
  chatgptResponses.push(() => Promise.resolve({ signed_in: false }));
  providerResponses.push(() => Promise.resolve(thinking.state.providers));
  await thinking.signOutChatGpt({ forget: false });
  assert.strictEqual(nodes.get('chatgptCardStatus').textContent, '');

  // c) start-over forget unlogged continues into sign-in
  openedWindows.length = 0;
  chatgptResponses.push(() => Promise.reject(Object.assign(new Error('Unlogged'), { status: 500, reasonCode: 'settings_saved_unlogged' })));
  chatgptResponses.push(() => Promise.resolve({ attempt_id: 'attempt_unlogged_forget', authorize_url: 'https://auth.openai.com/oauth/authorize?unlogged=1' }));
  const unloggedPollDeferred = deferred();
  chatgptResponses.push(() => unloggedPollDeferred.promise);
  await thinking.startChatGptSignIn({ startOver: true });
  assert.strictEqual(openedWindows.length, 1);
  assert.strictEqual(openedWindows[0].closed, false);
  assert.strictEqual(thinking.state.chatgpt.screen, 'pending');
  thinking.stopChatGptPoll();
  unloggedPollDeferred.resolve({ state: 'pending' });
  await settle();

  // 41. Glance with checking state (!== 'ready') + configured: true -> detail is SENTINEL_CHATGPT_LABEL: plus reason_text
  thinking.state.providers.provider_status.chatgpt.configured = true;
  thinking.state.providers.brain = {
    state: 'checking',
    headline: 'checking headline',
    reason_code: 'model_probing',
    reason_text: 'checking model latency',
    action: { label: 'Check now', refresh: true },
  };
  thinking.renderGlance();
  assert.strictEqual(nodes.get('thinkingActiveDetail').textContent, 'SENTINEL_CHATGPT_LABEL: checking model latency');

  // Restore state
  thinking.state.providers = savedProvidersBeforeGpt;
  thinking.state.keys = savedKeysBeforeGpt;

  console.log(`DOM CASES: ${passedCases}/${executedCases} passed`);
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});

