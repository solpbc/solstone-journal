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

Promise.resolve()
  .then(runQuickSubmit)
  .then(runGuidedSubmit)
  .then(runConfirmSubmit)
  .then(() => console.log(`DOM CASES: ${cases} passed`))
  .catch((error) => {
    console.error(error.stack || error);
    process.exitCode = 1;
  });
