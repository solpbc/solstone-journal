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

function detailSource() {
  return fs.readFileSync(path.join(crateDir, 'assets/import_detail.js'), 'utf8');
}

// import_detail.js is an IIFE that hangs its renderer off window, so the whole
// shipped file runs and the tests call the real thing. The drawer chrome is the
// shell's; the body it is handed is what this crate renders.
function loadImportDetailModule() {
  const context = vm.createContext({ console });
  context.window = {
    JournalFormat: { day: (value) => String(value) },
    Drawer: {
      render: (options) => `<div class="drawer"><p class="drawer-line">${options.line}</p>${options.bodyHtml}</div>`,
    },
  };
  vm.runInContext(detailSource(), context);
  assert.ok(context.window.ImportDetail, 'import_detail.js publishes its renderer on window');
  return context.window.ImportDetail;
}

// workspace.html's escapeHtml is a one-line delegate to AppServices.escapeHtml,
// which ships in the convey shell. import_detail.js carries this crate's own
// copy of that function -- the same five characters, the same entities -- so
// the harness renders through a real escaper instead of an identity stub. Every
// context that renders a server- or URL-derived string installs it, and
// assertEscaperIsReal below fails loudly if that copy ever stops escaping.
function realEscapeHtmlSource() {
  return functionSource(detailSource(), 'escapeHtml');
}

// Installs the real escapeHtml as a context global (and as
// window.AppServices.escapeHtml when the context already has a window). It has
// to run before any `const escapeHtml = ...` line would: a function declaration
// cannot follow a lexical binding of the same name in the same global.
function installRealEscapeHtml(context) {
  vm.runInContext(
    `${realEscapeHtmlSource()}\n`
    + "if (typeof window === 'object' && window) {\n"
    + '  window.AppServices = Object.assign({}, window.AppServices, { escapeHtml });\n'
    + '}\n',
    context
  );
}

const ESCAPE_PAYLOAD = 'x"y\'z<b>&';
const ESCAPED_PAYLOAD = 'x&quot;y&#39;z&lt;b&gt;&amp;';

function assertEscaperIsReal(context) {
  assert.strictEqual(
    vm.runInContext(`escapeHtml(${JSON.stringify(ESCAPE_PAYLOAD)})`, context),
    ESCAPED_PAYLOAD,
    'the harness renders through a real escapeHtml, not an identity stub'
  );
}

// What a browser actually stores in the fragment: a space, a double quote,
// angle brackets and a backtick come back percent-encoded, and so does anything
// outside ASCII. A value handed to the setter is therefore not always the value
// read back, which is what lets a "nothing changed" assignment look like a
// change to the caller that wrote it.
const FRAGMENT_ESCAPES = { ' ': '%20', '"': '%22', '<': '%3C', '>': '%3E', '`': '%60' };

function normalizeFragment(value) {
  return String(value)
    .replace(/[ "<>`]/g, (char) => FRAGMENT_ESCAPES[char])
    .replace(/[^\x00-\x7F]/g, (char) => encodeURIComponent(char));
}

// A browser normalizes what the hash setter is handed and delivers hashchange
// as a queued task -- never inside the assignment. Both halves matter: a dedup
// flag cleared on the line after the assignment is already clear when the
// handler runs, and an assignment that does not change the fragment fires
// nothing at all.
class QueuedHashLocation {
  constructor(initial = '') {
    this._hash = initial;
    this._tasks = [];
    this.onhashchange = null;
  }

  get hash() {
    return this._hash;
  }

  set hash(value) {
    const raw = value === null || value === undefined ? '' : String(value);
    const next = normalizeFragment(!raw ? '' : (raw.startsWith('#') ? raw : `#${raw}`));
    if (next === this._hash) {
      return;
    }
    this._hash = next;
    this._tasks.push(new Promise((resolve, reject) => {
      setImmediate(() => {
        Promise.resolve(typeof this.onhashchange === 'function' ? this.onhashchange() : undefined)
          .then(resolve, reject);
      });
    }));
  }

  async settle() {
    while (this._tasks.length) {
      await Promise.all(this._tasks.splice(0, this._tasks.length));
    }
  }
}

// Delivers every queued hashchange and lets the work it starts finish.
async function drain(location) {
  await location.settle();
  for (let turn = 0; turn < 4; turn += 1) {
    await new Promise((resolve) => setImmediate(resolve));
    await location.settle();
  }
}

// Enough of CSS.escape for an attribute-selector value: everything that is not
// a plain identifier character is backslash-escaped, which is what the real one
// does with the quote this test turns on.
const cssEscape = (value) => String(value).replace(/[^a-zA-Z0-9_-]/g, (char) => `\\${char}`);

const ATTRIBUTE_SELECTOR = /^[a-zA-Z]*\[[a-zA-Z-]+="((?:[^"\\]|\\.)*)"\]$/;

// A browser throws a DOMException on a selector whose attribute string is not
// terminated -- an unescaped " closes it early and leaves junk behind. The
// harness's plain `() => null` stub swallowed exactly that, so dropping
// CSS.escape looked harmless.
function selectorLookup(rowsByValue) {
  return (selector) => {
    const match = ATTRIBUTE_SELECTOR.exec(selector);
    if (!match) {
      const error = new Error(`Failed to execute 'querySelector' on 'Document': '${selector}' is not a valid selector.`);
      error.name = 'SyntaxError';
      throw error;
    }
    const value = match[1].replace(/\\(.)/g, '$1');
    return rowsByValue[value] || null;
  };
}

// The same validation in front of a lookup that answers by selector shape
// rather than by attribute value: any attribute selector still has to parse.
function strictSelector(handler) {
  return (selector) => {
    if (selector.includes('[') && !ATTRIBUTE_SELECTOR.test(selector)) {
      const error = new Error(`Failed to execute 'querySelector': '${selector}' is not a valid selector.`);
      error.name = 'SyntaxError';
      throw error;
    }
    return handler(selector);
  };
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

// An element that tells its document it has focus, and that can be detached the
// way a re-render detaches it: a browser takes focus off a node it removes.
class FocusableElement extends Element {
  constructor(id, ownerDocument, attributes = {}) {
    super('');
    this.id = id;
    this.ownerDocument = ownerDocument;
    this.attributes = attributes;
    this.attached = true;
  }

  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  focus() {
    super.focus();
    if (this.ownerDocument && this.attached) {
      this.ownerDocument.activeElement = this;
    }
  }

  detach() {
    this.attached = false;
    if (this.ownerDocument && this.ownerDocument.activeElement === this) {
      this.ownerDocument.activeElement = null;
    }
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
    "const window = {};",
  ].join('\n'), context);
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
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
    "  JournalFormat: { timestamp: () => '2026-07-22, 7:30 PM', day: (value) => value },",
    "};",
    "const sourceIconSvgByName = {};",
    "const sourceMetadataByName = {};",
  ].join('\n'), context);
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
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
    /<td class="source-cell">—<\/td>/.test(sourcelessHtml),
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
    /<td>—<\/td>/.test(namelessHtml),
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
    /<td class="source-cell">—<\/td>/.test(recordedFilenameHtml),
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
    "  location: { hash: '#progress/1700000000' },",
    "  CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },",
    "};",
    "let importsCache = [];",
    "let importEvents = {};",
    "let currentGuideSource = null;",
    "const sourceMetadataByName = {};",
  ].join('\n'), context);
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
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
    "  location: { hash: '#progress/1700000000' },",
    "  CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },",
    "};",
    "let importEvents = {};",   // a reload empties this
    "let currentGuideSource = null;",
    "const sourceMetadataByName = {};",
    "let importsCache = [{ timestamp: '1700000000', status: 'success', source_display: 'Images', entries_written: 1, entities_seeded: 0 }];",
  ].join('\n'), context);
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
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
  installRealEscapeHtml(context);
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
  installRealEscapeHtml(context);
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
  installRealEscapeHtml(context);
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
  installRealEscapeHtml(context);
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
  installRealEscapeHtml(context);
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
  // unavailable_description is a server field that can carry a provider's own
  // error text, so the panel states the fact in our own words instead of
  // repeating it. (This assertion previously required the server string to be
  // rendered; the surface it asserts is now the opposite one.)
  assert.ok(!guideSteps.innerHTML.includes('metadata corrupted'), 'the server description is not used as owner copy');
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
  const location = new QueuedHashLocation('');
  context.window = {
    location,
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
  installRealEscapeHtml(context);
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
  // The guard this test used to carry never ran: nothing in the harness had set
  // context._hashchange, because the workspace registers its listener at the top
  // level of the script, outside any function the harness extracts. The real
  // handler is wired to the real queued event instead.
  vm.runInContext(functionSource(workspace, 'handleHashChange'), context);
  context.navigatingHash = false;
  location.onhashchange = () => context.handleHashChange();

  await vm.runInContext("navigateTo('progress/1700000001')", context);
  await drain(location);

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
  installRealEscapeHtml(context);
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
  installRealEscapeHtml(context);
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
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);

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

