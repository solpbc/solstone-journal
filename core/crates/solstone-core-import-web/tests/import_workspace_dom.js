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
  return source.slice(start, start + source.slice(start).indexOf('{'))
    + balancedBlock(source, start);
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
  cases += 1;
}

Promise.resolve()
  .then(runQuickSubmit)
  .then(runGuidedSubmit)
  .then(runConfirmSubmit)
  .then(runHistoryHeaderSummary)
  .then(runImportRowColumns)
  .then(() => console.log(`DOM CASES: ${cases} passed`))
  .catch((error) => {
    console.error(error.stack || error);
    process.exitCode = 1;
  });
