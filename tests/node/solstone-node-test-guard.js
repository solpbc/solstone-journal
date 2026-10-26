// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Preloaded into every node process a cargo test starts, through NODE_OPTIONS
// in .cargo/config.toml. It keeps a failing or runaway DOM harness from taking
// the machine down with it:
//
// - A failed strict assertion reports a bounded description of its values.
//   node's own message inspects both sides 1000 levels deep, and a fake DOM
//   node reaches its whole document through parent, children and owner links,
//   so one failing comparison of two nodes could grow without limit.
// - A watchdog thread kills the process once it passes a memory cap or a time
//   limit. It runs off the main thread, so it still fires when a synchronous
//   loop has blocked the event loop.
//
// SOLSTONE_NODE_TEST_RSS_MB (default 2048) and SOLSTONE_NODE_TEST_SECONDS
// (default 300) raise the limits for one run.

'use strict';

const { isMainThread, Worker } = require('node:worker_threads');

if (isMainThread && !globalThis.__solstoneNodeTestGuard) {
  globalThis.__solstoneNodeTestGuard = true;
  installBoundedAssertions();
  startWatchdog();
}

function installBoundedAssertions() {
  const assert = require('node:assert');
  const util = require('node:util');

  const describe = (value) => {
    let text;
    try {
      text = util.inspect(value, {
        depth: 1,
        maxArrayLength: 10,
        maxStringLength: 300,
        breakLength: 100,
        compact: 3,
      });
    } catch (_error) {
      text = Object.prototype.toString.call(value);
    }
    return text.length > 1500 ? text.slice(0, 1500) + ' …' : text;
  };

  const fail = (operator, actual, expected, message, stackStartFn) => {
    if (message instanceof Error) throw message;
    throw new assert.AssertionError({
      message: message || `${operator} failed\n  actual:   ${describe(actual)}\n  expected: ${describe(expected)}`,
      operator,
      stackStartFn,
    });
  };

  function strictEqual(actual, expected, message) {
    if (!Object.is(actual, expected)) fail('strictEqual', actual, expected, message, strictEqual);
  }
  function notStrictEqual(actual, expected, message) {
    if (Object.is(actual, expected)) fail('notStrictEqual', actual, expected, message, notStrictEqual);
  }
  function deepStrictEqual(actual, expected, message) {
    if (!util.isDeepStrictEqual(actual, expected)) fail('deepStrictEqual', actual, expected, message, deepStrictEqual);
  }
  function notDeepStrictEqual(actual, expected, message) {
    if (util.isDeepStrictEqual(actual, expected)) fail('notDeepStrictEqual', actual, expected, message, notDeepStrictEqual);
  }

  for (const target of [assert, assert.strict]) {
    target.strictEqual = strictEqual;
    target.notStrictEqual = notStrictEqual;
    target.deepStrictEqual = deepStrictEqual;
    target.notDeepStrictEqual = notDeepStrictEqual;
  }
  // In strict mode the loose names are the strict checks.
  assert.strict.equal = strictEqual;
  assert.strict.notEqual = notStrictEqual;
  assert.strict.deepEqual = deepStrictEqual;
  assert.strict.notDeepEqual = notDeepStrictEqual;
  require('node:module').syncBuiltinESMExports();
}

function startWatchdog() {
  const limit = (name, fallback) => {
    const value = Number(process.env[name]);
    return Number.isFinite(value) && value > 0 ? value : fallback;
  };
  const watchdog = new Worker(
    `
    const { workerData } = require('node:worker_threads');
    const fs = require('node:fs');
    const started = Date.now();
    setInterval(() => {
      const rssMb = process.memoryUsage.rss() / 1048576;
      const seconds = (Date.now() - started) / 1000;
      let reason = null;
      if (rssMb > workerData.rssMb) {
        reason = 'memory ' + Math.round(rssMb) + ' MB, over the ' + workerData.rssMb + ' MB cap';
      } else if (seconds > workerData.seconds) {
        reason = 'ran ' + Math.round(seconds) + ' s, over the ' + workerData.seconds + ' s limit';
      }
      if (!reason) return;
      fs.writeSync(2, '\\nnode test guard: killed ' + workerData.script + ': ' + reason +
        ' (SOLSTONE_NODE_TEST_RSS_MB / SOLSTONE_NODE_TEST_SECONDS raise it)\\n');
      process.kill(process.pid, 'SIGKILL');
    }, 200);
    `,
    {
      eval: true,
      workerData: {
        rssMb: limit('SOLSTONE_NODE_TEST_RSS_MB', 2048),
        seconds: limit('SOLSTONE_NODE_TEST_SECONDS', 300),
        script: process.argv[1] || '(node)',
      },
    },
  );
  watchdog.unref();
}