// The dedup flag navigateTo raises has to be consumed by the handler, because
// hashchange is queued: a flag dropped on the line after the assignment is
// already down when the handler reads it, and every programmatic navigation ran
// its work a second time.
async function runHashChangeDedupConsumesTheFlag() {
  const context = vm.createContext({ console, Promise });
  const location = new QueuedHashLocation('');
  const guideRuns = [];
  context.window = { location };
  context.document = { getElementById: () => null, querySelector: () => null };
  context.currentGuideSource = null;
  context.navigatingHash = false;
  context.loadGuidedFlow = async (name) => { guideRuns.push(name); };
  context.showProgressView = () => {};
  context.reconcileImportState = async () => {};

  vm.runInContext(functionSource(workspace, 'navigateTo'), context);
  vm.runInContext(functionSource(workspace, 'handleHashChange'), context);
  location.onhashchange = () => context.handleHashChange();

  await vm.runInContext("navigateTo('guide/claude')", context);
  await drain(location);
  assert.deepStrictEqual(
    guideRuns,
    ['claude'],
    'a programmatic navigation loads the guided flow once, not again when its own hashchange lands'
  );

  // An assignment that does not change the fragment fires no event at all, so
  // the flag must not be left raised: the owner's next hash change would be
  // swallowed by it.
  await vm.runInContext("navigateTo('guide/claude')", context);
  await drain(location);
  assert.deepStrictEqual(
    guideRuns,
    ['claude', 'claude'],
    'navigating to the view already on screen still renders it'
  );

  location.hash = '#guide/plaud';
  await drain(location);
  assert.deepStrictEqual(
    guideRuns,
    ['claude', 'claude', 'plaud'],
    'an owner-driven hash change is acted on, never eaten by a dedup flag left raised'
  );
  cases += 1;
}

// The browser stores a normalised fragment, so navigating twice to a view whose
// id carries a space (the quick flow's timestamp field is free text and flows
// into progress/<ts>) assigns a value that reads back identical: no event fires,
// and a flag left raised swallows the owner's next navigation.
async function runNavigateToDropsTheFlagWhenTheFragmentDoesNotChange() {
  const context = vm.createContext({ console, Promise });
  const location = new QueuedHashLocation('');
  const seen = [];
  context.window = { location };
  context.document = { getElementById: () => null, querySelector: () => null };
  context.currentGuideSource = null;
  context.navigatingHash = false;
  context.loadGuidedFlow = async (name) => { seen.push(`guide:${name}`); };
  context.showProgressView = (id) => { seen.push(`progress:${id}`); };
  context.reconcileImportState = async () => {};

  vm.runInContext(functionSource(workspace, 'navigateTo'), context);
  vm.runInContext(functionSource(workspace, 'handleHashChange'), context);
  location.onhashchange = () => context.handleHashChange();

  await vm.runInContext("navigateTo('progress/20260919 1')", context);
  await drain(location);
  assert.strictEqual(
    location.hash,
    '#progress/20260919%201',
    'the browser stores the fragment percent-encoded'
  );

  // The stored hash no longer looks like the one navigateTo would write, so the
  // guard lets it through and the assignment changes nothing.
  await vm.runInContext("navigateTo('progress/20260919 1')", context);
  await drain(location);

  const before = seen.length;
  location.hash = '#guide/claude';
  await drain(location);
  assert.deepStrictEqual(
    seen.slice(before),
    ['guide:claude'],
    'the owner\'s next navigation is still acted on after an assignment that changed nothing'
  );

  // The same holds for the other characters a browser rewrites.
  for (const id of ['a"b', 'a<b>c', 'a`b', 'dagbok-ø']) {
    const runs = [];
    const scoped = vm.createContext({ console, Promise });
    const scopedLocation = new QueuedHashLocation('');
    scoped.window = { location: scopedLocation };
    scoped.document = { getElementById: () => null, querySelector: () => null };
    scoped.currentGuideSource = null;
    scoped.navigatingHash = false;
    scoped.loadGuidedFlow = async (name) => { runs.push(name); };
    scoped.showProgressView = () => {};
    scoped.reconcileImportState = async () => {};
    vm.runInContext(functionSource(workspace, 'navigateTo'), scoped);
    vm.runInContext(functionSource(workspace, 'handleHashChange'), scoped);
    scopedLocation.onhashchange = () => scoped.handleHashChange();

    await vm.runInContext(`navigateTo(${JSON.stringify(`progress/${id}`)})`, scoped);
    await drain(scopedLocation);
    await vm.runInContext(`navigateTo(${JSON.stringify(`progress/${id}`)})`, scoped);
    await drain(scopedLocation);
    scopedLocation.hash = '#guide/plaud';
    await drain(scopedLocation);
    assert.deepStrictEqual(runs, ['plaud'], `a normalised ${id} does not leave the dedup flag raised`);
  }
  cases += 1;
}

// navigateTo writes the hash after its awaits. An owner who leaves while a read
// is in flight -- "import another source" goes to the grid -- must not be pulled
// back by the navigation that was already on its way out.
async function runNavigateToDoesNotReassertAStaleHash() {
  const context = vm.createContext({ console, Promise });
  const location = new QueuedHashLocation('#guide/image');
  let releaseRead;
  const readInFlight = new Promise((resolve) => { releaseRead = resolve; });
  context.window = { location, _guidedClientItemId: 'item-1' };
  context.document = { getElementById: () => null, querySelector: () => null };
  context.currentGuideSource = 'image';
  context.currentGuidedFile = null;
  context.currentGuidedSaved = null;
  context.navigatingHash = false;
  context.loadGuidedFlow = async () => {};
  context.showProgressView = () => {};
  context.reconcileImportState = async () => { await readInFlight; };

  vm.runInContext(functionSource(workspace, 'navigateTo'), context);
  vm.runInContext(functionSource(workspace, 'showGrid'), context);
  vm.runInContext(functionSource(workspace, 'handleHashChange'), context);
  location.onhashchange = () => context.handleHashChange();

  const pending = vm.runInContext("navigateTo('progress/1700000001')", context);
  // The owner does not wait for the read: they take the way back to the grid.
  vm.runInContext('showGrid()', context);
  assert.strictEqual(location.hash, '', 'the owner is on the grid');

  releaseRead();
  await pending;
  await drain(location);

  assert.strictEqual(
    location.hash,
    '',
    'the navigation that was mid-await does not write its hash over the owner\'s'
  );
  cases += 1;
}

