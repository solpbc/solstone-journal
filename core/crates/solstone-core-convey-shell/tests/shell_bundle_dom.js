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

const [bundlePath, staticDir, isolationPath] = process.argv.slice(2);
const bundle = fs.readFileSync(bundlePath, 'utf8');
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

check('the bundle parses', () => {
  new vm.Script(bundle, { filename: 'shell.js' });
});

const table = segmentTable(bundle);

check('every file in the bundle parses on its own', () => {
  assert(table.length > 20, `only ${table.length} segments`);
  for (const [, name] of table) {
    const source = fs.readFileSync(path.join(staticDir, name.replace(/^static\//, '')), 'utf8');
    try {
      new vm.Script(source, { filename: '/' + name });
    } catch (error) {
      throw new Error(`/${name} doesn't parse: ${error.message}`);
    }
  }
});

check('each file hands over exactly the names it declares at its top level', () => {
  const lines = bundle.split('\n');
  let total = 0;
  for (const [line, name] of table) {
    const source = fs.readFileSync(path.join(staticDir, name.replace(/^static\//, '')), 'utf8');
    const header = lines[line - 2];
    assert(header.startsWith('try { (function () {'), `/${name}'s wrapper`);
    const exported = exportedNames(header);
    assert.deepStrictEqual(exported, declaredNames(source, name), `/${name}`);
    total += exported.length;
  }
  assert(total >= 20, `only ${total} names handed over`);
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

console.log(`DOM CASES: ${passed} passed`);
