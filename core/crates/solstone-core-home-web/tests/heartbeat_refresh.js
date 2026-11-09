// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Home reads pulse again when something pulse speaks for has changed, not on
// every report the journal sends while nothing has.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const manifestDir = process.argv[2];
if (!manifestDir) throw new Error('manifest directory required');

let source = fs.readFileSync(path.join(manifestDir, 'assets/home.js'), 'utf8');
source = source.replace(
  '  window.selectAccountSide = selectAccountSide;\n',
  `  window.__home = { wireRealtime, seedPulse: (data) => { lastPulse = data; } };
  window.selectAccountSide = selectAccountSide;\n`,
);
// No DOM here: drawing is out of scope, only what is read.
for (const name of ['renderVitals', 'renderPair', 'renderNarrative', 'renderPulse']) {
  source = source.replace(`  function ${name}(`, `  function ${name}() { return; }\n  function __unused_${name}(`);
}
const PULSE = { today: '20261109', health_glance: {}, narrative_content: null, needs_you_items: [] };

const requests = [];
const listeners = {};
const intervals = [];
const window = {
  createViewReady: () => () => {},
  location: { href: '', pathname: '/app/home/' },
  document: { readyState: 'loading', addEventListener() {}, querySelector() { return null; }, getElementById() { return null; } },
  addEventListener() {},
  setTimeout() { return 1; },
  clearTimeout() {},
  apiJson(url) {
    requests.push(url);
    return Promise.resolve(url === '/app/home/api/pulse' ? { ...PULSE } : {});
  },
  AppServices: { visibleInterval(fn, ms) { intervals.push({ fn, ms }); return () => {}; } },
  appEvents: {
    listen(tract, fn) { (listeners[tract] = listeners[tract] || []).push(fn); },
    onConnectionState() { return () => {}; },
  },
};
window.window = window;
window.document.defaultView = window;
vm.runInNewContext(source, { window, document: window.document, console, Map, JSON }, { filename: 'home.js' });

const send = (tract, msg) => (listeners[tract] || []).forEach((fn) => fn({ tract, ...msg }));
const settle = () => new Promise((resolve) => setImmediate(resolve));
const pulseReads = () => requests.filter((url) => url === '/app/home/api/pulse').length;
const firstMinutesReads = () => requests.filter((url) => url === '/app/home/api/first-minutes').length;

function status(overrides = {}) {
  return {
    event: 'status',
    services: [{ name: 'supervisor', pid: 10 + Math.random(), uptime_seconds: Math.random() * 1e5, phase: 'running' },
      { name: 'convey', pid: 11, uptime_seconds: 5, phase: 'running' }],
    crashed: [], tasks: [], recent_tasks: [], held: [], queue_hold: null, stale_heartbeats: [],
    ...overrides,
  };
}

(async () => {
  window.__home.wireRealtime();
  window.__home.seedPulse({ ...PULSE });
  await settle();
  requests.length = 0;

  // The first report is news; a run of reports that differ only in pids and
  // uptimes is not.
  send('supervisor', status());
  await settle();
  assert.strictEqual(pulseReads(), 1, 'the first report reads pulse');
  for (let i = 0; i < 12; i += 1) {
    send('supervisor', status());
    await settle();
  }
  assert.strictEqual(pulseReads(), 1, 'a minute of unchanged reports reads nothing');
  assert.strictEqual(firstMinutesReads(), 1, 'nor first-minutes');

  // A service changing phase is news.
  send('supervisor', status({ services: [{ name: 'convey', phase: 'restarting', reason_code: 'exit' }] }));
  await settle();
  assert.strictEqual(pulseReads(), 2);
  send('supervisor', status({ held: [{ reference: 'r1', reasons: ['quota'] }] }));
  await settle();
  assert.strictEqual(pulseReads(), 3, 'held work is news');

  // An observer's counters are not news; its mode moving is.
  send('observe', { event: 'status', stream: 'mac.screen', mode: 'screencast', screencast: { window_elapsed_seconds: 1 } });
  await settle();
  const afterFirstObserve = pulseReads();
  for (let i = 0; i < 6; i += 1) {
    send('observe', { event: 'status', stream: 'mac.screen', mode: 'screencast', screencast: { window_elapsed_seconds: i * 5 } });
    await settle();
  }
  assert.strictEqual(pulseReads(), afterFirstObserve, 'counters alone read nothing');
  send('observe', { event: 'status', stream: 'mac.screen', mode: 'idle' });
  await settle();
  assert.strictEqual(pulseReads(), afterFirstObserve + 1);
  send('observe', { event: 'observed', day: '20261109' });
  await settle();
  assert.strictEqual(pulseReads(), afterFirstObserve + 2, 'new material always reads');

  // A day changing is news for today and yesterday only.
  const beforeChanges = pulseReads();
  send('change', { event: 'changed', kind: 'day', key: '20260301' });
  send('change', { event: 'changed', kind: 'system', key: 'calendar' });
  await settle();
  assert.strictEqual(pulseReads(), beforeChanges, 'an old day or the calendar is not pulse\'s');
  send('change', { event: 'changed', kind: 'day', key: '20261108' });
  await settle();
  await settle();
  assert.ok(pulseReads() > beforeChanges, 'yesterday is');

  // A slow floor covers what moves with time alone, and only while in view.
  assert.deepStrictEqual(intervals.map((entry) => entry.ms), [60000]);
  console.log('home heartbeat refresh passed');
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