// The panel repaints from what was stored, not from the raw event. An importer
// event that carries null for a count it cannot answer this tick had its nulls
// dropped on the way into the store and then put straight back for the repaint,
// blanking the "4/10 items" the owner was watching. Both call sites are
// exercised: with the import's row on screen, and without it.
function runRepaintSeesTheFactsThatWereStored() {
  for (const scenario of [{ name: 'no row on screen', row: false }, { name: 'row on screen', row: true }]) {
    const guideSteps = new Element();
    const statusCell = new Element();
    const context = vm.createContext({ console });
    const row = {
      classList: { add() {}, remove() {} },
      dataset: {},
      querySelector: (selector) => (selector === '.status-cell' ? statusCell : null),
    };
    context.window = {
      location: { hash: '#progress/1700000075' },
      CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
    };
    context.document = {
      getElementById: (id) => (id === 'guideSteps' ? guideSteps : null),
      querySelector: () => (scenario.row ? row : null),
    };
    context.importEvents = {
      '1700000075': {
        import_id: '1700000075', event: 'status', status: 'running', stage: 'writing',
        generation: 2, items_total: 10, items_processed: 4,
      },
    };
    context.importGenerationFloor = { '1700000075': 2 };
    context.sourceMetadataByName = {};
    context.currentGuideSource = null;
    context.getImportById = () => null;
    installRealEscapeHtml(context);
    assertEscaperIsReal(context);
    context.humanStageName = (s) => s;
    context.formatElapsed = () => '0s';
    context.formatDateRange = () => '';
    context.clearPendingImport = () => {};
    context.trackPendingImport = () => {};
    context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);

    vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
    vm.runInContext(functionSource(workspace, 'formatProgressStats'), context);
    vm.runInContext(functionSource(workspace, 'renderProgressStats'), context);
    vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
    vm.runInContext(functionSource(workspace, 'showProgressView'), context);
    vm.runInContext(functionSource(workspace, 'refreshInlineProgress'), context);
    vm.runInContext(functionSource(workspace, 'updateImportRow'), context);

    vm.runInContext("showProgressView('1700000075')", context);
    assert.ok(
      guideSteps.innerHTML.includes('4/10 items'),
      `the panel starts on the progress it was given (${scenario.name})`
    );

    // The next importer tick knows the stage but not the item counts.
    vm.runInContext(
      "updateImportRow('1700000075', { import_id: '1700000075', event: 'status', status: 'running',"
      + ' stage: \'writing\', generation: 2, items_total: null, items_processed: null })',
      context
    );

    assert.strictEqual(
      context.importEvents['1700000075'].items_total,
      10,
      `the store keeps the count the event could not answer (${scenario.name})`
    );
    assert.ok(
      guideSteps.innerHTML.includes('4/10 items'),
      `and the repaint shows what the store kept, not the nulls that were dropped (${scenario.name})`
    );
    cases += 1;
  }
}

// repeatSource puts focus on the drop area the owner is about to use. The
// re-entrant loadGuidedFlow that the stale dedup flag allowed rewrote
// #guideSteps straight afterwards, detaching the node that had just taken it --
// a real Chrome reported document.activeElement as null after "import another
// image".
async function runRepeatSourceFocusSurvivesTheQueuedHashChange() {
  const variants = [
    { name: 'guided drop area', drop: true, path: true, quick: false, expected: 'guidedDropArea' },
    { name: 'guided path input', drop: false, path: true, quick: false, expected: 'guidedPathInput' },
    { name: 'quick drop area', drop: false, path: false, quick: true, expected: 'dropArea' },
  ];

  for (const variant of variants) {
    const context = vm.createContext({ console, Promise });
    const location = new QueuedHashLocation('#progress/1700000000');
    const guideSteps = new Element();
    const guide = new Element();
    const doc = { activeElement: null, querySelector: () => null };
    const guideRuns = [];
    let dropArea = null;
    let pathInput = null;
    let quickDropArea = null;

    doc.getElementById = (id) => {
      if (id === 'guidedDropArea') return dropArea;
      if (id === 'guidedPathInput') return pathInput;
      if (id === 'dropArea') return quickDropArea;
      if (id === 'guideSteps') return guideSteps;
      if (id === 'importGuide' || id === 'importGrid') return guide;
      return null;
    };
    context.document = doc;
    context.window = { location, _guidedClientItemId: 'item-1' };
    context.currentGuidedFile = null;
    context.currentGuidedSaved = null;
    context.currentGuideSource = null;
    context.navigatingHash = false;
    context.importEvents = { '1700000000': { import_id: '1700000000' } };
    context.clearPendingImport = () => {};
    context.showProgressView = () => {};
    context.reconcileImportState = async () => {};
    context.loadGuidedFlow = async (name) => {
      guideRuns.push(name);
      await new Promise((resolve) => setImmediate(resolve));
      // A guide render replaces everything inside #guideSteps. Whatever was
      // focused in there is detached, and a browser takes focus off a node it
      // removes.
      if (dropArea) dropArea.detach();
      if (pathInput) pathInput.detach();
      if (quickDropArea) quickDropArea.detach();
      dropArea = variant.drop ? new FocusableElement('guidedDropArea', doc) : null;
      pathInput = variant.path ? new FocusableElement('guidedPathInput', doc) : null;
      quickDropArea = variant.quick ? new FocusableElement('dropArea', doc) : null;
    };

    vm.runInContext(functionSource(workspace, 'navigateTo'), context);
    vm.runInContext(functionSource(workspace, 'handleHashChange'), context);
    vm.runInContext(functionSource(workspace, 'repeatSource'), context);
    location.onhashchange = () => context.handleHashChange();

    await vm.runInContext("repeatSource('claude')", context);
    await drain(location);

    assert.ok(doc.activeElement, `focus does not fall to the body after the queued hashchange (${variant.name})`);
    assert.strictEqual(
      doc.activeElement.id,
      variant.expected,
      `repeatSource leaves focus on the ${variant.name}`
    );
    assert.deepStrictEqual(guideRuns, ['claude'], `the guided flow renders once (${variant.name})`);
    cases += 1;
  }
}

// authoritative means "canonical for the generation gate" and nothing else. A
// read that says running does not un-complete a completed import: with a
// completed record in hand, one reconcile on the way into a progress view was
// rewriting it to running and blanking the counts the owner had been shown.
async function runAuthoritativeRunningReadKeepsACompletedImportComplete() {
  const guideSteps = new Element();
  const context = vm.createContext({ console, Promise });
  context.window = {
    location: { hash: '#progress/1700000060' },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
    apiJson: async () => ({
      import_id: '1700000060',
      status: 'running',
      generation: 1,
      entries_written: null,
      entities_seeded: null,
      total_files_created: null,
      duration_ms: null,
      date_range: null,
      error: null,
      error_stage: null,
    }),
  };
  context.document = {
    getElementById: (id) => (id === 'guideSteps' ? guideSteps : null),
    querySelector: () => null,
  };
  context.importEvents = {
    '1700000060': {
      import_id: '1700000060',
      event: 'completed',
      status: 'success',
      entries_written: 60,
      entities_seeded: 3,
      duration_ms: 4000,
      generation: 1,
      source_display: 'Images',
    },
  };
  context.importGenerationFloor = { '1700000060': 1 };
  context.sourceMetadataByName = {};
  context.currentGuideSource = null;
  context.getImportById = () => null;
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '';
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
  vm.runInContext(functionSource(workspace, 'reconcileImportState'), context);

  vm.runInContext("showProgressView('1700000060')", context);
  assert.ok(guideSteps.innerHTML.includes('import complete'), 'the panel opens on the completed import');
  assert.ok(
    guideSteps.innerHTML.includes('<strong>entries written:</strong> 60'),
    'and it names what the import wrote'
  );

  await vm.runInContext("reconcileImportState('1700000060')", context);

  const event = context.importEvents['1700000060'];
  assert.strictEqual(event.event, 'completed', 'a canonical running read does not un-complete a completed import');
  assert.strictEqual(event.status, 'success', 'the terminal status survives the canonical read');
  assert.strictEqual(event.entries_written, 60, 'and the counts survive with it');
  assert.ok(guideSteps.innerHTML.includes('import complete'), 'the open panel still reads as complete');
  assert.ok(
    guideSteps.innerHTML.includes('<strong>entries written:</strong> 60'),
    'the open panel still names the entries'
  );
  cases += 1;
}

// A read of an import that is still going carries null for everything it cannot
// answer yet. Those nulls must not land on top of facts already known.
function runRunningReadKeepsKnownFacts() {
  const context = vm.createContext({ console });
  context.document = { querySelector: () => null, getElementById: () => null };
  context.importEvents = {
    '1700000070': {
      import_id: '1700000070', event: 'status', status: 'running', stage: 'writing',
      entries_written: 12, generation: 2,
    },
  };
  context.importGenerationFloor = { '1700000070': 2 };
  context.refreshInlineProgress = () => {};
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);

  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);

  vm.runInContext(
    "updateImportRow('1700000070', { import_id: '1700000070', event: 'status', status: 'running',"
    + ' generation: 2, entries_written: null, entities_seeded: null, date_range: null },'
    + ' { authoritative: true })',
    context
  );

  const event = context.importEvents['1700000070'];
  assert.strictEqual(event.entries_written, 12, 'an unfinished read does not blank a count it cannot answer yet');
  assert.strictEqual(event.status, 'running', 'the running read itself still lands');
  cases += 1;
}

