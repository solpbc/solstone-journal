// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The shell's script bundle under Node: every file and the bundle parse; each
// file's top-level names are exactly the globals the bundle hands over; and in
// a bundle of throwaway files, one that throws stops only itself, a strict file
// stays strict, globals are live both ways, and an error's position names its
// file.
//
// This parse is the full syntax check of the bundle. The build's own lexer
// catches the common breakages on every build; a parse needs a JavaScript
// engine, and `make ci` starts no process, so it lives here, in `make ci-full`.
// Every journal release runs `make ci-full` on its final source and a failure
// holds it, so an unparsable bundle can't reach an owner. Keep this in the
// full suite.

'use strict';

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const [bundlePath, restPath, staticDir, isolationPath, splitBootPath, splitRestPath] = process.argv.slice(2);
const bundle = fs.readFileSync(bundlePath, 'utf8');
const restBundle = fs.readFileSync(restPath, 'utf8');
const splitBoot = fs.readFileSync(splitBootPath, 'utf8');
const splitRest = fs.readFileSync(splitRestPath, 'utf8');
let passed = 0;

function check(name, fn) {
  try {
    fn();
    passed += 1;
  } catch (error) {
    console.error(`FAIL ${name}: ${error && error.stack || error}`);
    process.exitCode = 1;
  }
}

function segmentTable(text) {
  const match = /var segments = (\[.*?\]);/.exec(text);
  assert(match, 'the bundle has a segment table');
  return JSON.parse(match[1]);
}

// Ask V8 what a file declares at its top level, without running any of it: a
// leading throw stops it after declarations are made. var and function names
// are then properties of the global object; a let, const or class name is a
// global binding that never initialised, so even `typeof` on it throws.
function declaredNames(source, filename) {
  const context = vm.createContext({});
  const before = new Set(Object.getOwnPropertyNames(context));
  try {
    vm.runInContext('throw 0;\n' + source, context, { filename });
  } catch (_) {}
  const names = new Set(
    Object.getOwnPropertyNames(context).filter((name) => !before.has(name))
  );
  const candidates = new Set(source.match(/[A-Za-z_$][\w$]*/g) || []);
  for (const name of candidates) {
    if (names.has(name)) continue;
    try {
      vm.runInContext(`typeof ${name}`, context);
    } catch (error) {
      // `typeof` never throws for an undeclared name, so a ReferenceError
      // here is a declared binding that never initialised. (The error comes
      // from the other context, so its name is compared, not its class.)
      if (error && error.name === 'ReferenceError') names.add(name);
    }
  }
  return [...names].sort();
}

