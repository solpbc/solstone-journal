// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Drives the Body app's embedded script against stubbed status and day
// payloads: what the overview lists and what a day page says.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const root = process.argv[2] || path.join(__dirname, '..');
const html = fs.readFileSync(path.join(root, 'assets/body/workspace.html'), 'utf8');
const script = html.slice(html.lastIndexOf('<script>') + '<script>'.length, html.lastIndexOf('</script>'));

function escapeHtml(value) {
  return String(value ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function element() {
  return {
    hidden: false,
    innerHTML: '',
    querySelector: () => null,
    toggleAttribute() {},
    addEventListener() {},
  };
}

async function load({ day, responses }) {
  const viewRoot = element();
  const gate = element();
  const shell = element();
  const document = {
    getElementById: (id) => (id === 'body-view-root' ? viewRoot : id === 'body-state-gate' ? gate : null),
    querySelector: (selector) => (selector === '.body-shell' ? shell : null),
    querySelectorAll: () => [],
    addEventListener() {},
  };
  const window = {
    createViewReady: () => () => {},
    AppServices: { escapeHtml },
    SurfaceState: { loading: () => 'loading', error: () => 'error' },
    CONVEY_COPY: { RELOAD_HINT: 'reload' },
    Drawer: { render: ({ label }) => `<drawer>${label}</drawer>` },
    solPathContext: () => ({ day, segment: day ? day : null }),
    apiJson: async (url) => {
      assert.ok(url in responses, `unexpected request: ${url}`);
      return responses[url];
    },
  };
  const context = vm.createContext({
    window,
    document,
    console,
    Number, String, Array, Object, Date, Math, Boolean,
    encodeURIComponent,
    setTimeout, CSS: { escape: (value) => value }, fetch: async () => ({ ok: false }),
  });
  vm.runInContext(script, context);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.strictEqual(gate.innerHTML, '', 'no error or loading state left on the gate');
  return viewRoot.innerHTML;
}

const EMPTY_STATUS = {
  coverage: null,
  sources: [],
  day_grid: [],
  days: 0,
  imports: { new: [], old: [] },
};

const STATUS = {
  coverage: { start: '20260110', end: '20260112' },
  sources: [
    { source: 'apple_health', name: 'Apple Health', first_label: 'Jan 10, 2026', last_label: 'Jan 12, 2026', days: 3, days_label: '3 days' },
  ],
  day_grid: [],
  days: 3,
  imports: {
    new: [
      { id: 'a', source: 'Apple Health', when: 'Jan 10, 2026, 12:00 PM', state: 'finished', summary: '# Apple Health import\n\nsaved', progress: null },
      { id: 'b', source: 'Oura', when: 'Jan 11, 2026, 12:00 PM', state: 'stopped', summary: null, progress: 'stopped partway: 3 of 120 days done.' },
      { id: 'c', source: 'Oura', when: 'Jan 12, 2026, 12:00 PM', state: 'running', summary: null, progress: 'still running: 5 of 40 days done.' },
    ],
    old: [
      { id: 'n', source: 'Apple Health', span_by: 'days', span: 'Jan 1, 2026 to Jan 2, 2026, 2 days', line: 'Apple Health from Jan 1, 2026 to Jan 2, 2026, 2 days, kept but not read here. import again from a new Apple Health export to bring it in.', importer: true },
      { id: 'd', source: 'Dexcom Clarity', span_by: 'months', span: 'March 2026 (by month)', line: "Dexcom Clarity from March 2026 (by month), kept but not read here. there's no importer for Dexcom Clarity now, so it stays as it is.", importer: false },
    ],
  },
};

const DAY = {
  day: '20260110',
  has_data: true,
  entry_total: 8,
  sources: [{
    source: 'apple_health',
    name: 'Apple Health',
    experiences: [{ kind: 'workout', name: 'running', start: '6:31 AM', end: '7:20 AM', by: 'Synthetic Watch', distance: '8.1 km', continuing: false }],
    readings: [{ name: 'heart rate', count: 2, count_label: '2 readings', first: '6:35 AM', last: '9:10 PM', by: 'Synthetic Watch' }],
    counts: [{ name: 'step count', total: '3,200.5', count: 2, count_label: '2 entries', by: 'Synthetic Watch' }],
    logged: [{ name: 'headache', count: 1, count_label: '1 entry', first: '6:50 AM', last: '6:50 AM', who: 'you, in Synthetic Watch' }],
    documents: [{ name: 'readiness', value: null }],
  }],
  strava: [{ name: 'Morning Run', source: 'Strava', start: '7:00 AM', duration: '30 min', metrics_label: '5.0 km', details: [], more_from_strava: [], strava_activity_id: '9001' }],
  unreadable_windows: 0,
  earlier_imports: [{ source: 'Oura', scope: 'day', line: 'an earlier Oura import holds this day\'s body data, kept but not read here. import again from your Oura account to bring it in.' }],
  nearest: { prev: null, prev_label: null, next: null, next_label: null },
};

const EMPTY_DAY = {
  day: '20260315',
  has_data: false,
  entry_total: 0,
  sources: [],
  strava: [],
  unreadable_windows: 0,
  earlier_imports: [{ source: 'Apple Health', scope: 'month', line: 'an earlier Apple Health import holds body data for March 2026, kept but not read here, so it doesn\'t show on this page. import again from a new Apple Health export to bring it in.' }],
  nearest: { prev: '20260110', prev_label: 'Jan 10', next: null, next_label: null },
};

const cases = [
  ['a first-run journal says nothing is here and names no privacy setup', async () => {
    const page = await load({ responses: { '/app/body/api/status': EMPTY_STATUS } });
    assert.ok(page.includes('nothing here yet'), page);
    assert.ok(!page.includes('privacy setup'), page);
  }],
  ['the overview lists sources, every import and each old bundle, and no daily values', async () => {
    const page = await load({ responses: { '/app/body/api/status': STATUS } });
    assert.ok(page.includes('Apple Health'), page);
    assert.ok(page.includes('Jan 10, 2026') && page.includes('Jan 12, 2026') && page.includes('3 days'), page);
    assert.ok(page.includes('saved'), page);
    assert.ok(page.includes('stopped partway: 3 of 120 days done.'), page);
    assert.ok(page.includes('still running: 5 of 40 days done.'), page);
    assert.ok(page.includes('import again from a new Apple Health export'), page);
    assert.ok(page.includes('no importer for Dexcom Clarity'), page);
    assert.ok(!/daily values|trends/i.test(page), page);
  }],
  ['a day shows each source with counts and an earlier-import line and no figures', async () => {
    const page = await load({ day: '20260110', responses: { '/app/body/api/day/20260110': DAY } });
    for (const expected of ['running', '6:31 AM', '8.1 km', '2 readings', '9:10 PM', '3,200.5', 'headache', 'readiness', 'Morning Run', 'delete']) {
      assert.ok(page.includes(expected), `${expected}: ${page}`);
    }
    assert.ok(page.includes('import again from your Oura account'), page);
    assert.ok(!/\bmax\b|average|glucose/.test(page), page);
  }],
  ['a day only an earlier import holds leads with that and never says nothing', async () => {
    const page = await load({ day: '20260315', responses: { '/app/body/api/day/20260315': EMPTY_DAY } });
    assert.ok(!page.includes('nothing for this day'), page);
    assert.ok(page.includes('March 2026'), page);
    assert.ok(page.indexOf('March 2026') < page.indexOf('nearby days'), page);
    assert.ok(!page.includes("this day's"), page);
  }],
  ['a day nothing holds says so', async () => {
    const page = await load({ day: '20260316', responses: { '/app/body/api/day/20260316': { ...EMPTY_DAY, day: '20260316', earlier_imports: [] } } });
    assert.ok(page.includes('nothing for this day'), page);
  }],
];

(async () => {
  for (const [name, run] of cases) {
    try {
      await run();
    } catch (error) {
      console.error(`FAILED: ${name}`);
      console.error(error);
      process.exit(1);
    }
  }
  console.log(`DOM CASES: ${cases.length} passed`);
})();