// Item 43: a "check status" read is the canonical answer for an import, so it
// lands even when it carries no generation at all -- which is what the detail
// route returns for an import whose state it cannot reconstruct.
async function runCanonicalReadOutranksTheGenerationFloor() {
  const guideSteps = new Element();
  const statusCell = new Element();
  const context = vm.createContext({ console, Promise });
  const row = {
    classList: { add() {}, remove() {} },
    dataset: {},
    querySelector: (selector) => (selector === '.status-cell' ? statusCell : null),
  };
  context.window = {
    location: { hash: '#progress/1700000001' },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
    apiJson: async () => ({
      import_id: '1700000001',
      status: 'unavailable',
      generation: null,
      unavailable_description: 'metadata could not be read',
    }),
  };
  context.document = {
    getElementById: (id) => (id === 'guideSteps' ? guideSteps : null),
    querySelector: (selector) => (selector.startsWith('tr[') ? row : null),
  };
  context.importEvents = {
    '1700000001': {
      import_id: '1700000001', event: 'status', status: 'running', stage: 'writing', generation: 1,
    },
  };
  context.importGenerationFloor = { '1700000001': 1 };
  context.sourceMetadataByName = {};
  context.currentGuideSource = null;
  context.getImportById = () => null;
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '';
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
  vm.runInContext(functionSource(workspace, 'reconcileImportState'), context);

  await vm.runInContext("reconcileImportState('1700000001')", context);

  assert.ok(
    statusCell.innerHTML.includes('<span class="import-status unavailable">status unavailable</span>'),
    'the row goes from running to unavailable on the canonical read'
  );
  assert.ok(!statusCell.innerHTML.includes('import-status running'), 'and stops claiming to be running');
  assert.ok(
    guideSteps.innerHTML.includes('import status unavailable'),
    'the open panel says the status is unavailable'
  );
  assert.ok(
    !guideSteps.innerHTML.includes('metadata could not be read'),
    'and states it in our words rather than repeating the server description'
  );
  assert.strictEqual(
    context.importGenerationFloor['1700000001'],
    1,
    'a read with no generation does not move the generation floor'
  );
  cases += 1;
}

// isTerminalState is what the terminal guard reads, so a state missing from it
// is a state a later running update can overwrite. unconfirmed and unavailable
// are ends of the road as much as completed and failed are.
function runTerminalStateCoversEveryEndState() {
  const context = vm.createContext({ console });
  context.document = { querySelector: () => null, getElementById: () => null };
  context.refreshInlineProgress = () => {};
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.importGenerationFloor = {};
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);
  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);

  for (const state of ['completed', 'success', 'failed', 'error', 'unconfirmed', 'unavailable']) {
    assert.strictEqual(
      vm.runInContext(`isTerminalState(${JSON.stringify(state)})`, context),
      true,
      `${state} is an end state`
    );
  }
  for (const state of ['running', 'started', 'status', 'pending', undefined]) {
    assert.strictEqual(
      vm.runInContext(`isTerminalState(${JSON.stringify(state)})`, context),
      false,
      `${state} is not an end state`
    );
  }

  // And the guard reads it: a status-only unconfirmed answer is terminal, so it
  // lands on a completed record instead of being turned away as a downgrade.
  context.importEvents = {
    '1700000085': { import_id: '1700000085', event: 'completed', status: 'success', entries_written: 60 },
  };
  vm.runInContext(
    "updateImportRow('1700000085', { import_id: '1700000085', status: 'unconfirmed', entries_written: null })",
    context
  );
  assert.strictEqual(
    context.importEvents['1700000085'].status,
    'unconfirmed',
    'an unconfirmed answer is terminal, so it is allowed to land on a terminal record'
  );
  cases += 1;
}

// The floor is the memory that survives a record without a generation on it --
// the one startGuidedImport writes when an import starts. If nothing raises it,
// a delayed event from an older run walks straight back in.
function runGenerationFloorIsRaisedByEveryEvent() {
  const context = vm.createContext({ console });
  context.document = { querySelector: () => null, getElementById: () => null };
  context.importEvents = {};
  context.importGenerationFloor = {};
  context.refreshInlineProgress = () => {};
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);
  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);

  vm.runInContext(
    "updateImportRow('1700000086', { import_id: '1700000086', event: 'status', status: 'running',"
    + " stage: 'writing', generation: 3 })",
    context
  );
  assert.strictEqual(context.importGenerationFloor['1700000086'], 3, 'an event raises the floor to its generation');

  // startGuidedImport writes a record with no generation on it. The floor is
  // the only thing that still knows a generation 3 has been seen.
  context.importEvents['1700000086'] = {
    import_id: '1700000086', event: 'started', stage: 'initialization',
  };
  vm.runInContext(
    "updateImportRow('1700000086', { import_id: '1700000086', event: 'completed', status: 'success',"
    + ' generation: 2, entries_written: 999 })',
    context
  );
  assert.strictEqual(
    context.importEvents['1700000086'].event,
    'started',
    'a delayed event from an older generation is turned away by the floor alone'
  );
  assert.strictEqual(
    context.importEvents['1700000086'].entries_written,
    undefined,
    'and it brings none of its facts with it'
  );
  cases += 1;
}

