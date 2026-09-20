// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');

function scriptFromWorkspace() {
  const source = fs.readFileSync(path.join(crateDir, 'assets/workspace.html'), 'utf8');
  const scripts = [...source.matchAll(/<script>([\s\S]*?)<\/script>/g)];
  const workspaceScript = scripts.find(([, script]) => script.includes('function setupQuickImportForm('));
  assert.ok(workspaceScript, 'workspace submission script exists');
  return workspaceScript[1];
}

function balancedBlock(source, start) {
  const open = source.indexOf('{', start);
  assert.ok(open >= 0, 'block opening brace exists');
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === '{') depth += 1;
    if (source[index] === '}' && --depth === 0) return source.slice(open, index + 1);
  }
  throw new Error('unterminated block');
}

function functionSource(source, name) {
  const match = new RegExp(`(?:async\\s+)?function\\s+${name}\\(`).exec(source);
  const start = match ? match.index : -1;
  assert.ok(start >= 0, `${name} is defined in workspace code`);
  const parenClose = source.indexOf(')', start);
  const openBrace = source.indexOf('{', parenClose);
  return source.slice(start, openBrace) + balancedBlock(source, parenClose);
}

function arrowBody(source, marker) {
  const start = source.indexOf(marker);
  assert.ok(start >= 0, `${marker} is defined in workspace code`);
  return balancedBlock(source, start);
}

class Element {
  constructor(value = '') {
    this.value = value;
    this.textContent = '';
    this.innerHTML = '';
    this.disabled = false;
    this.style = {};
    this.listeners = {};
    this.classList = { add() {}, remove() {} };
    this.focused = false;
  }

  focus() {
    this.focused = true;
  }

  addEventListener(type, listener) {
    (this.listeners[type] ||= []).push(listener);
  }
}

class CapturedFormData {
  constructor() {
    this.entries = [];
  }

  append(key, value) {
    this.entries.push([key, value]);
  }
}

function makeHarness(ids) {
  const elements = Object.fromEntries(Object.entries(ids).map(([id, value]) => [id, new Element(value)]));
  const requests = [];
  const document = {
    getElementById(id) { return elements[id] || null; },
  };
  const fetch = async (url, options) => {
    requests.push({ url, options });
    return { ok: true, async json() { return {}; } };
  };
  const window = {
    _quickClientItemId: null,
    _quickSaved: null,
    _guidedClientItemId: null,
    showError() {},
  };
  window.window = window;
  const context = vm.createContext({
    window,
    document,
    FormData: CapturedFormData,
    crypto: { randomUUID: () => 'test-client-item' },
    fetch,
    Error,
    console,
  });
  vm.runInContext([
    "const settingInput = document.getElementById('settingInput');",
    "const startBtn = document.getElementById('startBtn');",
    'let currentFile = null;',
    'let currentGuidedSaved = null;',
    'let importEvents = {};',
    'const isTerminalDuplicate = () => false;',
    'const trackPendingImport = () => {};',
    'const loadImports = () => {};',
    'const navigateTo = () => {};',
    'const closeDetectModal = () => {};',
    'const showDetect = () => {};',
  ].join('\n'), context);
  return { context, elements, requests };
}

function formObject(formData) {
  return Object.fromEntries(formData.entries);
}

function assertFacetAbsentAndSettingPreserved(request, setting) {
  const payload = request.options.body instanceof CapturedFormData
    ? formObject(request.options.body)
    : JSON.parse(request.options.body);
  assert.strictEqual(payload.setting, setting, 'setting survives submission');
  assert.ok(!Object.prototype.hasOwnProperty.call(payload, 'facet'), 'submission omits facet');
}

const workspace = scriptFromWorkspace();
let cases = 0;

async function runQuickSubmit() {
  const harness = makeHarness({
    dropArea: '',
    fileInput: '',
    fileLabel: '',
    pasteText: 'quick text',
    importForm: '',
    quickSettingInput: 'quick setting',
    validateBtn: '',
  });
  vm.runInContext(functionSource(workspace, 'setupQuickImportForm'), harness.context);
  vm.runInContext('setupQuickImportForm()', harness.context);
  await harness.elements.importForm.onsubmit({ preventDefault() {} });
  assert.strictEqual(harness.requests.length, 1, 'quick submit posts once');
  assert.strictEqual(harness.requests[0].url, '/app/import/api/save');
  assertFacetAbsentAndSettingPreserved(harness.requests[0], 'quick setting');
  cases += 1;
}

async function runGuidedSubmit() {
  const harness = makeHarness({
    guidedStartBtn: '',
    guidedSettingInput: 'guided setting',
  });
  vm.runInContext(functionSource(workspace, 'startGuidedImport'), harness.context);
  vm.runInContext("currentGuidedSaved = { path: '/saved', timestamp: '1700000000' };", harness.context);
  await harness.context.startGuidedImport({ name: 'notes', display_name: 'Notes' });
  assert.deepStrictEqual(harness.requests.map((request) => request.url), [
    '/app/import/api/meta',
    '/app/import/api/start',
  ]);
  assertFacetAbsentAndSettingPreserved(harness.requests[0], 'guided setting');
  assert.ok(!Object.prototype.hasOwnProperty.call(JSON.parse(harness.requests[1].options.body), 'facet'));
  cases += 1;
}

async function runConfirmSubmit() {
  const harness = makeHarness({
    startBtn: '',
    timestampInput: '1700000001',
    savedPath: '/saved',
    settingInput: 'confirm setting',
  });
  const body = arrowBody(workspace, "startBtn.addEventListener('click', async () =>");
  vm.runInContext(`async function confirmImport() ${body}`, harness.context);
  await harness.context.confirmImport();
  assert.deepStrictEqual(harness.requests.map((request) => request.url), [
    '/app/import/api/meta',
    '/app/import/api/start',
  ]);
  assertFacetAbsentAndSettingPreserved(harness.requests[0], 'confirm setting');
  assert.ok(!Object.prototype.hasOwnProperty.call(JSON.parse(harness.requests[1].options.body), 'facet'));
  cases += 1;
}