function exportedNames(segmentText) {
  const names = [];
  const pattern = /Object\.defineProperty\(globalThis, '([\w$]+)', \{ configurable/g;
  let found;
  while ((found = pattern.exec(segmentText))) names.push(found[1]);
  return names.sort();
}

function checkBundle(label, text, minSegments) {
  check(`${label} parses`, () => {
    new vm.Script(text, { filename: label });
  });
  const table = segmentTable(text);
  check(`every file in ${label} parses on its own`, () => {
    assert(table.length >= minSegments, `${label} has only ${table.length} segments`);
    for (const [, name] of table) {
      const source = fs.readFileSync(path.join(staticDir, name.replace(/^static\//, '')), 'utf8');
      try {
        new vm.Script(source, { filename: '/' + name });
      } catch (error) {
        throw new Error(`/${name} doesn't parse: ${error.message}`);
      }
    }
  });
  return table;
}

const bootTable = checkBundle('shell.js', bundle, 20);
const restTable = checkBundle('shell-rest.js', restBundle, 5);

check('each file hands over exactly the names it declares at its top level', () => {
  let total = 0;
  for (const [text, table] of [[bundle, bootTable], [restBundle, restTable]]) {
    const lines = text.split('\n');
    for (const [line, name] of table) {
      const source = fs.readFileSync(path.join(staticDir, name.replace(/^static\//, '')), 'utf8');
      const header = lines[line - 2];
      assert(header.startsWith('try { (function () {'), `/${name}'s wrapper`);
      const exported = exportedNames(header);
      assert.deepStrictEqual(exported, declaredNames(source, name), `/${name}`);
      total += exported.length;
    }
  }
  assert(total >= 20, `only ${total} names handed over`);
  assert(bootTable.length + restTable.length >= 25, 'a shell file is in neither bundle');
});

const isolation = fs.readFileSync(isolationPath, 'utf8');

function runIsolation() {
  const reported = [];
  const src = 'http://journal.test/static/0123456789abcdef/shell.js';
  const context = vm.createContext({
    document: { currentScript: { src } },
    setTimeout,
    reportError: (error) => reported.push(error),
  });
  vm.runInContext(isolation, context, { filename: src });
  return { context, reported, src };
}

check('a file that throws stops only itself, and its error is reported once', () => {
  const { context, reported } = runIsolation();
  assert.strictEqual(reported.length, 1);
  assert.strictEqual(reported[0].message, 'b broke');
  assert.strictEqual(vm.runInContext('cRan', context), true);
  assert.strictEqual(vm.runInContext('dRan', context), true);
});

check('top-level names stay globals with live bindings, both ways', () => {
  const { context } = runIsolation();
  const read = (code) => vm.runInContext(code, context);
  assert.strictEqual(read('typeof helper'), 'function');
  assert.strictEqual(read('helper()'), 'h');
  assert.strictEqual(read('first'), 1);
  assert.strictEqual(read('LIMIT'), 3);
  assert.strictEqual(read('typeof Box'), 'function');
  // c.js bumped a's counter, then set it to 5 by name; a sees both.
  assert.strictEqual(read('sawCounterLive'), true);
  assert.strictEqual(read('bump()'), 6);
  assert.strictEqual(read('counter'), 6);
  assert.strictEqual(read('typeof notDeclaredAnywhere'), 'undefined');
  // A hoisted function is a global from its file's first line, as before.
  assert.strictEqual(read('earlyCall'), 'h');
  // A name declared after a file throws is still that file's: b's function
  // was hoisted before its throw.
  assert.strictEqual(read('typeof afterThrow'), 'function');
});

check('a strict file stays strict and a sloppy one stays sloppy', () => {
  const { context } = runIsolation();
  assert.strictEqual(vm.runInContext('strictThis', context), true);
  assert.strictEqual(vm.runInContext('sloppyThisIsGlobal', context), true);
});

check("an error's position names its file and line", () => {
  const { context, src } = runIsolation();
  const shell = vm.runInContext('__solShell', context);
  const segments = shell.segments;
  const b = segments.find(([, name]) => name === 'static/b.js');
  assert(b, 'b.js is in the table');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(shell.locate(src, b[0] + 1))),
    { file: '/static/b.js', line: 2 }
  );
  assert.strictEqual(shell.locate('http://elsewhere/x.js', b[0]), null);
  const stack = `Error: b broke\n    at http://journal.test/static/0123456789abcdef/shell.js:${b[0]}:7`;
  assert.strictEqual(shell.stack(stack), 'Error: b broke\n    at /static/b.js:1:7');
});

function runSplit(order) {
  const reported = [];
  const bootUrl = 'http://journal.test/static/0123456789abcdef/shell.js';
  const restUrl = 'http://journal.test/static/0123456789abcdef/shell-rest.js';
  const currentScript = { src: bootUrl };
  const context = vm.createContext({
    document: { currentScript },
    setTimeout,
    reportError: (error) => reported.push(error),
  });
  for (const which of order) {
    currentScript.src = which === 'rest' ? restUrl : bootUrl;
    const text = which === 'rest' ? splitRest : splitBoot;
    vm.runInContext(text, context, { filename: currentScript.src });
  }
  return { context, reported, bootUrl, restUrl };
}

check('an error in the rest file names its own file, whichever file ran first', () => {
  const row = segmentTable(splitRest).find(([, name]) => name === 'static/rest.js');
  assert(row, 'rest.js is in the rest table');
  for (const order of [['boot', 'rest'], ['rest', 'boot']]) {
    const { context, reported, restUrl } = runSplit(order);
    assert.strictEqual(reported.length, 1, order.join(' then '));
    assert.strictEqual(reported[0].message, 'rest broke');
    const shell = vm.runInContext('__solShell', context);
    assert.deepStrictEqual(
      JSON.parse(JSON.stringify(shell.locate(restUrl, row[0] + 1))),
      { file: '/static/rest.js', line: 2 }
    );
    const stack = `Error: rest broke\n    at ${restUrl}:${row[0]}:1`;
    assert.strictEqual(shell.stack(stack), 'Error: rest broke\n    at /static/rest.js:1:1');
    assert.strictEqual(shell.locate('http://elsewhere/x.js', row[0]), null);
  }
});

function host() {
  const sandbox = vm.createContext({
    console: { error() {}, debug() {} },
    setTimeout,
    clearTimeout,
    URL,
  });
  sandbox.window = sandbox;
  sandbox.addEventListener = function () {};
  sandbox.document = {
    readyState: 'complete',
    baseURI: 'http://journal.test/',
    visibilityState: 'visible',
    addEventListener() {},
    getElementById() { return null; },
    querySelector() { return null; },
  };
  sandbox.location = {
    pathname: '/app/journal/',
    href: 'http://journal.test/app/journal/',
    origin: 'http://journal.test',
    reload() { sandbox.reloads += 1; },
  };
  sandbox.reloads = 0;
  const store = new Map();
  sandbox.sessionStorage = {
    getItem(key) { return store.has(key) ? store.get(key) : null; },
    setItem(key, value) { store.set(key, String(value)); },
  };
  return sandbox;
}

check('an error before the diagnostic console loads is shown once the console loads', () => {
  const sandbox = host();
  vm.runInContext(
    fs.readFileSync(path.join(staticDir, 'error-handler.js'), 'utf8'),
    sandbox,
    { filename: '/static/error-handler.js' }
  );
  sandbox.logError(new Error('kept'), { kind: 'test' });
  assert.strictEqual(sandbox.convey.pendingDiagnostics.length, 1);
  assert.strictEqual(sandbox.convey.pendingDiagnostics[0].detail.message, 'kept');
  const got = [];
  sandbox.convey.deliverPendingDiagnostics({ push(entry) { got.push(entry); } });
  assert.strictEqual(got.length, 1);
  assert.strictEqual(got[0].detail.message, 'kept');
  assert.strictEqual(sandbox.convey.pendingDiagnostics.length, 0);
});

async function staleAssetCase() {
  const sandbox = host();
  const url = 'http://journal.test/static/0123456789abcdef/shell-rest.js';
  sandbox.fetch = async () => { throw new Error('offline'); };
  vm.runInContext(
    fs.readFileSync(path.join(staticDir, 'api.js'), 'utf8'),
    sandbox,
    { filename: '/static/api.js' }
  );
  assert.strictEqual(await sandbox.recoverFromStaleAsset(url), false);
  assert.strictEqual(sandbox.reloads, 0);
  sandbox.fetch = async () => ({ status: 404 });
  assert.strictEqual(await sandbox.recoverFromStaleAsset(url), true);
  assert.strictEqual(sandbox.reloads, 1);
  assert.strictEqual(await sandbox.recoverFromStaleAsset(url), false);
  assert.strictEqual(sandbox.reloads, 1);
  passed += 1;
}

staleAssetCase().then(() => {
  console.log(`DOM CASES: ${passed} passed`);
}).catch((error) => {
  console.error(`FAIL a rest file that 404s reloads once, and a network failure does not: ${error && error.stack || error}`);
  process.exitCode = 1;
  console.log(`DOM CASES: ${passed} passed`);
});
