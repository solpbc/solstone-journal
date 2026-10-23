// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

let cases = 0;
function check(condition, message) {
  cases += 1;
  assert.ok(condition, message);
}

class ClassList {
  constructor() {
    this.values = new Set();
  }

  add(...names) {
    names.forEach((name) => this.values.add(name));
  }

  remove(...names) {
    names.forEach((name) => this.values.delete(name));
  }

  contains(name) {
    return this.values.has(name);
  }
}

class Style {
  constructor() {
    this.values = new Map();
  }

  setProperty(name, value) {
    this.values.set(name, value);
  }

  removeProperty(name) {
    this.values.delete(name);
  }

  getPropertyValue(name) {
    return this.values.get(name) || '';
  }
}

class Element {
  constructor(document, id = '') {
    this.ownerDocument = document;
    this.id = id;
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.attributes = {};
    this.classList = new ClassList();
    this.style = new Style();
    this.hidden = false;
    this.tabIndex = 0;
    this.textContent = '';
    this.className = '';
    this.type = '';
  }

  get childNodes() {
    return this.children;
  }

  append(...children) {
    children.forEach((child) => this.appendChild(child));
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  remove() {
    if (!this.parentElement) return;
    this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
    this.parentElement = null;
  }

  contains(node) {
    return node === this || this.children.some((child) => child.contains(node));
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }

  getAttribute(name) {
    return Object.hasOwn(this.attributes, name) ? this.attributes[name] : null;
  }

  addEventListener(name, listener) {
    (this.listeners[name] ||= []).push(listener);
  }

  emit(name, event = {}) {
    for (const listener of this.listeners[name] || []) {
      listener({target: this, preventDefault() {}, stopPropagation() {}, ...event});
    }
  }

  focus() {
    this.ownerDocument.activeElement = this;
  }

  matches(selector) {
    if (selector.startsWith('#')) return this.id === selector.slice(1);
    if (selector === '[role="option"]') return this.attributes.role === 'option';
    return false;
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }

  querySelectorAll(selector) {
    const matches = [];
    const visit = (node) => {
      node.children.forEach((child) => {
        if (child.matches(selector)) matches.push(child);
        visit(child);
      });
    };
    visit(this);
    return matches;
  }
}

function setLocation(window, value) {
  const [beforeHash, hash = ''] = String(value).split('#', 2);
  const [pathname, search = ''] = beforeHash.split('?', 2);
  window.location.pathname = pathname || '/app/entities';
  window.location.search = search ? `?${search}` : '';
  window.location.hash = hash ? `#${hash}` : '';
}

function findById(root, id) {
  if (root.id === id) return root;
  for (const child of root.children) {
    const found = findById(child, id);
    if (found) return found;
  }
  return null;
}

async function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) throw new Error('manifest directory required');
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/entities/workspace.html'), 'utf8');
  check(!workspace.includes('window.selectedFacet'), 'Entities does not read the shared selected facet');
  check(!workspace.includes('window.facetsData'), 'Entities does not read the shared facet roster');
  check(!workspace.includes('facet.switch'), 'Entities does not register the retired facet event');

  const scriptStart = workspace.lastIndexOf('<script>') + '<script>'.length;
  const scriptEnd = workspace.lastIndexOf('</script>');
  new vm.Script(workspace.slice(scriptStart, scriptEnd), {filename: 'entities-workspace.js'});
  check(scriptStart >= '<script>'.length && scriptEnd > scriptStart, 'the complete Entities workspace script parses');

  // Every entities refusal answers `error: "Entity request refused"`; the owner's reason is in `detail`.
  const errorStart = workspace.indexOf('function entityTrustFormat(');
  const errorEnd = workspace.indexOf('function entityTrustIdentitySummary', errorStart);
  check(errorStart !== -1 && errorEnd !== -1, 'workspace exposes the entities error-message helper');
  const errorContext = {
    window: {SurfaceState: {serverMessageFrom: (err) => err?.serverMessage || ''}},
    ENT_COPY: {},
  };
  vm.runInNewContext(
    `${workspace.slice(errorStart, errorEnd)}\nwindow.entityErrorMessage = entityErrorMessage;`,
    errorContext,
    {filename: 'entities-error-message.js'},
  );
  const entityErrorMessage = errorContext.window.entityErrorMessage;
  const refused = {status: 400, serverMessage: 'Entity request refused', rawDetail: "can't merge a person into something that isn't a person."};
  check(entityErrorMessage(refused, 'fallback') === refused.rawDetail, 'a refusal shows its detail, not the generic error');
  check(entityErrorMessage({serverMessage: 'Not Found'}, 'fallback') === 'Not Found', 'an error without detail shows its server message');
  check(entityErrorMessage(new TypeError('Failed to fetch'), 'fallback') === 'fallback', 'an error with neither shows the fallback');
  check(
    entityErrorMessage({status: 500, serverMessage: 'Entity request refused', rawDetail: 'No such file or directory (os error 2)'}, 'fallback') === 'fallback',
    'a server failure shows the fallback, not its engineering detail',
  );
  // Failed is also returned after a successful rollback, so its partial-state
  // envelope is not a reason to disable the recovery-aware retry path.
  const mergeFailure = {
    status: 500,
    serverMessage: 'Entity request refused',
    rawDetail: 'entity merge failed during audit: injected failure',
    payload: {
      error: 'Entity request refused', reason_code: 'entity_operation_failed',
      detail: 'entity merge failed during audit: injected failure',
      operation_state: 'partially_applied', mutation_applied: true,
      failed_phase: 'audit', source_id: 'source', target_id: 'target',
      source_state: {id: 'source'}, target_state: {id: 'target'},
      safe_remediation: 'Contact an operator to repair this merge.',
    },
  };
  const mergeStart = workspace.indexOf('async function commitJournalEntityMerge(');
  const mergeEnd = workspace.indexOf('function showJournalMergeDone(', mergeStart);
  let calls = 0;
  let shownError;
  const mergeContext = {
    ...errorContext,
    currentDetailEntity: {name: 'source'},
    ENT_COPY: {ENT_TRUST_ACTION_FAILED: 'failure fallback'},
    window: {
      ...errorContext.window,
      confirm: () => true,
      apiJson: async () => { calls += 1; throw mergeFailure; },
    },
    showInlineError: (_id, message) => { shownError = message; },
  };
  vm.runInNewContext(workspace.slice(errorStart, errorEnd) + workspace.slice(mergeStart, mergeEnd), mergeContext);
  const mergeButton = {disabled: false};
  await mergeContext.commitJournalEntityMerge('source', 'target', 'target', mergeButton);
  check(!mergeButton.disabled, 'a failed entity merge leaves its confirm button retryable');
  check(shownError === mergeContext.ENT_COPY.ENT_TRUST_ACTION_FAILED, 'a failed merge shows the fallback rather than diagnostics or operator advice');
  await mergeContext.commitJournalEntityMerge('source', 'target', 'target', mergeButton);
  check(calls === 2 && !mergeButton.disabled, 'a repeat confirmation reaches the recovery-aware merge route');

  const curation = fs.readFileSync(path.join(manifestDir, '../solstone-core-facets-web/assets/curation/workspace.html'), 'utf8');
  const curationStart = curation.indexOf('  function repairRequired(');
  const curationEnd = curation.indexOf('  function postJson(', curationStart);
  const curationContext = {
    window: {SurfaceState: {serverMessageFrom: (err) => err.serverMessage}},
    CUR_COPY: {CUR_REPAIR_REQUIRED: '{detail} {remediation}'},
    copyFormat: (text, values) => text.replace(/\{(\w+)\}/g, (_, key) => values[key]),
    setBusy: (control, busy) => { control.disabled = busy; },
    showError: (_item, message) => { shownError = message; },
  };
  vm.runInNewContext(curation.slice(curationStart, curationEnd), curationContext);
  const curationButton = {disabled: true};
  const siblingControl = {disabled: false};
  const item = {querySelectorAll: () => [curationButton, siblingControl]};
  // Curation returns an ordinary per-item error for EntityMergeError::Failed.
  curationContext.showMutationError(item, curationButton, {
    serverMessage: mergeFailure.rawDetail,
    payload: {status: 'error', kind: 'entity_merge', error: mergeFailure.rawDetail},
  }, 'fallback');
  check(!curationButton.disabled && !siblingControl.disabled, 'curation also permits a repeat entity merge');
  curationContext.showMutationError(item, curationButton, {
    payload: {operation_state: 'repair_required', safe_remediation: 'speaker remedy'},
  }, 'fallback');
  check(curationButton.disabled && siblingControl.disabled, 'speaker repair still disables curation mutations');

  const helperBody = workspace.slice(errorStart, errorEnd);
  const outsideHelper = workspace.replace(helperBody, '');
  check(!outsideHelper.includes('SurfaceState.serverMessageFrom('), 'every entities error surface reads through entityErrorMessage');
  check(!/err(or)?\?\.serverMessage/.test(outsideHelper), 'no entities error surface reads the generic server message directly');
  check(!/new Error\((data|body)\.error/.test(workspace), 'raw-fetch failures prefer the refusal detail');

  const start = workspace.indexOf('let ENT_COPY = {};');
  const scopeEnd = workspace.indexOf('function showFacetDetailView', start);
  const metadataStart = workspace.indexOf('function adoptJournalSummary', start);
  const metadataEnd = workspace.indexOf('// Edit form submission', metadataStart);
  check(start !== -1 && scopeEnd !== -1 && metadataStart !== -1 && metadataEnd !== -1, 'workspace exposes scope implementation boundaries');

  const documentListeners = {};
  const document = {
    activeElement: null,
    body: null,
    createElement() { return new Element(document); },
    getElementById(id) { return findById(document.body, id); },
    querySelector(selector) { return document.body.querySelector(selector); },
    addEventListener(name, listener) { (documentListeners[name] ||= []).push(listener); },
    removeEventListener(name, listener) {
      documentListeners[name] = (documentListeners[name] || []).filter((candidate) => candidate !== listener);
    },
  };
  document.body = new Element(document, 'body');
  const appendRoot = (id) => {
    const node = new Element(document, id);
    document.body.appendChild(node);
    return node;
  };
  appendRoot('entities-scope-anchor');
  appendRoot('entities-list-region');
  appendRoot('journal-entities-view');
  appendRoot('journal-entity-detail-view');
  appendRoot('entities-list-view');
  appendRoot('entity-detail-view');
  const emptyView = appendRoot('facet-entities-empty-view');
  emptyView.append(
    new Element(document, 'facet-entities-empty-title'),
    new Element(document, 'facet-entities-empty-body'),
    new Element(document, 'facet-entities-empty-action'),
  );

  const windowListeners = {};
  const window = {
    location: {pathname: '/app/entities', search: '', hash: ''},
    history: {
      pushed: [],
      replaced: [],
      pushState(_state, _title, value) {
        this.pushed.push(value);
        setLocation(window, value);
      },
      replaceState(_state, _title, value) {
        this.replaced.push(value);
        setLocation(window, value);
      },
    },
    addEventListener(name, listener) { (windowListeners[name] ||= []).push(listener); },
    emit(name) { (windowListeners[name] || []).forEach((listener) => listener()); },
    logError() {},
    SurfaceState: {replaceLoading() {}, error() {}},
    CONVEY_COPY: {RELOAD_HINT: ''},
  };
  window.window = window;
  const context = {
    window,
    document,
    URLSearchParams,
    history: window.history,
    location: window.location,
    console,
  };
  const scopeSource = `${workspace.slice(start, scopeEnd)}\nfunction loadEntities() { window.__scopeLoads = (window.__scopeLoads || 0) + 1; showListView(); }`;
  vm.runInNewContext(scopeSource, context, {filename: 'entities-scope.js'});
  // The list-state key builder lives in the paged-list region, outside both slices; the scope
  // harness only needs adoptJournalSummary to record a key, not to compute one.
  context.journalListStateKey = () => 'harness';
  vm.runInNewContext(workspace.slice(metadataStart, metadataEnd), context, {filename: 'entities-metadata.js'});
  vm.runInNewContext(`
    window.__entities = {
      setRoster(value) { facetRoster = value; },
      setJournal(summary) { adoptJournalSummary(summary); },
      restore: restoreEntityScopeFromLocation,
      render: renderScopeControl,
      showList: showListView,
      currentFacet: () => currentFacet,
      count: facetCount,
    };
  `, context, {filename: 'entities-exports.js'});
  const entities = window.__entities;
  const copy = {
    ENT_SCOPE_SHOWING: 'showing',
    ENT_SCOPE_WHOLE_JOURNAL: 'your whole journal',
    ENT_SCOPE_EMPTY_TITLE: 'nothing in {facet} yet.',
    ENT_SCOPE_EMPTY_BODY: 'entities show up here as your journal holds people, places and projects in this facet.',
    ENT_SCOPE_EMPTY_ACTION: 'show your whole journal',
    ENT_SCOPE_FACET_MISSING: "that facet isn't in your journal any more, so this is your whole journal.",
  };
  vm.runInNewContext(`ENT_COPY = ${JSON.stringify(copy)};`, context, {filename: 'entities-copy.js'});

  entities.setRoster([
    {name: 'work', title: 'Work', color: '#2f6fdd'},
    {name: 'personal', title: 'Personal', color: '#a23f7b'},
  ]);
  // Counts come from the paged summary read; the route excludes blocked and
  // detached memberships server-side (see solstone-core-entities router tests).
  entities.setJournal({facet_counts: {work: 1}});
  entities.restore();
  const control = document.getElementById('entities-scope-control');
  check(control !== null, 'scope control is present when the roster is non-empty');
  const options = control.querySelectorAll('[role="option"]');
  check(options.length === 3, 'scope menu includes whole journal and every roster facet');
  check(
    JSON.stringify(options.map((option) => [option.children[0].textContent, option.children[1]?.textContent || '']))
      === JSON.stringify([['your whole journal', ''], ['Work', '1'], ['Personal', '0']]),
    'scope control shows counts from the journal summary and includes zero-entity facets',
  );

  setLocation(window, '/app/entities?facet=work');
  entities.restore();
  entities.showList();
  check(entities.currentFacet() === 'work' && document.getElementById('entities-list-view').style.display === 'block', 'facet query selects the facet-scoped list');
  check(document.getElementById('entities-list-region').style.getPropertyValue('--entities-facet-color') === '#2f6fdd', 'facet tint stays on the local list region');

  setLocation(window, '/app/entities');
  entities.restore();
  entities.showList();
  check(entities.currentFacet() === null && document.getElementById('journal-entities-view').style.display === 'block', 'plain Entities URL selects the journal-wide list');

  setLocation(window, '/app/entities?facet=personal');
  entities.restore();
  entities.showList();
  check(document.getElementById('facet-entities-empty-view').style.display === 'block', 'zero-entity facets use the page-level empty state');
  check(
    document.getElementById('facet-entities-empty-title').textContent === 'nothing in Personal yet.'
      && document.getElementById('facet-entities-empty-body').textContent === copy.ENT_SCOPE_EMPTY_BODY
      && document.getElementById('facet-entities-empty-action').textContent === copy.ENT_SCOPE_EMPTY_ACTION,
    'page-level empty state uses its own scoped copy',
  );

  setLocation(window, '/app/entities?facet=work');
  entities.restore();
  const journalOption = document.getElementById('entities-scope-control').querySelectorAll('[role="option"]')[0];
  journalOption.emit('click');
  check(window.history.pushed.at(-1) === '/app/entities' && window.location.search === '', 'scope selection pushes the journal-wide query state');
  setLocation(window, '/app/entities?facet=work');
  window.emit('popstate');
  check(entities.currentFacet() === 'work' && document.getElementById('entities-list-view').style.display === 'block', 'popstate restores the prior scoped list');
  check((window.__scopeLoads || 0) >= 2, 'scope selections and popstate rerender through the page loader');

  setLocation(window, '/app/entities?facet=missing');
  entities.restore();
  check(window.location.search === '' && entities.currentFacet() === null, 'unknown facets canonicalize to the journal-wide URL');
  check(
    document.getElementById('entities-scope-notice').textContent === copy.ENT_SCOPE_FACET_MISSING,
    'unknown facets show the stale-facet notice',
  );

  entities.setRoster([]);
  entities.setJournal({facet_counts: {}});
  entities.render();
  check(document.getElementById('entities-scope-control') === null, 'scope control and popover are removed when the roster is empty');

  const parseStart = workspace.indexOf('function parseYYYYMMDD(');
  const parseEnd = workspace.indexOf('function formatDateShort(', parseStart);
  const connMapStart = workspace.indexOf('function entityConnCopyMap(');
  const connMapEnd = workspace.indexOf('async function fetchEntityConnJson(', connMapStart);
  const netStart = workspace.indexOf('function renderConnectionsNetwork(');
  const netEnd = workspace.indexOf('async function toggleConnectionEvidence(', netStart);
  check(parseStart !== -1 && parseEnd !== -1 && connMapStart !== -1 && connMapEnd !== -1 && netStart !== -1 && netEnd !== -1, 'connections workspace slices exist');

  const connDoc = {
    createElement() { return new Element(connDoc); },
  };
  const connContext = {
    document: connDoc,
    console,
    ENT_COPY: {
      ENT_CONN_SUMMARY: 'ENT_CONN_SUMMARY:{s}:{a}',
      ENT_CONN_SUMMARY_CALENDAR: 'ENT_CONN_SUMMARY_CALENDAR:{c}',
      ENT_CONN_ROW_META_SCHEDULED: 'ENT_CONN_ROW_META_SCHEDULED:{day}',
      ENT_CONN_CALENDAR_ONLY: 'ENT_CONN_CALENDAR_ONLY',
      ENT_CONN_EVENTS_ONLY: 'ENT_CONN_EVENTS_ONLY',
      ENT_CONN_ROW_META_ATTENDANCE: 'ENT_CONN_ROW_META_ATTENDANCE:{n}:{day}',
      ENT_CONN_ROW_META_ONE_ATTENDANCE: 'ENT_CONN_ROW_META_ONE_ATTENDANCE:{day}',
      ENT_CONN_ROW_META: 'ENT_CONN_ROW_META:{n}:{day}',
      ENT_CONN_ROW_META_ONE: 'ENT_CONN_ROW_META_ONE:{day}',
      ENT_CONN_WITH_YOU_SUMMARY: 'ENT_CONN_WITH_YOU_SUMMARY:{n}:{kind}:{day}',
      ENT_CONN_WITH_YOU_SUMMARY_ONE: 'ENT_CONN_WITH_YOU_SUMMARY_ONE:{kind}:{day}',
      ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED: 'ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:{day}',
      ENT_CONN_WITH_YOU_TOGGLE: 'ENT_CONN_WITH_YOU_TOGGLE',
    },
    ATTENDANCE_KINDS: new Set(['attended-with', 'co-present', 'scheduled-with']),
  };
  const connSource = [
    workspace.slice(parseStart, parseEnd),
    workspace.slice(connMapStart, connMapEnd),
    workspace.slice(netStart, netEnd),
  ].join('\n');
  vm.runInNewContext(connSource, connContext, { filename: 'entities-connections.js' });

  function allDescendants(node) {
    const list = [node];
    for (const child of node.children) {
      list.push(...allDescendants(child));
    }
    return list;
  }

  const connContainer = new Element(connDoc, 'connections-container');
  const testNeighbors = [
    {entity_id:'sem', name:'Ada', evidence_class:'semantic', count:2, last_seen:'20260501', kinds:{'works-with':{weighted:1}}},
    {entity_id:'att', name:'Bea', evidence_class:'attendance', count:3, last_seen:'20260502', kinds:{'attended-with':{weighted:1}}},
    {entity_id:'sch', name:'Cal', evidence_class:'scheduled', count:4, last_seen:'20260503', kinds:{'scheduled-with':{weighted:1}}},
  ];
  connContext.renderConnectionsNetwork(connContainer, 'self', testNeighbors, '20260530', true);

  const connRows = allDescendants(connContainer).filter(node => node.className === 'entity-conn-row');
  const calRow = connRows.find(r => allDescendants(r).some(n => n.className === 'entity-conn-name' && n.textContent === 'Cal'));
  check(calRow !== undefined, 'found scheduled row for Cal');
  const calDescendants = allDescendants(calRow);
  const calText = calDescendants.map(n => n.textContent).join(' ');

  check(
    calDescendants.some(n => n.textContent.includes('ENT_CONN_ROW_META_SCHEDULED')),
    'scheduled row contains ENT_CONN_ROW_META_SCHEDULED',
  );
  check(
    calDescendants.some(n => n.textContent === 'ENT_CONN_CALENDAR_ONLY' && n.className === 'entity-conn-chip entity-conn-chip-event-only'),
    'scheduled row has calendar-only chip with expected classes',
  );
  check(
    !calText.includes('ENT_CONN_EVENTS_ONLY')
      && !calText.includes('ENT_CONN_ROW_META_ATTENDANCE')
      && !calText.includes('ENT_CONN_ROW_META_ONE_ATTENDANCE'),
    'scheduled row does not contain attendance copy keys',
  );

  const summary = allDescendants(connContainer).find(n => n.className === 'entity-conn-summary');
  check(
    summary && summary.textContent === 'ENT_CONN_SUMMARY:1:1 · ENT_CONN_SUMMARY_CALENDAR:1',
    'summary includes direct, attendance, and scheduled counts',
  );

  const connContainer2 = new Element(connDoc, 'connections-container-2');
  connContext.renderConnectionsNetwork(connContainer2, 'self', [
    {entity_id:'sem', name:'Ada', evidence_class:'semantic', count:2, last_seen:'20260501', kinds:{'works-with':{weighted:1}}},
    {entity_id:'att', name:'Bea', evidence_class:'attendance', count:3, last_seen:'20260502', kinds:{'attended-with':{weighted:1}}},
  ], '20260530', true);
  const summary2 = allDescendants(connContainer2).find(n => n.className === 'entity-conn-summary');
  check(
    summary2 && summary2.textContent === 'ENT_CONN_SUMMARY:1:1' && !summary2.textContent.includes('ENT_CONN_SUMMARY_CALENDAR'),
    'summary without scheduled neighbours omits calendar clause',
  );

  function checkWithYou(history, label) {
    const container = new Element(connDoc);
    const rendered = connContext.renderWithYouBlock(container, 'alice', history, '20260530');
    check(rendered, `${label} rendered with-you block`);
    const summaryNode = allDescendants(container).find(n => n.className === 'entity-conn-with-you-summary');
    const text = summaryNode ? summaryNode.textContent : '';
    const toggleNode = allDescendants(container).find(n => n.className === 'entity-conn-with-you-toggle');
    const paneNode = allDescendants(container).find(n => n.className === 'entity-conn-evidence');
    return { container, text, toggleNode, paneNode };
  }

  const resScheduled1 = checkWithYou(
    { evidence_class: 'scheduled', total: 1, evidence: [{ kind: 'scheduled-with', day: '20260501' }] },
    'scheduled single',
  );
  check(
    resScheduled1.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:')
      && !resScheduled1.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
      && !resScheduled1.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE'),
    'scheduled total 1 uses scheduled copy and neither one- nor many-moments copy',
  );

  const resScheduledMany = checkWithYou(
    {
      evidence_class: 'scheduled',
      total: 3,
      evidence: [
        { kind: 'scheduled-with', day: '20260501' },
        { kind: 'scheduled-with', day: '20260501' },
      ],
    },
    'scheduled multiple',
  );
  check(
    resScheduledMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:')
      && !resScheduledMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
      && !resScheduledMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE'),
    'scheduled total > 1 uses scheduled copy and neither one- nor many-moments copy',
  );

  const otherClasses = ['semantic', 'mixed', 'attendance', undefined];
  for (const cls of otherClasses) {
    const historyOne = { total: 1, evidence: [{ kind: 'works-with', day: '20260501' }] };
    if (cls !== undefined) historyOne.evidence_class = cls;
    const resOne = checkWithYou(historyOne, `${cls ?? 'undefined'} total 1`);
    check(
      resOne.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE')
        && !resOne.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
        && !resOne.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:'),
      `${cls ?? 'undefined'} total 1 uses one-moment copy and neither many nor scheduled copy`,
    );

    const historyMany = {
      total: 4,
      evidence: [
        { kind: 'works-with', day: '20260501' },
        { kind: 'works-with', day: '20260501' },
      ],
    };
    if (cls !== undefined) historyMany.evidence_class = cls;
    const resMany = checkWithYou(historyMany, `${cls ?? 'undefined'} total > 1`);
    check(
      resMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
        && !resMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE')
        && !resMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:'),
      `${cls ?? 'undefined'} total > 1 uses many-moments copy and neither one nor scheduled copy`,
    );
  }

  const resNull = checkWithYou(
    {
      evidence_class: null,
      total: 2,
      evidence: [
        { kind: 'works-with', day: '20260501' },
        { kind: 'works-with', day: '20260501' },
      ],
    },
    'null class multiple',
  );
  check(
    resNull.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
      && !resNull.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE')
      && !resNull.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:'),
    'null evidence_class total > 1 uses many-moments copy and not scheduled copy',
  );

  check(
    resScheduled1.toggleNode
      && resScheduled1.toggleNode.textContent === 'ENT_CONN_WITH_YOU_TOGGLE'
      && resScheduled1.toggleNode.getAttribute('aria-expanded') === 'false',
    'scheduled body toggle has expected text and aria-expanded false',
  );
  check(
    resScheduled1.paneNode
      && resScheduled1.paneNode.className === 'entity-conn-evidence'
      && resScheduled1.paneNode.hidden === true,
    'scheduled body pane has entity-conn-evidence class and is hidden',
  );

  const resSemantic = checkWithYou(
    { evidence_class: 'semantic', total: 1, evidence: [{ kind: 'mentioned', day: '20260501' }] },
    'semantic toggle check',
  );
  check(
    resSemantic.toggleNode
      && resSemantic.toggleNode.textContent === 'ENT_CONN_WITH_YOU_TOGGLE'
      && resSemantic.toggleNode.getAttribute('aria-expanded') === 'false',
    'semantic body toggle has expected text and aria-expanded false',
  );
  check(
    resSemantic.paneNode
      && resSemantic.paneNode.className === 'entity-conn-evidence'
      && resSemantic.paneNode.hidden === true,
    'semantic body pane has entity-conn-evidence class and is hidden',
  );

  // Tidy-up: bulk hide batches and resume, undo numbers, merge planning.
  const tidyStart = workspace.indexOf('// Tidy-up:');
  const tidyEnd = workspace.indexOf('restoreTidyOp();', tidyStart);
  check(tidyStart !== -1 && tidyEnd !== -1, 'workspace exposes the tidy-up block');
  function tidyHarness(api, {stored = null} = {}) {
    const storage = new Map(stored ? [['entities.tidyOperation', stored]] : []);
    const log = {requests: [], sleeps: [], storedAtFirstRequest: null};
    const context = {
      console,
      Promise,
      Map,
      Set,
      Uint8Array,
      journalFolder: '',
      facetRoster: [{name: 'work', title: 'Work'}],
      groupedCount: (n) => String(n),
      renderJournalEntities: () => {},
      loadJournalEntities: async () => {},
      window: {
        logError: () => {},
        crypto: {getRandomValues: (bytes) => bytes.fill(7)},
        sessionStorage: {
          getItem: (key) => (storage.has(key) ? storage.get(key) : null),
          setItem: (key, value) => storage.set(key, value),
          removeItem: (key) => storage.delete(key),
        },
        apiJson: async (url, opts) => {
          const body = opts && opts.body ? JSON.parse(opts.body) : null;
          if (log.storedAtFirstRequest === null && body) log.storedAtFirstRequest = storage.get('entities.tidyOperation') || '';
          log.requests.push({url, body});
          return api(url, body, log.requests.length);
        },
      },
    };
    vm.runInNewContext(`${workspace.slice(tidyStart, tidyEnd)}\nthis.tidyClock = tidyClock; this.tidyState = tidyState;`, context, {filename: 'entities-tidy.js'});
    context.tidyClock.sleep = async (ms) => { log.sleeps.push(ms); };
    context.renderTidyToast = () => {};
    context.renderIdsPanel = () => {};
    context.renderDupsPanel = () => {};
    return {context, log, storage};
  }
  const noLoss = (url) => {
    if (url.includes('tidy-up')) return {id_shaped: {count: 0, members: [], items: []}, duplicates: {count: 0, members: [], items: []}, damage: {count: 0, paths: []}};
    return null;
  };

  {
    const members = Array.from({length: 118}, (_, i) => `id${i}`);
    const {context, log} = tidyHarness((url, body) => noLoss(url) || {hidden: body.ids, already_hidden: [], missing: [], unreadable: []});
    context.tidyState.captured = members;
    context.tidyState.unticked = new Set(['id3', 'id110']);
    context.startBulkHide();
    context.startBulkHide();
    await new Promise(resolve => setImmediate(resolve));
    for (let i = 0; i < 20 && context.tidyState.running; i += 1) await new Promise(resolve => setImmediate(resolve));
    const hides = log.requests.filter(r => r.url.endsWith('/journal/block'));
    check(JSON.stringify(hides.map(r => r.body.ids.length)) === '[50,50,16]', 'bulk hide sends the 116 ticked ids in batches of 50, 50, 16');
    check(new Set(hides.map(r => r.body.operation)).size === 1, 'every batch carries one operation id');
    check(!hides.some(r => r.body.ids.includes('id3') || r.body.ids.includes('id110')), 'unticked ids are never sent');
    check(log.sleeps.filter(ms => ms >= 100).length >= 2, 'batches after the first wait at least 100 ms');
    check(JSON.parse(log.storedAtFirstRequest).phase === 'hiding', 'the operation is stored before the first request');
    check(!log.requests.slice(0, 3).some(r => r.url.includes('tidy-up')), 'no groups read runs between the press and the last batch');
    check(context.tidyState.op.view === 'hide-done' && context.tidyState.op.hidden.size === 116, 'a finished hide counts the hidden ids');
  }

  const settle = async (context) => {
    for (let i = 0; i < 40 && (context.tidyState.running || context.tidyState.dupBusy); i += 1) await new Promise(resolve => setImmediate(resolve));
    await new Promise(resolve => setImmediate(resolve));
  };
  const refusal = {status: 409, reasonCode: 'entity_busy_refusal', serverMessage: 'no'};

  {
    let calls = 0;
    const {context, log} = tidyHarness((url) => {
      const read = noLoss(url); if (read) return read;
      calls += 1;
      if (calls === 1) throw new TypeError('Failed to fetch');
      throw refusal;
    });
    context.tidyState.captured = ['a', 'b'];
    context.startBulkHide();
    await settle(context);
    const ops = new Set(log.requests.filter(r => r.body).map(r => r.body.operation));
    check(context.tidyState.op.view === 'hide-stopped' && ops.size === 1, 'a lost first attempt then a refusal is stopped partway under the same operation');
  }

  {
    const {context} = tidyHarness((url) => { const read = noLoss(url); if (read) return read; throw refusal; });
    context.tidyState.captured = ['a'];
    context.startBulkHide();
    await settle(context);
    check(context.tidyState.op.view === 'hide-failed', 'a refusal on a first batch with no earlier attempt means nothing was hidden');
  }

  {
    let calls = 0;
    const {context} = tidyHarness((url, body) => {
      const read = noLoss(url); if (read) return read;
      calls += 1;
      if (calls === 1) throw new TypeError('Failed to fetch');
      return {hidden: body.ids.filter(id => id !== 'x' && id !== 'y' && id !== 'z'), already_hidden: ['x', 'y'], missing: ['z'], unreadable: []};
    });
    context.tidyState.captured = ['a', 'b', 'x', 'y', 'z'];
    context.startBulkHide();
    await settle(context);
    const content = context.tidyToastContent(context.tidyState.op);
    check(context.tidyState.op.hidden.size === 2 && context.tidyState.op.other.size === 3, 'hide numbers come from the responses, not the ticked count');
    check(content.text.includes('2') && content.text.includes('3'), 'the done toast states m and k');
  }

  {
    const {context} = tidyHarness(() => null, {stored: JSON.stringify({id: 'op12345678', selection: ['a', 'b'], phase: 'hiding', hidden: [], other: []})});
    context.restoreTidyOp();
    check(context.tidyState.op.view === 'hide-stopped' && context.tidyState.op.unanswered, 'a reload while hiding restores the stopped-partway state as unanswered');
  }

  {
    const responses = [
      {restored: [{id: 'a', facets_reattached: []}], skipped: ['facets/w/entities/q/entity.json'], failed: [{id: 'x', reason: 'r'}], remaining: 2, unreadable: []},
      {restored: [{id: 'b', facets_reattached: []}], skipped: ['facets/w/entities/q/entity.json'], failed: [{id: 'x', reason: 'r'}], remaining: 1, unreadable: []},
      {restored: [{id: 'c', facets_reattached: []}], skipped: [], failed: [{id: 'x', reason: 'r'}], remaining: 0, unreadable: []},
    ];
    let n = 0;
    const {context, log} = tidyHarness((url) => noLoss(url) || responses[n++]);
    context.tidyState.op = {id: 'op12345678', selection: ['a', 'b', 'c', 'x'], phase: 'done', hidden: new Set(['a', 'b', 'c', 'x']), other: new Set(), view: 'hide-done', undo: null};
    await context.runTidyUndo();
    const op = context.tidyState.op;
    const content = context.tidyToastContent(op);
    check(op.undo.total === 4 && op.undo.restored.size === 3 && op.undo.failed.size === 1, 'undo denominator is the first response total and failures dedupe');
    check(content.text.startsWith('put back 3 of 4.'), 'undo with a failure shows the put-back-of form');
    check(content.small && content.small.split('facets/w/entities/q/entity.json').length === 2, 'the facets line names a skipped path once');
    check(log.sleeps.filter(ms => ms >= 100).length >= 2, 'undo requests wait at least 100 ms');
  }

  {
    const {context} = tidyHarness((url) => noLoss(url) || {restored: [], skipped: [], failed: [{id: 'x', reason: 'r'}], remaining: 3, unreadable: []});
    context.tidyState.op = {id: 'op12345678', selection: ['x'], phase: 'done', hidden: new Set(['x']), other: new Set(), view: 'hide-done', undo: null};
    await context.runTidyUndo();
    check(context.tidyState.op.view === 'undo-stopped', 'undo with no progress stops instead of polling');
  }

  {
    const {context} = tidyHarness(() => null);
    const pair = (s, t, facets = ['work']) => ({pair: [s, t], names: [s, t], rows: facets.map(facet => ({facet, source_slug: s, target_slug: t}))});
    const members = [pair('A', 'B'), pair('C', 'B'), pair('A', 'C'), pair('D', 'E', ['work', 'home']), pair('F', 'A'), pair('B', 'G'), pair('G', 'H')];
    const selected = new Set(['A\u0000B', 'C\u0000B', 'A\u0000C', 'D\u0000E']);
    const plan = context.planDuplicateMerge(members, selected);
    check(JSON.stringify(plan.kept.map(m => m.pair.join(''))) === '["AB","CB","DE"]', 'the merge confirm keeps pairs that name no kept source');
    check(JSON.stringify(plan.leaving.map(m => m.pair.join(''))) === '["AC","FA"]', 'listed pairs naming a kept source leave the list, ticked or not');
  }

  {
    const pair = (i) => ({pair: [`s${i}`, `t${i}`], names: [`s${i}`, `t${i}`], rows: [{facet: 'work', source_slug: `s${i}`, target_slug: `t${i}`}]});
    const members = Array.from({length: 60}, (_, i) => pair(i));
    let batch = 0;
    const {context, log} = tidyHarness((url, body) => {
      if (url.includes('tidy-up')) return {id_shaped: {count: 0, members: []}, duplicates: {count: 40, members: members.slice(20), items: []}, damage: {count: 0, paths: []}};
      batch += 1;
      if (batch === 2) throw new TypeError('Failed to fetch');
      return {results: body.items.map(() => ({status: 'accepted'}))};
    });
    context.tidyState.data = {duplicates: {members}};
    await context.runDuplicateMerge({kept: members, leaving: []});
    const accepts = log.requests.filter(r => r.url.endsWith('accept-batch'));
    check(JSON.stringify(accepts.map(r => r.body.items.length)) === '[20,20]', 'merge sends 20 items per request and stops after a request with no response');
    check(context.tidyState.dupStatus === 'merged 20 of 60 pairs. 40 couldn\'t be merged.', 'pairs with no answer are decided by the re-read');
  }

  {
    let release;
    const gate = new Promise(resolve => { release = resolve; });
    const {context} = tidyHarness(async () => {
      await gate;
      return {id_shaped: {count: 1, actionable: true, members: ['code-1'], items: []}, duplicates: {count: 0, members: [], items: []}, damage: {count: 0, paths: []}};
    });
    const doc = {createElement: () => {
      const el = Object.assign(new Element(doc), {dataset: {}});
      const append = el.append.bind(el);
      el.append = (...nodes) => append(...nodes.filter(node => typeof node !== 'string'));
      return el;
    }};
    const host = new Element(doc, 'journal-detail-indicators');
    doc.getElementById = (id) => (id === 'journal-detail-indicators' ? host : null);
    context.document = doc;
    context.currentDetailEntity = null;
    context.loadTidyGroups();
    const shown = context.renderJournalDetailIdNote({id: 'code-1', entity_id: 'code-1'});
    release();
    await shown;
    check(host.children.some(node => node.className === 'journal-detail-id-note'), 'a detail opened while the groups read is in flight shows the ID line once the read lands');
  }

  console.log(`DOM CASES: ${cases} passed`);
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