function runHistoryHeaderSummary() {
  const context = vm.createContext({ console });
  vm.runInContext([
    "let cachedSources = [];",
    "let currentSourceFilter = '';",
    "const window = { AppServices: { escapeHtml: (value) => String(value) } };",
    "const escapeHtml = (value) => window.AppServices.escapeHtml(value);",
  ].join('\n'), context);
  vm.runInContext(functionSource(workspace, 'buildHistoryHeader'), context);

  const html = vm.runInContext('buildHistoryHeader(417, 4951)', context);
  assert.ok(
    html.includes('417 imports, 4,951 entries.'),
    'G3-114: summary reads as one plain sentence with thousands separators'
  );
  assert.ok(
    !html.includes('entities found') && !html.includes('imports total'),
    'G3-114: the always-zero entity count and the comma-run wording are gone'
  );

  const singularHtml = vm.runInContext('buildHistoryHeader(1, 1)', context);
  assert.ok(singularHtml.includes('1 import, 1 entry.'), 'singular counts use singular nouns');

  const nullEntriesHtml = vm.runInContext('buildHistoryHeader(5, null)', context);
  assert.ok(nullEntriesHtml.includes('5 imports, —.'), 'null entries format as em-dash');

  const undefinedEntriesHtml = vm.runInContext('buildHistoryHeader(5, undefined)', context);
  assert.ok(undefinedEntriesHtml.includes('5 imports, —.'), 'undefined entries format as em-dash');
  cases += 1;
}