// One payload, every attribute and text node an import writes from server- or
// URL-derived text. Each of these was an escape nothing would have noticed the
// loss of.
function runEveryOwnerSinkEscapes() {
  // --- the progress panel: failed error text, stalled import id ---
  const guideSteps = new Element();
  const panelContext = vm.createContext({ console });
  panelContext.document = { getElementById: (id) => (id === 'guideSteps' ? guideSteps : null) };
  panelContext.window = {
    location: { hash: `#progress/${ESCAPE_PAYLOAD}` },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  panelContext.sourceMetadataByName = {};
  panelContext.currentGuideSource = null;
  panelContext.importEvents = {};
  panelContext.getImportById = () => null;
  installRealEscapeHtml(panelContext);
  assertEscaperIsReal(panelContext);
  panelContext.humanStageName = (s) => s;
  panelContext.formatElapsed = () => '0s';
  panelContext.renderProgressStats = () => '';
  panelContext.formatDateRange = () => '';
  vm.runInContext(functionSource(workspace, 'formatStatValue'), panelContext);
  vm.runInContext(functionSource(workspace, 'showProgressView'), panelContext);

  vm.runInContext(
    `showProgressView('1700000087', { import_id: '1700000087', event: 'error', status: 'failed',`
    + ` error: ${JSON.stringify(ESCAPE_PAYLOAD)} })`,
    panelContext
  );
  assert.ok(
    guideSteps.innerHTML.includes(`<strong>error:</strong> ${ESCAPED_PAYLOAD}`),
    'the failed panel escapes the error it was handed'
  );
  assert.ok(!guideSteps.innerHTML.includes('<b>'), 'and no payload markup reaches the failed panel');

  vm.runInContext(
    `showProgressView(${JSON.stringify(ESCAPE_PAYLOAD)}, { import_id: ${JSON.stringify(ESCAPE_PAYLOAD)},`
    + ' stalled: true })',
    panelContext
  );
  assert.ok(
    guideSteps.innerHTML.includes(`<strong>import id:</strong> ${ESCAPED_PAYLOAD}`),
    'the stalled panel escapes the import id'
  );
  assert.ok(!guideSteps.innerHTML.includes('<b>'), 'and no payload markup reaches the stalled panel');

  // --- the live row: started stage, status detail, error detail, gap chip ---
  const statusCell = new Element();
  const rowContext = vm.createContext({ console });
  const row = {
    classList: { add() {}, remove() {} },
    dataset: {},
    querySelector: (selector) => (selector === '.status-cell' ? statusCell : null),
  };
  rowContext.document = { querySelector: () => row, getElementById: () => null };
  rowContext.window = { location: { hash: '' } };
  rowContext.importEvents = {};
  rowContext.importGenerationFloor = {};
  rowContext.sourceMetadataByName = {};
  rowContext.refreshInlineProgress = () => {};
  rowContext.clearPendingImport = () => {};
  rowContext.trackPendingImport = () => {};
  rowContext.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);
  installRealEscapeHtml(rowContext);
  assertEscaperIsReal(rowContext);
  vm.runInContext(`const STAGE_NAMES = ${arrowBody(workspace, 'const STAGE_NAMES =')};`, rowContext);
  for (const name of [
    'capitalizeStage', 'humanStageName', 'formatElapsed', 'formatProgressStats',
    'renderSourceDisplay', 'formatImportStats', 'isTerminalState', 'updateImportRow',
  ]) {
    vm.runInContext(functionSource(workspace, name), rowContext);
  }

  vm.runInContext(
    `updateImportRow('r1', { import_id: 'r1', event: 'started', stage: ${JSON.stringify(ESCAPE_PAYLOAD)} })`,
    rowContext
  );
  assert.ok(!statusCell.innerHTML.includes('<b>'), 'the started row escapes its stage name');
  assert.ok(statusCell.innerHTML.includes('&lt;b&gt;'), 'and renders it as text');

  vm.runInContext(
    `updateImportRow('r2', { import_id: 'r2', event: 'status', stage: 'writing', items_total: 2,`
    + ` items_processed: 1, earliest_date: ${JSON.stringify(ESCAPE_PAYLOAD)} })`,
    rowContext
  );
  assert.ok(!statusCell.innerHTML.includes('<b>'), 'the status row escapes its progress detail');
  assert.ok(statusCell.innerHTML.includes('&lt;b&gt;'), 'and renders the detail as text');

  vm.runInContext(
    `updateImportRow('r3', { import_id: 'r3', event: 'error', error: ${JSON.stringify(ESCAPE_PAYLOAD)} })`,
    rowContext
  );
  assert.ok(!statusCell.innerHTML.includes('<b>'), 'the error row escapes the error text');
  assert.ok(statusCell.innerHTML.includes('&lt;b&gt;'), 'and renders the error as text');

  vm.runInContext(
    "updateImportRow('r4', { import_id: 'r4', event: 'status', status: 'running', stage: 'writing', has_gaps: true })",
    rowContext
  );
  assert.ok(
    statusCell.innerHTML.includes('contains gaps'),
    'a live row that reports gaps still shows the chip that says so'
  );

  // --- the history table: row error detail, source cell, filter options ---
  const historyContext = vm.createContext({ console });
  vm.runInContext([
    "const window = {",
    "  JournalFormat: { timestamp: () => '2026-07-22, 7:30 PM', day: (value) => value },",
    "};",
    "const sourceIconSvgByName = {};",
    "const sourceMetadataByName = {};",
    "let currentSourceFilter = '';",
    `let cachedSources = [{ name: ${JSON.stringify(ESCAPE_PAYLOAD)}, display_name: ${JSON.stringify(ESCAPE_PAYLOAD)} }];`,
  ].join('\n'), historyContext);
  installRealEscapeHtml(historyContext);
  assertEscaperIsReal(historyContext);
  for (const name of [
    'capitalizeStage', 'renderSourceDisplay', 'formatImportStats', 'renderImportRow', 'buildHistoryHeader',
  ]) {
    vm.runInContext(functionSource(workspace, name), historyContext);
  }

  const failedRowHtml = vm.runInContext(
    `renderImportRow({ timestamp: 'h1', status: 'failed', imported_at: 1700000000,`
    + ` error: ${JSON.stringify(ESCAPE_PAYLOAD)}, error_stage: ${JSON.stringify(ESCAPE_PAYLOAD)} })`,
    historyContext
  );
  assert.ok(!failedRowHtml.includes('<b>'), 'the history row escapes the error it reports');
  assert.ok(failedRowHtml.includes('&lt;b&gt;'), 'and renders it as text');

  const sourceHtml = vm.runInContext(
    `renderSourceDisplay('plaud', ${JSON.stringify(ESCAPE_PAYLOAD)})`,
    historyContext
  );
  assert.ok(sourceHtml.includes(ESCAPED_PAYLOAD), 'the source cell escapes its display text');
  assert.ok(!sourceHtml.includes('<b>'), 'and no payload markup reaches the source cell');

  const headerHtml = vm.runInContext('buildHistoryHeader(1, 1)', historyContext);
  assert.ok(
    headerHtml.includes(`<option value="${ESCAPED_PAYLOAD}"`),
    'the source filter escapes the option value'
  );
  assert.ok(!headerHtml.includes('<b>'), 'and no payload markup reaches the filter');
  cases += 1;
}

// A clean success owes the owner nothing about failures. kvRow rendered a dash
// for every absent key, so a finished import listed "failed at -", "error -" and
// five more lines of things that had not happened.
function runDetailDropsFactsThatDoNotApply() {
  const ImportDetail = loadImportDetailModule();

  const cleanSuccess = {
    status: 'success',
    import_json: {
      original_filename: 'note.pdf',
      file_size: 2048,
      mime_type: 'application/pdf',
      upload_datetime: '2026-07-06T12:09:03',
      detected_timestamp: '20260706_120903',
      user_timestamp: '20260706_120903',
      setting: 'lunch with joe',
    },
    imported_json: {
      source_display: 'Images',
      target_day: '20260706',
      date_range: ['20260706', '20260706'],
      entries_written: 60,
      entities_seeded: 3,
      total_files_created: 61,
      processing_completed: '2026-07-06T12:19:03',
    },
  };
  const successHtml = ImportDetail.renderDetail(cleanSuccess);
  for (const label of ['unavailable description', 'unavailable pages', 'failed at', 'failed stage', 'error']) {
    assert.ok(!successHtml.includes(`<dt>${label}</dt>`), `a clean success says nothing about "${label}"`);
  }
  assert.ok(!successHtml.includes('<dd>—</dd>'), 'a clean success renders no dashes at all');
  assert.ok(successHtml.includes('<dt>entries</dt><dd>60 entries</dd>'), 'the facts it does have are still rendered');

  const successWithoutCounts = {
    status: 'success',
    import_json: { original_filename: 'note.pdf' },
    imported_json: { processing_completed: '2026-07-06T12:19:03' },
  };
  const sparseHtml = ImportDetail.renderDetail(successWithoutCounts);
  assert.ok(
    sparseHtml.includes('<dt>entries</dt><dd>—</dd>'),
    'a count the status makes meaningful renders the dash: the blank is the answer'
  );
  assert.ok(!sparseHtml.includes('<dt>mime type</dt>'), 'a fact the upload never recorded is still dropped');
  assert.ok(!sparseHtml.includes('<dt>failed at</dt>'), 'and a success still says nothing about failing');

  const unavailableHtml = ImportDetail.renderDetail({ status: 'unavailable', import_json: {}, imported_json: {} });
  for (const label of ['entries', 'entities', 'files']) {
    assert.ok(
      unavailableHtml.includes(`<dt>${label}</dt><dd>—</dd>`),
      `an unavailable import shows the ${label} it cannot account for`
    );
  }
  for (const label of ['failed at', 'failed stage', 'mime type', 'file size', 'setting', 'unavailable pages']) {
    assert.ok(!unavailableHtml.includes(`<dt>${label}</dt>`), `and nothing about "${label}"`);
  }

  const failedHtml = ImportDetail.renderDetail({
    status: 'failed', import_json: {}, imported_json: {}, error: 'disk full',
  });
  assert.ok(failedHtml.includes('<dt>error</dt><dd>disk full</dd>'), 'a failure names its error');
  assert.ok(failedHtml.includes('<dt>failed at</dt><dd>—</dd>'), 'and owes the owner the time, even unknown');
  assert.ok(!failedHtml.includes('<dt>entries</dt>'), 'a failure does not report counts it never produced');

  assert.ok(
    ImportDetail.kvRow('status', ESCAPE_PAYLOAD).includes(`<dd>${ESCAPED_PAYLOAD}</dd>`),
    'the detail rows escape what they render'
  );
  cases += 1;
}

// One page is a page, not "1 pages".
function runDetailPageCountMatchesItsNumber() {
  const ImportDetail = loadImportDetailModule();

  const oneHtml = ImportDetail.renderDetail({
    status: 'success', import_json: {}, imported_json: {}, unavailable_pages: 1,
  });
  assert.ok(
    oneHtml.includes('<dd>1 page could not be extracted</dd>'),
    'a single unavailable page reads as one page'
  );
  assert.ok(!oneHtml.includes('1 pages'), '"1 pages" is gone');

  const manyHtml = ImportDetail.renderDetail({
    status: 'success', import_json: {}, imported_json: {}, unavailable_pages: 4,
  });
  assert.ok(
    manyHtml.includes('<dd>4 pages could not be extracted</dd>'),
    'more than one reads as pages'
  );

  const noneHtml = ImportDetail.renderDetail({
    status: 'success', import_json: {}, imported_json: {}, unavailable_pages: 0,
  });
  assert.ok(
    !noneHtml.includes('<dt>unavailable pages</dt>'),
    'an import that lost no pages says nothing about pages'
  );
  cases += 1;
}

