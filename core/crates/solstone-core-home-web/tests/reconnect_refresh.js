// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const manifestDir = process.argv[2];
if (!manifestDir) throw new Error('manifest directory required');

let source = fs.readFileSync(path.join(manifestDir, 'assets/home.js'), 'utf8');
source = source.replace(
  '  window.selectAccountSide = selectAccountSide;\n',
  `  window.__home = { connectionRecovery, wireRealtime };
  window.selectAccountSide = selectAccountSide;\n`,
);

let firstMinutesResponse = null;
const requests = [];
const connectionHandlers = [];
const window = {
  location: { href: '' },
  document: { readyState: 'loading', addEventListener() {}, querySelector() { return null; }, getElementById() { return null; } },
  addEventListener() {},
  apiJson(url) {
    requests.push(url);
    if (url === '/app/home/api/first-minutes' && firstMinutesResponse) {
      return Promise.resolve(firstMinutesResponse);
    }
    return Promise.resolve({});
  },
  appEvents: {
    listen() {},
    onConnectionState(handler) {
      connectionHandlers.push(handler);
      return () => {};
    },
  },
};
window.window = window;
window.document.defaultView = window;

vm.runInNewContext(source, { window, document: window.document, console }, { filename: 'home.js' });
const { connectionRecovery, wireRealtime } = window.__home;
assert(connectionRecovery, 'connectionRecovery exported');
assert(wireRealtime, 'wireRealtime exported');

function sequence(states) {
  let refreshes = 0;
  const handle = connectionRecovery(() => { refreshes += 1; });
  states.forEach((connected) => handle({ connected, state: connected ? 'connected' : 'disconnected' }));
  return refreshes;
}

// The state reported at subscription, and repeats of it, are not a reconnect.
assert.strictEqual(sequence([true]), 0);
assert.strictEqual(sequence([true, true, true]), 0);
assert.strictEqual(sequence([false]), 0);
// One recovery per disconnect, however many disconnected states arrive first.
assert.strictEqual(sequence([true, false, true]), 1);
assert.strictEqual(sequence([true, false, false, true]), 1);
assert.strictEqual(sequence([true, false, true, true]), 1);
assert.strictEqual(sequence([true, false, true, false, true]), 2);
// A page that subscribed while still connecting recovers on its first connect.
assert.strictEqual(sequence([false, true]), 1);
// A missing state is treated as disconnected, never as a reconnect.
const tolerant = connectionRecovery(() => { throw new Error('unexpected refresh'); });
tolerant(undefined);
tolerant(null);

async function main() {
  // The wiring Home installs: a real disconnect and reconnect re-reads pulse (vitals and
  // narrative), the briefing, first-minutes, and setup state when the window is open.
  wireRealtime();
  requests.length = 0;
  assert.strictEqual(connectionHandlers.length, 1, 'Home subscribes to connection state exactly once');
  const notify = connectionHandlers[0];
  notify({ connected: true, state: 'connected' });
  assert.deepStrictEqual(requests, [], 'the baseline connected state requests nothing');
  notify({ connected: false, state: 'disconnected' });
  assert.deepStrictEqual(requests, [], 'a disconnect requests nothing');
  notify({ connected: true, state: 'connected' });
  assert.deepStrictEqual(
    requests.slice().sort(),
    ['/app/home/api/briefing', '/app/home/api/first-minutes', '/app/home/api/pulse'],
    'open window reconnect re-reads briefing, pulse, and first-minutes (which carries setup)',
  );
  notify({ connected: true, state: 'connected' });
  assert.strictEqual(requests.length, 3, 'a repeated connected state does not reconcile again');

  // Let first reconnect settle
  await new Promise((resolve) => setImmediate(resolve));

  // Next reconnect resolves window_open: false for first-minutes
  firstMinutesResponse = { window_open: false };
  notify({ connected: false, state: 'disconnected' });
  requests.length = 0;
  notify({ connected: true, state: 'connected' });
  assert.deepStrictEqual(
    requests.slice().sort(),
    ['/app/home/api/briefing', '/app/home/api/first-minutes', '/app/home/api/pulse'],
  );
  // Allow promise to resolve and latch firstMinutesClosed
  await new Promise((resolve) => setImmediate(resolve));

  // Subsequent reconnect requests only briefing and pulse
  requests.length = 0;
  notify({ connected: false, state: 'disconnected' });
  assert.deepStrictEqual(requests, [], 'a disconnect requests nothing');
  notify({ connected: true, state: 'connected' });
  assert.deepStrictEqual(
    requests.slice().sort(),
    ['/app/home/api/briefing', '/app/home/api/pulse'],
    'after window closed, reconnect requests only briefing and pulse',
  );
  notify({ connected: true, state: 'connected' });
  assert.strictEqual(requests.length, 2, 'a repeated connected state does not reconcile again');

  // A shell without the connection-state API keeps Home's event wiring working.
  const bareRequests = [];
  const bare = {
    location: { href: '' },
    document: { readyState: 'loading', addEventListener() {}, querySelector() { return null; } },
    addEventListener() {},
    apiJson(url) { bareRequests.push(url); return Promise.resolve({}); },
    appEvents: { listen() {} },
  };
  bare.window = bare;
  bare.document.defaultView = bare;
  vm.runInNewContext(source, { window: bare, document: bare.document, console }, { filename: 'home.js' });
  assert.doesNotThrow(() => bare.__home.wireRealtime());

  console.log('home reconnect reconciliation passed');
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