function runImportRowColumns() {
  const context = vm.createContext({ console });
  vm.runInContext([
    "const window = {",
    "  AppServices: { escapeHtml: (value) => String(value) },",
    "  JournalFormat: { timestamp: () => '2026-07-22, 7:30 PM', day: (value) => value },",
    "};",
    "const escapeHtml = (value) => window.AppServices.escapeHtml(value);",
    "const sourceIconSvgByName = {};",
    "const sourceMetadataByName = {};",
  ].join('\n'), context);
  vm.runInContext(functionSource(workspace, 'renderSourceDisplay'), context);
  vm.runInContext(functionSource(workspace, 'formatImportStats'), context);
  vm.runInContext(functionSource(workspace, 'renderImportRow'), context);
  assert.strictEqual(
    vm.runInContext("formatImportStats(0, 0)", context),
    '0 entries • 0 entities',
    'measured zero is 0 not a dash'
  );

  const sourcelessRow = {
    timestamp: 't1', status: 'success', imported_at: 1700000000, target_day: null,
    original_filename: 'note.opus', source_display: 'note.opus',
    total_files_created: 60, entries_written: 60, entities_seeded: 0,
  };
  const sourcelessHtml = vm.runInContext(`renderImportRow(${JSON.stringify(sourcelessRow)})`, context);
  assert.ok(
    /<td class="source-cell">-<\/td>/.test(sourcelessHtml),
    'G3-113: a row with no source type is the only row whose source cell is a dash'
  );
  assert.ok(sourcelessHtml.includes('note.opus'), 'the file column still names the uploaded file');

  const namelessRow = {
    timestamp: 't3', status: 'success', imported_at: 1700000000, target_day: null,
    source_type: 'plaud', source_display: 'Plaud recorder',
    total_files_created: 60, entries_written: 60, entities_seeded: 0,
  };
  const namelessHtml = vm.runInContext(`renderImportRow(${JSON.stringify(namelessRow)})`, context);
  assert.ok(
    namelessHtml.includes('Plaud recorder'),
    'G3-113: an import with no filename still names its source instead of dashing the column'
  );
  assert.ok(
    /<td>-<\/td>/.test(namelessHtml),
    'G3-113: the file column holds a file or a dash, never a copy of the source'
  );

  // G3-308: the .opus imports recorded the uploaded file's name in source_display
  // and carried no source_type, so the source column repeated a filename while the
  // file column was a dash on every row. The file goes in the file column, and the
  // source column shows a source or nothing.
  const recordedFilenameRow = {
    timestamp: 't4', status: 'success', imported_at: 1700000000, target_day: '20260706',
    source_display: '2026-07-06_12_09_03.opus',
    total_files_created: 60, entries_written: 60, entities_seeded: 0,
  };
  const recordedFilenameHtml = vm.runInContext(`renderImportRow(${JSON.stringify(recordedFilenameRow)})`, context);
  assert.ok(
    recordedFilenameHtml.includes('<td>2026-07-06_12_09_03.opus</td>'),
    'G3-308: a recorded filename is shown in the file column'
  );
  assert.ok(
    /<td class="source-cell">-<\/td>/.test(recordedFilenameHtml),
    'G3-308: the source column does not repeat the filename'
  );

  const distinctRow = {
    timestamp: 't2', status: 'success', imported_at: 1700000000, target_day: '20260706',
    original_filename: 'note.opus', source_type: 'plaud', source_display: 'Plaud recorder',
    total_files_created: 60, entries_written: 60, entities_seeded: 0,
  };
  const distinctHtml = vm.runInContext(`renderImportRow(${JSON.stringify(distinctRow)})`, context);
  assert.ok(distinctHtml.includes('Plaud recorder'), 'source cell keeps text that genuinely differs from the file column');
  assert.ok(!/class="file-size/.test(distinctHtml), 'G3-113: the always-empty size column is gone');
  assert.ok(!/class="duration-cell/.test(distinctHtml), 'G3-113: the always-empty duration column is gone');
  assert.ok(!/class="files-cell/.test(distinctHtml), 'G3-113: files-created is dropped; stats carries the count instead');
  assert.ok(distinctHtml.includes('60 entries'), 'stats cell still reports the entry count');
  assert.ok(
    distinctHtml.indexOf('20260706') < distinctHtml.indexOf('2026-07-22, 7:30 PM'),
    'G3-209: the journal day the material landed on leads, and the constant import clock trails'
  );

  const singleEntryRow = Object.assign({}, distinctRow, { entries_written: 1 });
  const singleEntryHtml = vm.runInContext(`renderImportRow(${JSON.stringify(singleEntryRow)})`, context);
  assert.ok(singleEntryHtml.includes('1 entry'), 'G3-210: one entry reads as "1 entry"');
  assert.ok(!singleEntryHtml.includes('1 entries'), 'G3-210: "1 entries" is gone');

  const unconfirmedRow = {
    timestamp: 'u1', status: 'unconfirmed', imported_at: 1700000000, target_day: null,
    original_filename: 'doc.pdf', source_type: 'document',
  };
  const unconfirmedHtml = vm.runInContext(`renderImportRow(${JSON.stringify(unconfirmedRow)})`, context);
  assert.ok(unconfirmedHtml.includes('unconfirmed'), 'unconfirmed history row is not pending');
  assert.ok(!unconfirmedHtml.includes('>pending<'), 'unconfirmed does not render pending');

  const unavailableRow = {
    timestamp: 'u2', status: 'unavailable', imported_at: 1700000000, target_day: null,
    original_filename: 'doc.pdf', source_type: 'document',
  };
  const unavailableHtml = vm.runInContext(`renderImportRow(${JSON.stringify(unavailableRow)})`, context);
  assert.ok(unavailableHtml.includes('unavailable'), 'unavailable history row is not pending');
  assert.ok(!unavailableHtml.includes('>pending<'), 'unavailable does not render pending');

  const gapRow = {
    timestamp: 'g1', status: 'success', imported_at: 1700000000, target_day: '20260706',
    original_filename: 'doc.pdf', source_type: 'document', has_gaps: true,
    entries_written: 1,
  };
  const gapHtml = vm.runInContext(`renderImportRow(${JSON.stringify(gapRow)})`, context);
  assert.ok(gapHtml.includes('contains gaps'), 'gap chip rendered on has_gaps row');
  cases += 1;
}

// G3-401: the progress panel's only live updater called renderProgressView, a
// name that exists nowhere in the workspace. Every importer event raised a
// ReferenceError that the SSE dispatcher caught and logged, so the row in the
// history table went to "completed" while the open panel stayed on its first
// render -- the frozen "preparing..." a field report showed us.
function runProgressPanelUpdatesOnCompletion() {
  const guideSteps = new Element();
  const context = vm.createContext({ console });
  vm.runInContext([
    "const window = {",
    "  AppServices: { escapeHtml: (value) => String(value) },",
    "  location: { hash: '#progress/1700000000' },",
    "  CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },",
    "};",
    "const escapeHtml = (value) => window.AppServices.escapeHtml(value);",
    "let importsCache = [];",
    "let importEvents = {};",
    "let currentGuideSource = null;",
    "const sourceMetadataByName = {};",
  ].join('\n'), context);
  context.document = { getElementById: (id) => (id === 'guideSteps' ? guideSteps : null) };
  vm.runInContext(`const STAGE_NAMES = ${arrowBody(workspace, 'const STAGE_NAMES =')};`, context);
  for (const name of [
    'capitalizeStage', 'humanStageName', 'formatElapsed', 'formatDateRange',
    'formatStatValue', 'formatProgressStats', 'renderProgressStats', 'getImportById',
    'showProgressView', 'refreshInlineProgress',
  ]) {
    vm.runInContext(functionSource(workspace, name), context);
  }

  vm.runInContext("importEvents['1700000000'] = { import_id: '1700000000', event: 'started', stage: 'initialization', source_display: 'Images' };", context);
  vm.runInContext("showProgressView('1700000000')", context);
  assert.ok(guideSteps.innerHTML.includes('preparing'), 'the panel starts on the preparing stage');

  vm.runInContext("refreshInlineProgress('1700000000', { import_id: '1700000000', event: 'completed', entries_written: 1, entities_seeded: 0, duration_ms: 4000 })", context);
  assert.ok(
    guideSteps.innerHTML.includes('import complete'),
    'G3-401: a completed event re-renders the open progress panel'
  );
  assert.ok(
    guideSteps.innerHTML.includes('import another source'),
    'G3-401: the completed panel offers the way back to the upload prompt'
  );
  assert.ok(
    !guideSteps.innerHTML.includes('preparing'),
    'G3-401: the stale preparing stage is gone once the import completes'
  );

  // An event for a different import must not repaint this panel.
  vm.runInContext("refreshInlineProgress('1700000099', { import_id: '1700000099', event: 'error', error: 'other import' })", context);
  assert.ok(
    !guideSteps.innerHTML.includes('other import'),
    'G3-401: only the displayed import repaints the panel'
  );
  cases += 1;
}

// G3-402: initImportWorkspace acted on the hash without waiting for the import
// list, so a reload straight onto #progress/<id> rendered from an empty cache --
// a generic "Import / preparing..." panel for an import that had already
// finished, which no later event would correct.
function runInitWaitsForTheImportList() {
  const order = [];
  const context = vm.createContext({ console, Promise });
  context.window = {
    solPathContext: () => ({ segment: null }),
    appEvents: { listen: () => ({ pending: { track() {}, clear() {} } }) },
  };
  context.document = { getElementById: () => new Element() };
  vm.runInContext([
    "const IMPORT_STALL_TIMEOUT_MS = 1000;",
    "const IMPORT_ROW_EVENTS = new Set();",
    "const IMPORT_TERMINAL_EVENTS = new Set();",
    "let importEventsCleanup = null;",
    "const markRowStalled = () => {};",
    "const updateImportRow = () => {};",
    "const trackPendingImport = () => {};",
    "const clearPendingImport = () => {};",
  ].join('\n'), context);
  context.order = order;
  context.loadImports = async () => { await null; order.push('importList'); };
  context.handleHashChange = () => { order.push('hash'); };
  context.loadSourceGrid = () => { order.push('sourceGrid'); };
  vm.runInContext(functionSource(workspace, 'initImportWorkspace'), context);
  vm.runInContext('initImportWorkspace()', context);

  return new Promise((resolve) => setImmediate(resolve)).then(() => {
    assert.deepStrictEqual(
      order,
      ['importList', 'hash'],
      'G3-402: the hash is acted on only after the import list has loaded'
    );
    cases += 1;
  });
}

// G3-403: the panel's heading read only the live event while the summary below it
// also read the import list, so a reload onto a finished import headlined
// "preparing..." over its own completion summary. importEvents is in-memory and a
// reload empties it, which is exactly the case a reload lands in.
function runReloadedPanelAgreesWithItsOwnSummary() {
  const guideSteps = new Element();
  const context = vm.createContext({ console });
  vm.runInContext([
    "const window = {",
    "  AppServices: { escapeHtml: (value) => String(value) },",
    "  location: { hash: '#progress/1700000000' },",
    "  CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },",
    "};",
    "const escapeHtml = (value) => window.AppServices.escapeHtml(value);",
    "let importEvents = {};",   // a reload empties this
    "let currentGuideSource = null;",
    "const sourceMetadataByName = {};",
    "let importsCache = [{ timestamp: '1700000000', status: 'success', source_display: 'Images', entries_written: 1, entities_seeded: 0 }];",
  ].join('\n'), context);
  context.document = { getElementById: (id) => (id === 'guideSteps' ? guideSteps : null) };
  vm.runInContext(`const STAGE_NAMES = ${arrowBody(workspace, 'const STAGE_NAMES =')};`, context);
  for (const name of [
    'capitalizeStage', 'humanStageName', 'formatElapsed', 'formatDateRange',
    'formatStatValue', 'formatProgressStats', 'renderProgressStats', 'getImportById', 'showProgressView',
  ]) {
    vm.runInContext(functionSource(workspace, name), context);
  }

  vm.runInContext("showProgressView('1700000000')", context);
  assert.ok(
    guideSteps.innerHTML.includes('import complete'),
    'G3-403: a reload onto a finished import says so'
  );
  assert.ok(
    !guideSteps.innerHTML.includes('preparing'),
    'G3-403: the heading does not contradict the completion summary under it'
  );
  assert.ok(
    guideSteps.innerHTML.includes('import another source'),
    'G3-403: the reloaded panel still offers the way on'
  );

  // The same reading governs the failed case.
  vm.runInContext("importsCache = [{ timestamp: '1700000001', status: 'failed', error: 'disk full' }];", context);
  vm.runInContext("window.location.hash = '#progress/1700000001'; showProgressView('1700000001')", context);
  assert.ok(
    guideSteps.innerHTML.includes('import failed') && guideSteps.innerHTML.includes('disk full'),
    'G3-403: a reload onto a failed import says so, with the reason'
  );

  // An import still running is still reported as running.
  vm.runInContext("importsCache = [{ timestamp: '1700000002', status: 'running' }];", context);
  vm.runInContext("window.location.hash = '#progress/1700000002'; showProgressView('1700000002')", context);
  assert.ok(
    guideSteps.innerHTML.includes('preparing'),
    'G3-403: an unfinished import is not reported as complete'
  );

  // The list is only as fresh as the last load, so a live event outranks it: a
  // retry in progress must not wear the failure cached against its own id.
  vm.runInContext("importsCache = [{ timestamp: '1700000003', status: 'failed', error: 'disk full' }];", context);
  vm.runInContext("importEvents['1700000003'] = { import_id: '1700000003', event: 'status', stage: 'writing' };", context);
  vm.runInContext("window.location.hash = '#progress/1700000003'; showProgressView('1700000003')", context);
  assert.ok(
    guideSteps.innerHTML.includes('writing to journal'),
    'G3-403: a live event outranks a stale terminal row in the list'
  );
  assert.ok(
    !guideSteps.innerHTML.includes('disk full'),
    'G3-403: and the stale failure summary does not render under it'
  );
  cases += 1;
}

async function runRepeatSourceFocus() {
  const fileInput = new Element();
  const pathInput = new Element();
  const guide = new Element();
  const guideSteps = new Element();
  const context = vm.createContext({ console, Promise, setImmediate, setTimeout });
  let showGridCalled = false;
  let dropArea = null;

  let _hash = '#progress/1700000000';
  context.window = {
    location: {
      get hash() { return _hash; },
      set hash(val) {
        _hash = !val ? '' : (val.startsWith('#') ? val : '#' + val);
      }
    },
    _guidedClientItemId: 'item-1',
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  context.document = {
    getElementById: (id) => {
      if (id === 'guidedFileInput') return fileInput;
      if (id === 'guidedDropArea') return dropArea;
      if (id === 'guidedPathInput') return pathInput;
      if (id === 'importGuide' || id === 'importGrid') return guide;
      if (id === 'guideSteps') return guideSteps;
      return null;
    },
    querySelector: () => null,
  };
  context.importEvents = { '1700000000': { import_id: '1700000000' } };
  context.importGenerationFloor = {};
  context.currentGuideSource = null;
  context.sourceMetadataByName = { claude: { name: 'claude', display_name: 'Claude', has_guide: true } };
  context.getImportById = () => null;
  context.escapeHtml = (s) => String(s);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '-';
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.reconcileImportState = () => {};
  context.showGrid = () => { showGridCalled = true; };
  context.loadGuidedFlow = async () => {
    await new Promise((resolve) => setImmediate(resolve));
    dropArea = new Element();
  };
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);
  context.IMPORT_TERMINAL_EVENTS = new Set(['completed', 'error', 'declined']);
  context.inFlightReconcile = new Set();
  context.navigatingHash = false;

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);
  vm.runInContext(functionSource(workspace, 'refreshInlineProgress'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);
  vm.runInContext(functionSource(workspace, 'navigateTo'), context);
  vm.runInContext(functionSource(workspace, 'repeatSource'), context);

  await vm.runInContext("repeatSource('claude')", context);

  assert.ok(dropArea, 'await navigateTo waits for has_guide loadGuidedFlow to create the drop area');
  assert.strictEqual(dropArea.focused, true, 'repeatSource focuses guidedDropArea when available');
  assert.strictEqual(fileInput.focused, false, 'repeatSource does not focus hidden fileInput');
  assert.strictEqual(context.window.location.hash, '#guide/claude', 'repeatSource navigates to #guide/claude');
  assert.strictEqual(context.window._guidedClientItemId, null, 'repeatSource clears _guidedClientItemId');
  assert.strictEqual(showGridCalled, false, 'repeatSource does not call showGrid');
  cases += 1;
}

function runTerminalImportEventsNotOverwrittenByStart() {
  const startButton = new Element();
  startButton.textContent = 'start';
  const settingInput = new Element();
  settingInput.value = '';

  const context = vm.createContext({ console, Promise });
  context.window = {
    location: { hash: '' },
    showError: () => {},
  };
  context.document = {
    getElementById: (id) => {
      if (id === 'guidedStartBtn') return startButton;
      if (id === 'guidedSettingInput') return settingInput;
      return null;
    },
    querySelector: () => null,
  };
  context.currentGuidedSaved = { path: '/tmp/test.png', timestamp: '1700000001' };
  context.isTerminalDuplicate = () => false;
  context.trackPendingImport = () => {};
  context.clearPendingImport = () => {};
  context.loadImports = () => {};
  context.navigateTo = () => {};
  context.fetch = async () => ({
    ok: true,
    json: async () => ({ status: 'started' }),
  });
  context.IMPORT_TERMINAL_EVENTS = new Set(['completed', 'error', 'declined']);
  context.importEvents = {
    '1700000001': { import_id: '1700000001', event: 'completed', entries_written: 5 }
  };

  vm.runInContext(functionSource(workspace, 'startGuidedImport'), context);

  const startSrc = functionSource(workspace, 'startGuidedImport');
  assert.ok(
    startSrc.includes('IMPORT_TERMINAL_EVENTS.has(importEvents[ts].event)'),
    'start-path terminal guard is present'
  );
  assert.ok(
    startSrc.includes('...(importEvents[ts] || {})'),
    'start-path spread preserves an existing terminal event'
  );

  return vm.runInContext("startGuidedImport({ name: 'image', display_name: 'Images', input_type: 'file_upload' })", context).then(() => {
    const event = context.importEvents['1700000001'];
    assert.strictEqual(event.event, 'completed', 'terminal completed event is not overwritten by start');
    assert.strictEqual(event.entries_written, 5, 'completed event properties are preserved');
    cases += 1;
  });
}

async function runStallCallsReconcile() {
  const context = vm.createContext({ console, Promise });
  const fetchedUrls = [];
  const tracked = [];
  const statusCell = new Element();
  const row = {
    classList: { add() {}, remove() {} },
    querySelector: (sel) => (sel === '.status-cell' ? statusCell : null),
  };
  context.window = {
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
    apiJson: async (url) => {
      fetchedUrls.push(url);
      return { import_id: '1700000002', status: 'running', generation: 1 };
    },
  };
  context.document = {
    querySelector: () => row,
    getElementById: () => null,
    body: { contains: () => true },
  };
  context.CSS = { escape: (s) => s };
  context.importEvents = {};
  context.importGenerationFloor = {};
  context.clearPendingImport = () => {};
  context.refreshInlineProgress = () => {};
  context.trackPendingImport = (id) => { tracked.push(id); };
  context.humanStageName = (s) => s;
  context.escapeHtml = (s) => s;
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);
  context.IMPORT_TERMINAL_EVENTS = new Set(['completed', 'error', 'declined']);
  context.inFlightReconcile = new Set();

  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);
  vm.runInContext(functionSource(workspace, 'reconcileImportState'), context);
  vm.runInContext(functionSource(workspace, 'markRowStalled'), context);

  vm.runInContext("markRowStalled('1700000002')", context);
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepStrictEqual(
    fetchedUrls,
    ['/app/import/api/1700000002'],
    'stall path calls reconcileImportState with /app/import/api/{id}'
  );
  assert.strictEqual(context.importEvents['1700000002'].stalled, true, 'running answer from stall keeps stalled');
  assert.ok(tracked.includes('1700000002'), 'stall running answer re-tracks the pending import');
  assert.ok(statusCell.innerHTML.includes('check status'), 'stalled row offers check status');
  assert.ok(!statusCell.innerHTML.includes('retry-import'), 'stalled row does not post retry-import');
  cases += 1;
}

function runGenerationComparisonInRowUpdates() {
  const guideSteps = new Element();
  const context = vm.createContext({ console });
  context.importEvents = {
    '1700000001': { import_id: '1700000001', generation: 2, event: 'status', stage: 'writing' }
  };
  context.importGenerationFloor = { '1700000001': 2 };
  context.document = {
    querySelector: () => null,
    getElementById: (id) => (id === 'guideSteps' ? guideSteps : null),
  };
  context.window = {
    location: { hash: '#progress/1700000001' },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  context.currentGuideSource = null;
  context.sourceMetadataByName = {};
  context.getImportById = () => null;
  context.escapeHtml = (s) => String(s);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '-';
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);
  context.IMPORT_TERMINAL_EVENTS = new Set(['completed', 'error', 'declined']);
  context.inFlightReconcile = new Set();

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);
  vm.runInContext(functionSource(workspace, 'refreshInlineProgress'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);

  // Older generation (1) is ignored against non-terminal state (m2 test)
  vm.runInContext("updateImportRow('1700000001', { import_id: '1700000001', generation: 1, event: 'status', stage: 'initialization' })", context);
  assert.strictEqual(context.importEvents['1700000001'].stage, 'writing', 'older generation event ignored on non-terminal state');

  // Generation null/undefined is ignored when currGen exists
  vm.runInContext("updateImportRow('1700000001', { import_id: '1700000001', event: 'status', stage: 'initialization' })", context);
  assert.strictEqual(context.importEvents['1700000001'].stage, 'writing', 'null generation event ignored');

  // Same generation status cannot overwrite completed terminal state (m5 test)
  context.importEvents['1700000001'] = { import_id: '1700000001', generation: 2, event: 'completed', status: 'success', entries_written: 10 };
  vm.runInContext("updateImportRow('1700000001', { import_id: '1700000001', generation: 2, event: 'status', stage: 'writing' })", context);
  assert.strictEqual(context.importEvents['1700000001'].event, 'completed', 'running event cannot overwrite terminal of same generation');
  assert.strictEqual(context.importEvents['1700000001'].entries_written, 10, 'completed facts preserved');

  // Higher generation running event is accepted
  vm.runInContext("updateImportRow('1700000001', { import_id: '1700000001', generation: 3, event: 'status', stage: 'processing' })", context);
  assert.strictEqual(context.importEvents['1700000001'].generation, 3, 'newer generation accepted');
  assert.strictEqual(context.importEvents['1700000001'].stage, 'processing', 'newer generation running event applied');
  cases += 1;
}

function runShowProgressViewDoesNotReconcile() {
  const guideSteps = new Element();
  const context = vm.createContext({ console });
  let reconcileCount = 0;
  context.document = {
    getElementById: (id) => (id === 'guideSteps' ? guideSteps : null),
  };
  context.window = {
    location: { hash: '#progress/1700000001' },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  context.reconcileImportState = () => { reconcileCount += 1; };
  context.sourceMetadataByName = {};
  context.currentGuideSource = null;
  context.importEvents = {};
  context.getImportById = () => null;
  context.escapeHtml = (s) => String(s);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '-';
  context.formatStatValue = (v) => String(v);

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);

  vm.runInContext("showProgressView('1700000001', { import_id: '1700000001', event: 'started' })", context);
  assert.strictEqual(reconcileCount, 0, 'showProgressView does not trigger reconcileImportState directly');
  cases += 1;
}

function runDistinctUnconfirmedAndUnavailablePanels() {
  const guideSteps = new Element();
  const context = vm.createContext({ console });
  context.document = {
    getElementById: (id) => (id === 'guideSteps' ? guideSteps : null),
  };
  context.window = {
    location: { hash: '#progress/1700000001' },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  context.encodeURIComponent = encodeURIComponent;
  context.CSS = { escape: (s) => s };
  context.sourceMetadataByName = {};
  context.currentGuideSource = null;
  context.importEvents = {};
  context.getImportById = () => null;
  context.escapeHtml = (s) => String(s);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '-';

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);

  // Unconfirmed panel
  vm.runInContext("showProgressView('1700000001', { import_id: '1700000001', status: 'unconfirmed', total_files_created: 1, entries_written: 1 })", context);
  assert.ok(guideSteps.innerHTML.includes('import incomplete') || guideSteps.innerHTML.includes('import unconfirmed'), 'unconfirmed title rendered');
  assert.ok(guideSteps.innerHTML.includes('check status'), 'check status button rendered');
  assert.ok(guideSteps.innerHTML.includes('browse what was imported'), 'browse button rendered when verified outputs exist');
  assert.ok(!guideSteps.innerHTML.includes('repeatSource'), 'no repeat button on unconfirmed');
  assert.ok(!guideSteps.innerHTML.includes('retry-import'), 'no retry-import on unconfirmed');

  // Unavailable panel
  vm.runInContext("showProgressView('1700000002', { import_id: '1700000002', status: 'unavailable', unavailable_description: 'metadata corrupted' })", context);
  assert.ok(guideSteps.innerHTML.includes('import status unavailable') || guideSteps.innerHTML.includes('import unavailable'), 'unavailable title rendered');
  assert.ok(guideSteps.innerHTML.includes('metadata corrupted'), 'unavailable description rendered');
  assert.ok(guideSteps.innerHTML.includes('check status'), 'check status button rendered');
  assert.ok(!guideSteps.innerHTML.includes('repeatSource'), 'no repeat button on unavailable');

  vm.runInContext("showProgressView('1700000003', { import_id: '1700000003', event: 'error', status: 'failed', error: 'import failed' })", context);
  assert.ok(!guideSteps.innerHTML.includes('repeatSource'), 'no repeat on failed panel');
  assert.ok(!guideSteps.innerHTML.includes('data-source='), 'failed panel has no same-source repeat');

  vm.runInContext("currentGuideSource = 'image'; showProgressView('id/with space', { import_id: 'id/with space', event: 'completed', status: 'success' })", context);
  assert.ok(guideSteps.innerHTML.includes('data-source='), 'confirmed success offers data-source repeat');
  assert.ok(!guideSteps.innerHTML.includes("onclick=\"repeatSource("), 'no interpolated onclick repeatSource');
  assert.ok(guideSteps.innerHTML.includes('/app/import/id%2Fwith%20space'), 'browse href uses encodeURIComponent');
  cases += 1;
}

async function runSingleFlightReconcile() {
  const context = vm.createContext({ console, Promise });
  let fetchCount = 0;
  let resolveFirst;
  const firstPromise = new Promise((resolve) => { resolveFirst = resolve; });

  context.window = {
    apiJson: async () => {
      fetchCount += 1;
      await firstPromise;
      return { status: 'running' };
    },
  };
  context.document = { querySelector: () => null, getElementById: () => null };
  context.importEvents = {};
  context.clearPendingImport = () => {};
  context.updateImportRow = () => {};
  context.IMPORT_TERMINAL_EVENTS = new Set(['completed', 'error', 'declined']);
  context.inFlightReconcile = new Set();

  vm.runInContext(functionSource(workspace, 'reconcileImportState'), context);

  // Fire 3 simultaneous calls for the same importId
  const p1 = vm.runInContext("reconcileImportState('1700000001')", context);
  const p2 = vm.runInContext("reconcileImportState('1700000001')", context);
  const p3 = vm.runInContext("reconcileImportState('1700000001')", context);

  resolveFirst();
  await Promise.all([p1, p2, p3]);

  assert.strictEqual(fetchCount, 1, 'single-flight reconcile issues exactly 1 request');
  cases += 1;
}

async function runNavigateToProgressIssuesOnlyOneGet() {
  const guideSteps = new Element();
  const guide = new Element();
  const context = vm.createContext({ console, Promise });
  let fetchCount = 0;
  let _hash = '';
  context.window = {
    location: {
      get hash() { return _hash; },
      set hash(val) {
        _hash = !val ? '' : (val.startsWith('#') ? val : '#' + val);
      }
    },
    apiJson: async () => {
      fetchCount += 1;
      return { import_id: '1700000001', status: 'running', generation: 1 };
    },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  context.document = {
    getElementById: (id) => {
      if (id === 'guideSteps') return guideSteps;
      if (id === 'sourceGuide' || id === 'importGuide') return guide;
      return null;
    },
    querySelector: () => null,
  };
  context.sourceMetadataByName = {};
  context.currentGuideSource = null;
  context.importEvents = {
    '1700000001': { import_id: '1700000001', event: 'started', generation: 1 }
  };
  context.getImportById = () => null;
  context.escapeHtml = (s) => String(s);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '-';
  context.formatStatValue = (v) => String(v);
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.importGenerationFloor = {};
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);
  context.IMPORT_TERMINAL_EVENTS = new Set(['completed', 'error', 'declined']);
  context.inFlightReconcile = new Set();

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);
  vm.runInContext(functionSource(workspace, 'refreshInlineProgress'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);
  vm.runInContext(functionSource(workspace, 'reconcileImportState'), context);
  vm.runInContext(functionSource(workspace, 'navigateTo'), context);

  context.window.addEventListener = (type, handler) => {
    if (type === 'hashchange') {
      context._hashchange = handler;
    }
  };
  await vm.runInContext("navigateTo('progress/1700000001')", context);
  if (typeof context._hashchange === 'function') {
    await context._hashchange();
  }
  await new Promise((resolve) => setImmediate(resolve));

  assert.strictEqual(context.window.location.hash, '#progress/1700000001', 'hash is #progress/1700000001');
  assert.strictEqual(fetchCount, 1, 'navigateTo + showProgressView issues exactly 1 GET to reconcileImportState');
  cases += 1;
}

async function runConnectionStateReconcile() {
  const context = vm.createContext({ console, Promise });
  let reconciledId = null;
  let connectionCallback = null;

  context.window = {
    location: { hash: '#progress/1700000002' },
    appEvents: {
      onConnectionState: (cb) => {
        connectionCallback = cb;
        return () => {};
      },
      listen: () => () => {},
    },
    solPathContext: () => ({ segment: null }),
  };
  context.document = {
    getElementById: () => null,
  };
  context.reconcileImportState = (id) => {
    reconciledId = id;
  };
  context.markRowStalled = () => {};
  context.loadImports = () => Promise.resolve();
  context.handleHashChange = () => {};
  context.connectionStateCleanup = null;
  context.IMPORT_STALL_TIMEOUT_MS = 600000;
  context.IMPORT_ROW_EVENTS = new Set();
  context.IMPORT_TERMINAL_EVENTS = new Set();

  vm.runInContext(functionSource(workspace, 'initImportWorkspace'), context);
  vm.runInContext("initImportWorkspace()", context);

  assert.ok(typeof connectionCallback === 'function', 'connection callback registered');

  // Disconnect event
  connectionCallback({ connected: false });
  assert.strictEqual(reconciledId, null, 'disconnect does not trigger reconcile');

  // Reconnect event
  connectionCallback({ connected: true });
  assert.strictEqual(reconciledId, '1700000002', 'reconnect triggers reconcile for active progress hash');
  cases += 1;
}

async function runRepeatSourceThenDelayedOldGenCompleted() {
  const dropArea = new Element();
  const fileInput = new Element();
  const pathInput = new Element();
  const guide = new Element();
  const guideSteps = new Element();
  const context = vm.createContext({ console, Promise });

  let _hash = '#progress/1700000001';
  context.window = {
    location: {
      get hash() { return _hash; },
      set hash(val) {
        _hash = !val ? '' : (val.startsWith('#') ? val : '#' + val);
      }
    },
    _guidedClientItemId: 'client-item-1',
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  context.document = {
    getElementById: (id) => {
      if (id === 'guidedFileInput') return fileInput;
      if (id === 'guidedDropArea') return dropArea;
      if (id === 'guidedPathInput') return pathInput;
      if (id === 'importGuide' || id === 'importGrid') return guide;
      if (id === 'guideSteps') return guideSteps;
      return null;
    },
    querySelector: () => null,
  };
  context.importEvents = {
    '1700000001': { import_id: '1700000001', generation: 2, event: 'completed', status: 'success' }
  };
  context.importGenerationFloor = { '1700000001': 2 };
  context.currentGuideSource = null;
  context.sourceMetadataByName = {};
  context.getImportById = () => null;
  context.escapeHtml = (s) => String(s);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '-';
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.loadGuidedFlow = async () => {};
  context.showGrid = () => {};
  context.reconcileImportState = () => {};
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);
  context.IMPORT_TERMINAL_EVENTS = new Set(['completed', 'error', 'declined']);
  context.inFlightReconcile = new Set();

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);
  vm.runInContext(functionSource(workspace, 'refreshInlineProgress'), context);
  vm.runInContext(functionSource(workspace, 'navigateTo'), context);
  vm.runInContext(functionSource(workspace, 'repeatSource'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);

  // Call repeatSource('image')
  await vm.runInContext("repeatSource('image')", context);
  assert.strictEqual(context.window.location.hash, '#guide/image', 'navigated to #guide/image');
  assert.strictEqual(context.window._guidedClientItemId, null, 'guided client item ID reset');
  assert.strictEqual(context.importEvents['1700000001'].stalled, false, 'old import kept with generation floor');

  // Delayed generation 1 event arrives
  vm.runInContext("updateImportRow('1700000001', { import_id: '1700000001', generation: 1, event: 'completed', status: 'success' })", context);

  // Verify that hash and guided state are undisturbed
  assert.strictEqual(context.window.location.hash, '#guide/image', 'view remains guided after delayed event');
  assert.strictEqual(context.window._guidedClientItemId, null, 'guided state is not corrupted by delayed event');
  cases += 1;
}

function runShowGridResetsGuidedClientItemId() {
  const context = vm.createContext({ console });
  context.window = { _guidedClientItemId: 'client-1' };
  context.currentGuideSource = 'image';
  context.currentGuidedFile = 'file.png';
  context.currentGuidedSaved = {};
  context.navigateTo = () => {};

  vm.runInContext(functionSource(workspace, 'showGrid'), context);
  vm.runInContext('showGrid()', context);

  assert.strictEqual(context.window._guidedClientItemId, null, 'showGrid resets _guidedClientItemId');
  assert.strictEqual(context.currentGuideSource, null, 'showGrid resets currentGuideSource');
  cases += 1;
}

function runAuthoritativeUpdateBypassesGenerationFloor() {
  const context = vm.createContext({ console });
  context.importEvents = {
    '1700000001': { import_id: '1700000001', generation: 1, event: 'started', status: 'running' }
  };
  context.importGenerationFloor = { '1700000001': 1 };
  context.document = { querySelector: () => null };
  context.refreshInlineProgress = () => {};
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);

  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);

  // Non-authoritative update with null generation is ignored
  vm.runInContext("updateImportRow('1700000001', { import_id: '1700000001', generation: null, status: 'unavailable', event: 'error' })", context);
  assert.strictEqual(context.importEvents['1700000001'].status, 'running', 'non-authoritative null-gen update ignored');

  // Authoritative update with null generation updates the state
  vm.runInContext("updateImportRow('1700000001', { import_id: '1700000001', generation: null, status: 'unavailable', event: 'error' }, { authoritative: true })", context);
  assert.strictEqual(context.importEvents['1700000001'].status, 'unavailable', 'authoritative update applies regardless of generation floor');
  cases += 1;
}