// unavailable_description is a server field. It can carry a provider's own
// error text, and escaping it makes it safe to put on the page without making
// it something we are willing to say to an owner.
function runUnavailablePanelNeverEchoesTheServer() {
  const guideSteps = new Element();
  const context = vm.createContext({ console });
  const hostile = 'ORA-01017: invalid username/password; logon denied <b>at</b> pg-7';
  context.document = { getElementById: (id) => (id === 'guideSteps' ? guideSteps : null) };
  context.window = {
    location: { hash: '#progress/1700000088' },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  context.sourceMetadataByName = {};
  context.currentGuideSource = null;
  context.importEvents = {};
  context.getImportById = () => ({
    timestamp: '1700000088', status: 'unavailable', unavailable_description: hostile,
  });
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '';

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);

  vm.runInContext(
    `showProgressView('1700000088', { import_id: '1700000088', status: 'unavailable',`
    + ` unavailable_description: ${JSON.stringify(hostile)} })`,
    context
  );

  assert.ok(
    guideSteps.innerHTML.includes('import status unavailable'),
    'the panel says what we can stand behind'
  );
  assert.ok(!guideSteps.innerHTML.includes('ORA-01017'), 'and never repeats the server text');
  assert.ok(!guideSteps.innerHTML.includes('logon denied'), 'not any part of it');
  assert.ok(guideSteps.innerHTML.includes('check status'), 'the way on is still offered');

  // The same holds when the panel falls back to the import list rather than a
  // live event.
  vm.runInContext("showProgressView('1700000088')", context);
  assert.ok(!guideSteps.innerHTML.includes('ORA-01017'), 'including when the description comes from the list');
  cases += 1;
}

// A source name is server data, and an inline onclick puts it in a script
// context where one escape is all that stands between the page and the string.
// The card carries it as data and the delegated listener reads it.
async function runSourceCardsCarryNoInlineHandler() {
  const grid = new Element();
  const context = vm.createContext({ console, Promise });
  context.document = { getElementById: (id) => (id === 'importGrid' ? grid : null) };
  context.window = {};
  context.cachedSources = [];
  context.sourceMetadataByName = {};
  context.sourceIconSvgByName = {};
  context.fetch = async () => ({
    json: async () => ({
      items: [
        { name: ESCAPE_PAYLOAD, display_name: 'Odd source', description: 'a source with a name like that' },
        { name: 'quick', display_name: 'Quick import', description: 'paste or drop' },
      ],
    }),
  });
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
  vm.runInContext(functionSource(workspace, 'loadSourceGrid'), context);
  await vm.runInContext('loadSourceGrid()', context);

  assert.ok(!/onclick=/.test(grid.innerHTML), 'no source card carries an inline handler');
  assert.ok(
    grid.innerHTML.includes(`data-import-source="${ESCAPED_PAYLOAD}"`),
    'the source name is escaped into a data attribute'
  );
  assert.ok(grid.innerHTML.includes('data-import-quick="true"'), 'the quick card is marked as itself');
  assert.ok(!grid.innerHTML.includes('<b>'), 'no payload markup reaches the grid');

  // And the delegated listener acts on it, so the card is not inert.
  const clickContext = vm.createContext({ console });
  const navigated = [];
  let quickOpened = 0;
  clickContext.navigateTo = (view) => { navigated.push(view); };
  clickContext.showQuickImport = () => { quickOpened += 1; };
  clickContext.reconcileImportState = () => {};
  clickContext.repeatSource = () => {};
  clickContext.window = { location: { href: '' } };
  clickContext.document = { addEventListener: () => {} };
  vm.runInContext(
    `function handleDocumentClick(event) ${arrowBody(workspace, "document.addEventListener('click', event =>")}`,
    clickContext
  );

  const cardTarget = (attributes) => ({
    closest: (selector) => {
      const wanted = selector.split(',').map((part) => part.trim().replace(/^\[|\]$/g, ''));
      return wanted.some((name) => Object.prototype.hasOwnProperty.call(attributes, name))
        ? {
          hasAttribute: (name) => Object.prototype.hasOwnProperty.call(attributes, name),
          getAttribute: (name) => (Object.prototype.hasOwnProperty.call(attributes, name) ? attributes[name] : null),
        }
        : null;
    },
  });

  clickContext.sourceEvent = { target: cardTarget({ 'data-import-source': ESCAPE_PAYLOAD }), stopPropagation() {} };
  vm.runInContext('handleDocumentClick(sourceEvent)', clickContext);
  assert.deepStrictEqual(
    navigated,
    [`guide/${ESCAPE_PAYLOAD}`],
    'clicking a source card opens that source, name and all'
  );

  clickContext.quickEvent = { target: cardTarget({ 'data-import-quick': 'true' }), stopPropagation() {} };
  vm.runInContext('handleDocumentClick(quickEvent)', clickContext);
  assert.strictEqual(quickOpened, 1, 'and the quick card opens the quick flow');
  cases += 1;
}

// One dash for unknown across the history table. The stats column already used
// the em dash while every other column used an ASCII hyphen.
function runUnknownUsesTheEmDashInTheHistoryTable() {
  const context = vm.createContext({ console });
  vm.runInContext([
    "const window = {",
    "  JournalFormat: { timestamp: () => '2026-07-22, 7:30 PM', day: (value) => value },",
    "};",
    "const sourceIconSvgByName = {};",
    "const sourceMetadataByName = {};",
  ].join('\n'), context);
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
  vm.runInContext(functionSource(workspace, 'renderSourceDisplay'), context);
  vm.runInContext(functionSource(workspace, 'formatImportStats'), context);
  vm.runInContext(functionSource(workspace, 'capitalizeStage'), context);
  vm.runInContext(functionSource(workspace, 'renderImportRow'), context);

  const bareRow = { timestamp: 'b1', status: 'running', imported_at: 1700000000 };
  const html = vm.runInContext(`renderImportRow(${JSON.stringify(bareRow)})`, context);
  assert.ok(html.includes('<td class="nowrap">—</td>'), 'an unknown journal day is an em dash');
  assert.ok(html.includes('<td>—</td>'), 'an unknown file is an em dash');
  assert.ok(html.includes('<td class="source-cell">—</td>'), 'an unknown source is an em dash');
  assert.ok(html.includes('<td class="stats-cell import-stats-cell">—</td>'), 'unknown stats stay an em dash');
  assert.ok(!/>-</.test(html), 'no ASCII hyphen is left standing in for an unknown');

  assert.strictEqual(
    vm.runInContext("renderSourceDisplay('', '')", context).includes('—'),
    true,
    'the source fallback is the em dash too'
  );
  cases += 1;
}

// A measured zero is a number the import actually reported, not an unknown.
function runMeasuredZeroIsNotUnknown() {
  const guideSteps = new Element();
  const context = vm.createContext({ console });
  context.document = { getElementById: (id) => (id === 'guideSteps' ? guideSteps : null) };
  context.window = {
    location: { hash: '#progress/1700000050' },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  context.sourceMetadataByName = {};
  context.currentGuideSource = null;
  context.importEvents = {};
  context.getImportById = () => null;
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '';

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'formatImportStats'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);

  assert.strictEqual(vm.runInContext('formatStatValue(0)', context), '0', 'a measured zero renders as 0');
  assert.strictEqual(
    vm.runInContext('formatImportStats(0, 0)', context),
    '0 entries • 0 entities',
    'a measured zero is not an unknown in the history stats either'
  );

  vm.runInContext(
    "showProgressView('1700000050', { import_id: '1700000050', event: 'completed', status: 'success',"
    + ' entries_written: 0, entities_seeded: 0 })',
    context
  );
  assert.ok(
    guideSteps.innerHTML.includes('<strong>entries written:</strong> 0'),
    'an import that wrote nothing says 0, not an em dash'
  );
  assert.ok(
    !guideSteps.innerHTML.includes('<strong>entries written:</strong> —'),
    'the em dash is reserved for what is genuinely unknown'
  );
  cases += 1;
}