function runLiveEventClearsStalled() {
  const context = vm.createContext({ console });
  context.importEvents = {
    '1700000001': { import_id: '1700000001', generation: 1, event: 'status', status: 'running', stalled: true }
  };
  context.importGenerationFloor = { '1700000001': 1 };
  context.document = { querySelector: () => null };
  context.refreshInlineProgress = () => {};
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);

  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);

  // Live status event arrives without fromStall
  vm.runInContext("updateImportRow('1700000001', { import_id: '1700000001', generation: 1, event: 'status', status: 'running' })", context);
  assert.strictEqual(context.importEvents['1700000001'].stalled, false, 'live event clears stalled state');
  cases += 1;
}

function runSingularPageUnavailable() {
  const guideSteps = new Element();
  const context = vm.createContext({ console });
  context.document = {
    getElementById: (id) => (id === 'guideSteps' ? guideSteps : null),
  };
  context.window = {
    location: { hash: '#progress/1700000001' },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
    AppServices: { escapeHtml: (s) => String(s) },
  };
  context.sourceMetadataByName = {};
  context.currentGuideSource = null;
  context.importEvents = {};
  context.getImportById = () => null;
  context.escapeHtml = (s) => String(s);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '-';

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);

  vm.runInContext("showProgressView('1700000001', { import_id: '1700000001', status: 'success', unavailable_pages: 1 })", context);
  assert.ok(guideSteps.innerHTML.includes('1 page unavailable'), 'singular 1 page unavailable rendered');
  assert.ok(!guideSteps.innerHTML.includes('1 pages unavailable'), 'not plural for 1 page');

  vm.runInContext("showProgressView('1700000002', { import_id: '1700000002', status: 'success', unavailable_pages: 2 })", context);
  assert.ok(guideSteps.innerHTML.includes('2 pages unavailable'), 'plural 2 pages unavailable rendered');
  cases += 1;
}

function runFormatStatValueEscapesAndHandlesEmDash() {
  const context = vm.createContext({ console });
  context.escapeHtml = (s) => String(s).replace(/</g, '&lt;').replace(/>/g, '&gt;');

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);

  assert.strictEqual(vm.runInContext("formatStatValue(null)", context), '—', 'null returns em-dash');
  assert.strictEqual(vm.runInContext("formatStatValue(undefined)", context), '—', 'undefined returns em-dash');
  assert.strictEqual(vm.runInContext("formatStatValue(42)", context), '42', 'number formatted as string');
  assert.strictEqual(vm.runInContext("formatStatValue('<script>')", context), '&lt;script&gt;', 'html escaped in formatStatValue');
  cases += 1;
}

async function runAsyncNavigateToWithGuideFetch() {
  const grid = new Element();
  const guide = new Element();
  const context = vm.createContext({ console, Promise });
  let guideFlowFinished = false;
  context.window = {
    location: { hash: '' },
  };
  context.document = {
    getElementById: (id) => {
      if (id === 'importGrid') return grid;
      if (id === 'importGuide') return guide;
      return null;
    }
  };
  context.loadGuidedFlow = async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
    guideFlowFinished = true;
  };
  context.showProgressView = () => {};
  context.reconcileImportState = async () => {};

  vm.runInContext(functionSource(workspace, 'navigateTo'), context);

  const promise = vm.runInContext("navigateTo('guide/pdf')", context);
  assert.strictEqual(guideFlowFinished, false, 'guide flow in progress');
  await promise;
  assert.strictEqual(guideFlowFinished, true, 'navigateTo awaited loadGuidedFlow');
  cases += 1;
}