// An import id goes into a CSS attribute selector, so it goes through
// CSS.escape: an id carrying a quote closes the selector string early and the
// browser throws a DOMException on the junk that follows.
function runImportIdSelectorsEscapeQuotes() {
  const importId = '17000"0001';
  const statusCell = new Element();
  const context = vm.createContext({ console });
  const row = {
    classList: { add() {}, remove() {} },
    dataset: {},
    querySelector: (selector) => (selector === '.status-cell' ? statusCell : null),
  };
  context.CSS = { escape: cssEscape };
  context.document = {
    querySelector: selectorLookup({ [importId]: row }),
    getElementById: () => null,
  };
  context.window = { CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' } };
  context.importEvents = {};
  context.importGenerationFloor = {};
  context.clearPendingImport = () => {};
  context.trackPendingImport = () => {};
  context.refreshInlineProgress = () => {};
  context.reconcileImportState = () => {};
  context.humanStageName = (s) => s;
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
  context.IMPORT_ROW_EVENTS = new Set(['started', 'status', 'completed', 'error']);

  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'updateImportRow'), context);
  vm.runInContext(functionSource(workspace, 'markRowStalled'), context);

  vm.runInContext(`markRowStalled(${JSON.stringify(importId)})`, context);
  assert.ok(
    statusCell.innerHTML.includes('check status'),
    'the stalled row is still found when its id carries a quote'
  );
  assert.ok(
    statusCell.innerHTML.includes('data-import-check-status="17000&quot;0001"'),
    'and the id is escaped into the attribute it is written to'
  );

  vm.runInContext(
    `updateImportRow(${JSON.stringify(importId)}, { import_id: ${JSON.stringify(importId)},`
    + " event: 'error', status: 'failed', error: 'disk full' })",
    context
  );
  assert.ok(statusCell.innerHTML.includes('failed'), 'and the row update finds the same row');
  cases += 1;
}