Promise.resolve()
  .then(runQuickSubmit)
  .then(runGuidedSubmit)
  .then(runConfirmSubmit)
  .then(runHistoryHeaderSummary)
  .then(runImportRowColumns)
  .then(runProgressPanelUpdatesOnCompletion)
  .then(runInitWaitsForTheImportList)
  .then(runReloadedPanelAgreesWithItsOwnSummary)
  .then(runRepeatSourceFocus)
  .then(runTerminalImportEventsNotOverwrittenByStart)
  .then(runStallCallsReconcile)
  .then(runGenerationComparisonInRowUpdates)
  .then(runShowProgressViewDoesNotReconcile)
  .then(runDistinctUnconfirmedAndUnavailablePanels)
  .then(runSingleFlightReconcile)
  .then(runNavigateToProgressIssuesOnlyOneGet)
  .then(runConnectionStateReconcile)
  .then(runRepeatSourceThenDelayedOldGenCompleted)
  .then(runShowGridResetsGuidedClientItemId)
  .then(runAuthoritativeUpdateBypassesGenerationFloor)
  .then(runLiveEventClearsStalled)
  .then(runSingularPageUnavailable)
  .then(runFormatStatValueEscapesAndHandlesEmDash)
  .then(runAsyncNavigateToWithGuideFetch)
  .then(() => console.log(`DOM CASES: ${cases} passed`))
  .catch((error) => {
    console.error(error.stack || error);
    process.exitCode = 1;
  });