// Every attribute an import writes from server- or URL-derived text, with a
// payload that would break out of each one.
function runOwnerSinksEscapeServerDerivedPayloads() {
  const rowContext = vm.createContext({ console });
  vm.runInContext([
    "const window = {",
    "  JournalFormat: { timestamp: () => '2026-07-22, 7:30 PM', day: (value) => value },",
    "};",
    "const sourceIconSvgByName = {};",
    "const sourceMetadataByName = {};",
  ].join('\n'), rowContext);
  installRealEscapeHtml(rowContext);
  assertEscaperIsReal(rowContext);
  vm.runInContext(functionSource(workspace, 'renderSourceDisplay'), rowContext);
  vm.runInContext(functionSource(workspace, 'formatImportStats'), rowContext);
  vm.runInContext(functionSource(workspace, 'capitalizeStage'), rowContext);
  vm.runInContext(functionSource(workspace, 'renderImportRow'), rowContext);

  const payloadRow = {
    timestamp: ESCAPE_PAYLOAD,
    status: 'success',
    imported_at: 1700000000,
    source_type: ESCAPE_PAYLOAD,
    source_display: 'Plaud recorder',
    entries_written: 1,
    entities_seeded: 0,
  };
  const rowHtml = vm.runInContext(`renderImportRow(${JSON.stringify(payloadRow)})`, rowContext);
  assert.ok(rowHtml.includes(`data-import-id="${ESCAPED_PAYLOAD}"`), 'data-import-id is escaped');
  assert.ok(rowHtml.includes(`data-source-type="${ESCAPED_PAYLOAD}"`), 'data-source-type is escaped');
  assert.ok(
    rowHtml.includes(`data-import-link="/app/import/${encodeURIComponent(ESCAPE_PAYLOAD)}"`),
    'the row link is percent-encoded'
  );
  assert.ok(!rowHtml.includes(`data-import-id="x"`), 'the raw quote never reaches the attribute');

  const guideSteps = new Element();
  const panelContext = vm.createContext({ console });
  panelContext.document = { getElementById: (id) => (id === 'guideSteps' ? guideSteps : null) };
  panelContext.window = {
    location: { hash: `#progress/${ESCAPE_PAYLOAD}` },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  panelContext.encodeURIComponent = encodeURIComponent;
  panelContext.sourceMetadataByName = {};
  panelContext.currentGuideSource = ESCAPE_PAYLOAD;
  panelContext.importEvents = {};
  panelContext.getImportById = () => null;
  installRealEscapeHtml(panelContext);
  assertEscaperIsReal(panelContext);
  panelContext.humanStageName = (s) => s;
  panelContext.formatElapsed = () => '0s';
  panelContext.renderProgressStats = () => '';
  panelContext.formatDateRange = () => '';

  vm.runInContext(functionSource(workspace, 'formatStatValue'), panelContext);
  vm.runInContext(functionSource(workspace, 'showProgressView'), panelContext);

  vm.runInContext(
    `showProgressView(${JSON.stringify(ESCAPE_PAYLOAD)}, { import_id: ${JSON.stringify(ESCAPE_PAYLOAD)},`
    + " event: 'completed', status: 'success', entries_written: 1, entities_seeded: 0 })",
    panelContext
  );
  assert.ok(guideSteps.innerHTML.includes(`data-source="${ESCAPED_PAYLOAD}"`), 'data-source is escaped');
  assert.ok(
    guideSteps.innerHTML.includes(`href="/app/import/${encodeURIComponent(ESCAPE_PAYLOAD)}#content"`),
    'the browse link percent-encodes the import id'
  );
  assert.ok(!guideSteps.innerHTML.includes('<b>'), 'no payload markup reaches the panel');

  vm.runInContext(
    `showProgressView(${JSON.stringify(ESCAPE_PAYLOAD)}, { import_id: ${JSON.stringify(ESCAPE_PAYLOAD)},`
    + ` status: 'unavailable', unavailable_description: ${JSON.stringify(ESCAPE_PAYLOAD)} })`,
    panelContext
  );
  assert.ok(
    guideSteps.innerHTML.includes(`data-import-check-status="${ESCAPED_PAYLOAD}"`),
    'data-import-check-status is escaped'
  );
  assert.ok(!guideSteps.innerHTML.includes('<b>'), 'and the description is escaped with it');
  cases += 1;
}

// The answer to "check status" repaints the panel, so the button that was
// pressed is gone by the time focus is restored. It goes to the panel's own
// button, or to the panel itself -- never to the history row's button for the
// same import, and never to <body>.
async function runCheckStatusFocusLandsOnThePanel() {
  const scenarios = [
    { name: 'success', status: 'success', extra: { entries_written: 60, entities_seeded: 0 }, expected: 'panel' },
    { name: 'unconfirmed', status: 'unconfirmed', extra: { total_files_created: 1, entries_written: 1 }, expected: 'panelCheckStatus' },
  ];

  for (const scenario of scenarios) {
    // An id with a quote in it: the selector this restore builds goes through
    // CSS.escape like the row selectors do, or the browser throws on it.
    const importId = '1700000080"x';
    const context = vm.createContext({ console, Promise });
    const doc = { activeElement: null };
    const guideSteps = new Element();
    const panel = new FocusableElement('panel', doc);
    const rowButton = new FocusableElement('rowCheckStatus', doc, { 'data-import-check-status': importId });
    const pressedButton = new FocusableElement('panelCheckStatus', doc, { 'data-import-check-status': importId });
    let panelButton = pressedButton;
    let repaints = 0;

    Object.defineProperty(guideSteps, 'innerHTML', {
      configurable: true,
      get() { return this._html || ''; },
      set(value) {
        this._html = value;
        repaints += 1;
        assert.ok(
          value.includes('class="import-progress-panel" tabindex="-1"'),
          'the progress panel can take focus'
        );
        // The repaint replaces the panel's markup: the button that was pressed
        // is detached, and the new panel only has one if it rendered one.
        if (panelButton) panelButton.detach();
        panelButton = value.includes('data-import-check-status')
          ? new FocusableElement('panelCheckStatus', doc, { 'data-import-check-status': importId })
          : null;
      },
    });

    panel.querySelector = strictSelector((selector) => {
      assert.ok(
        selector.startsWith('[data-import-check-status='),
        'the panel is asked for its own check-status button'
      );
      return panelButton;
    });
    doc.getElementById = (id) => (id === 'guideSteps' ? guideSteps : null);
    doc.querySelector = strictSelector((selector) => {
      if (selector === '.import-progress-panel') return panel;
      // The history table comes before the panel in the document, so a
      // document-wide lookup returns its button, not the panel's.
      if (selector.startsWith('[data-import-check-status=')) return rowButton;
      return null;
    });
    doc.body = { contains: (element) => Boolean(element && element.attached) };
    doc.activeElement = pressedButton;

    context.document = doc;
    context.CSS = { escape: cssEscape };
    context.window = {
      location: { hash: `#progress/${importId}` },
      CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
      apiJson: async () => Object.assign({ import_id: importId, status: scenario.status, generation: 1 }, scenario.extra),
    };
    context.importEvents = {};
    context.importGenerationFloor = {};
    context.sourceMetadataByName = {};
    context.currentGuideSource = null;
    context.getImportById = () => null;
    installRealEscapeHtml(context);
    context.humanStageName = (s) => s;
    context.formatElapsed = () => '0s';
    context.renderProgressStats = () => '';
    context.formatDateRange = () => '';
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
    vm.runInContext(functionSource(workspace, 'reconcileImportState'), context);

    await vm.runInContext(`reconcileImportState(${JSON.stringify(importId)})`, context);

    assert.ok(repaints > 0, `the ${scenario.name} answer repaints the panel`);
    assert.ok(doc.activeElement, `focus does not fall to the body after a ${scenario.name} answer`);
    assert.notStrictEqual(
      doc.activeElement,
      rowButton,
      `focus does not jump to the history row's button after a ${scenario.name} answer`
    );
    assert.strictEqual(
      doc.activeElement.id,
      scenario.expected,
      `focus lands on the ${scenario.expected} after a ${scenario.name} answer`
    );
    cases += 1;
  }
}

// One register for the id across both surfaces the owner can read it on.
function runStalledCopyUsesTheLowercaseImportId() {
  const importId = '1700000090';
  const guideSteps = new Element();
  const statusCell = new Element();
  const context = vm.createContext({ console });
  const row = {
    classList: { add() {}, remove() {} },
    dataset: {},
    querySelector: (selector) => (selector === '.status-cell' ? statusCell : null),
  };
  context.CSS = { escape: cssEscape };
  context.document = {
    querySelector: selectorLookup({ [importId]: row }),
    getElementById: (id) => (id === 'guideSteps' ? guideSteps : null),
  };
  context.window = {
    location: { hash: `#progress/${importId}` },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again.' },
  };
  context.importEvents = {};
  context.sourceMetadataByName = {};
  context.currentGuideSource = null;
  context.getImportById = () => null;
  context.clearPendingImport = () => {};
  context.reconcileImportState = () => {};
  installRealEscapeHtml(context);
  assertEscaperIsReal(context);
  context.humanStageName = (s) => s;
  context.formatElapsed = () => '0s';
  context.renderProgressStats = () => '';
  context.formatDateRange = () => '';

  vm.runInContext(functionSource(workspace, 'formatStatValue'), context);
  vm.runInContext(functionSource(workspace, 'showProgressView'), context);
  vm.runInContext(functionSource(workspace, 'refreshInlineProgress'), context);
  vm.runInContext(functionSource(workspace, 'markRowStalled'), context);

  vm.runInContext(`markRowStalled(${JSON.stringify(importId)})`, context);

  assert.ok(statusCell.innerHTML.includes(`import id: ${importId}`), 'the stalled row names the import in lower case');
  assert.ok(!statusCell.innerHTML.includes('import ID:'), 'the upper-case register is gone from the row');
  assert.ok(
    guideSteps.innerHTML.includes(`<strong>import id:</strong> ${importId}`),
    'and the panel the row points at says the same words'
  );
  cases += 1;
}

// The floor was module state nothing ever pruned. It is bounded now, and the
// entry repeatSource depends on is pinned rather than swept.
function runGenerationFloorIsPruned() {
  const capMatch = /const IMPORT_GENERATION_FLOOR_CAP = (\d+);/.exec(workspace);
  assert.ok(capMatch, 'the generation floor carries a cap');
  const cap = Number(capMatch[1]);

  const context = vm.createContext({ console });
  context.window = { location: { hash: '#progress/open-panel' } };
  context.importEvents = {
    'live-event': { import_id: 'live-event', event: 'status', status: 'running' },
    // Finished, and no longer in the list: its own terminal state guards it, so
    // the floor is not what is keeping it honest.
    'finished-and-gone': { import_id: 'finished-and-gone', event: 'completed', status: 'success' },
  };
  context.importsCache = [{ timestamp: 'in-the-list' }];
  context.importGenerationFloor = {
    'in-the-list': 1, 'long-gone': 4, 'live-event': 2, 'open-panel': 3, 'finished-and-gone': 5,
  };
  vm.runInContext(`const IMPORT_GENERATION_FLOOR_CAP = ${cap};`, context);
  vm.runInContext(functionSource(workspace, 'isTerminalState'), context);
  vm.runInContext(functionSource(workspace, 'pruneImportGenerationFloor'), context);
  vm.runInContext('pruneImportGenerationFloor()', context);

  assert.deepStrictEqual(
    Object.keys(context.importGenerationFloor).sort(),
    ['in-the-list', 'live-event', 'open-panel'],
    'an id the history list no longer carries, and nothing in flight refers to, is dropped'
  );
  assert.strictEqual(
    context.importGenerationFloor['live-event'],
    2,
    'an import still running keeps its floor: a stale event can still arrive for it'
  );
  assert.strictEqual(
    context.importGenerationFloor['open-panel'],
    3,
    'the floor the open progress view relies on is never dropped: repeatSource reads it on the way out'
  );

  const listed = [];
  Object.keys(context.importGenerationFloor).forEach((id) => { delete context.importGenerationFloor[id]; });
  for (let index = 0; index < cap + 40; index += 1) {
    const id = `17000${String(index).padStart(5, '0')}`;
    listed.push({ timestamp: id });
    context.importGenerationFloor[id] = 1;
  }
  context.importGenerationFloor['open-panel'] = 3;
  context.importsCache = listed;
  context.importEvents = {};
  vm.runInContext('pruneImportGenerationFloor()', context);

  assert.strictEqual(Object.keys(context.importGenerationFloor).length, cap, 'the floor stops growing at the cap');
  assert.strictEqual(context.importGenerationFloor['open-panel'], 3, 'and the pinned entry survives the cap pass');
  assert.ok(
    !Object.prototype.hasOwnProperty.call(context.importGenerationFloor, '1700000000'),
    'the oldest ids are the ones that go'
  );

  assert.ok(
    functionSource(workspace, 'loadImports').includes('pruneImportGenerationFloor()'),
    'the import list prunes the floor as it reloads'
  );
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
  .then(runHashChangeDedupConsumesTheFlag)
  .then(runNavigateToDropsTheFlagWhenTheFragmentDoesNotChange)
  .then(runNavigateToDoesNotReassertAStaleHash)
  .then(runRepaintSeesTheFactsThatWereStored)
  .then(runRepeatSourceFocusSurvivesTheQueuedHashChange)
  .then(runAuthoritativeRunningReadKeepsACompletedImportComplete)
  .then(runRunningReadKeepsKnownFacts)
  .then(runCanonicalReadOutranksTheGenerationFloor)
  .then(runTerminalStateCoversEveryEndState)
  .then(runGenerationFloorIsRaisedByEveryEvent)
  .then(runEveryOwnerSinkEscapes)
  .then(runDetailDropsFactsThatDoNotApply)
  .then(runDetailPageCountMatchesItsNumber)
  .then(runUnavailablePanelNeverEchoesTheServer)
  .then(runSourceCardsCarryNoInlineHandler)
  .then(runUnknownUsesTheEmDashInTheHistoryTable)
  .then(runMeasuredZeroIsNotUnknown)
  .then(runImportIdSelectorsEscapeQuotes)
  .then(runOwnerSinksEscapeServerDerivedPayloads)
  .then(runCheckStatusFocusLandsOnThePanel)
  .then(runStalledCopyUsesTheLowercaseImportId)
  .then(runGenerationFloorIsPruned)
  .then(() => console.log(`DOM CASES: ${cases} passed`))
  .catch((error) => {
    console.error(error.stack || error);
    process.exitCode = 1;
  });
