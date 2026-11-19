// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const path = require('path');
const { journalContext } = require('./journal_catalog_dom.js');
globalThis.JournalI18n = journalContext({}).JournalI18n;
const year = require(path.join(__dirname, '../assets/year/year.js'));

function matches(node, sel) {
  if (sel.startsWith('.')) return node.className.split(/\s+/).includes(sel.slice(1));
  if (sel.startsWith('[')) {
    const eq = sel.match(/^\[([^\]]+)="([^"]*)"\]$/);
    if (eq) return node.attrs[eq[1]] === eq[2];
    const has = sel.match(/^\[([^\]]+)\]$/);
    return Boolean(has && Object.prototype.hasOwnProperty.call(node.attrs, has[1]));
  }
  return node.tagName === sel.toUpperCase();
}

function walk(node, out) {
  node.children.forEach((child) => {
    out.push(child);
    walk(child, out);
  });
  return out;
}

let focused = null;

function make(tag) {
  const node = {
    tagName: String(tag).toUpperCase(),
    className: '',
    children: [],
    attrs: {},
    listeners: {},
    text: '',
    parent: null,
    value: '',
  };
  Object.defineProperty(node, 'textContent', {
    get() {
      return node.text + node.children.map((child) => child.textContent).join('');
    },
    set(value) {
      node.text = value == null ? '' : String(value);
      node.children.forEach((child) => { child.parent = null; });
      node.children = [];
    },
  });
  node.setAttribute = (key, value) => { node.attrs[key] = String(value); };
  node.removeAttribute = (key) => { delete node.attrs[key]; };
  node.getAttribute = (key) => (Object.prototype.hasOwnProperty.call(node.attrs, key) ? node.attrs[key] : null);
  node.hasAttribute = (key) => Object.prototype.hasOwnProperty.call(node.attrs, key);
  // As in a page, a node has one parent: appending it again moves it.
  node.appendChild = (child) => {
    if (child.parent) child.parent.children = child.parent.children.filter((item) => item !== child);
    child.parent = node;
    node.children.push(child);
    return child;
  };
  node.addEventListener = (type, fn) => { (node.listeners[type] = node.listeners[type] || []).push(fn); };
  node.fire = (type, event) => (node.listeners[type] || []).forEach((fn) => fn(event || { preventDefault() {} }));
  node.click = () => node.fire('click');
  node.focus = () => { focused = node; };
  node.querySelectorAll = (sel) => walk(node, []).filter((item) => matches(item, sel));
  node.querySelector = (sel) => node.querySelectorAll(sel)[0] || null;
  return node;
}

function shown(node) {
  for (let at = node; at; at = at.parent) if (at.hasAttribute('hidden')) return false;
  return true;
}

const PAYLOAD = {
  year: '2025',
  years: ['2024', '2025'],
  today: '20261117',
  layers: {
    activities: { 20250105: 3, 20250106: 1 },
    location: { 20250105: 40 },
  },
  places: [
    { id: 'aa', label: 'the bakery', days: ['20250105', '20250106', '20250107', '20250301'] },
  ],
  names: [
    { type: 'Person', name: 'Ben Okri', days: ['20250105'] },
    { type: 'Company', name: 'Acme', days: ['20250210'] },
    { type: 'Person', name: 'Ada Wong', days: ['20250301', '20250302'] },
  ],
  places_unavailable: false,
  names_unavailable: false,
};

function draw(read, search, extra) {
  const root = make('main');
  const doc = { createElement: make };
  const went = [];
  const grids = [];
  const opts = Object.assign({
    go: (href) => went.push(href),
    reload: () => { went.push('reload'); },
    dayGrid: {
      mount: (host, options) => grids.push(options),
      legend: () => {},
    },
  }, extra);
  const readiness = year.render(doc, root, read, year.stateFromSearch(search || ''), opts);
  return { root, went, grids, readiness };
}

const ok = (body) => ({ status: 200, body: Object.assign({}, PAYLOAD, body) });

let cases = 0;
// Cases run one after another: the mounted ones share the page's globals.
let chain = Promise.resolve();
function test(name, fn) {
  chain = chain.then(async () => {
    try {
      await fn();
      cases += 1;
    } catch (error) {
      console.error(`FAILED: ${name}`);
      throw error;
    }
  });
}

// A mounted view over a page that answers `answers` in order. `settle` lets
// the reads in flight answer.
function mounted(search, answers) {
  const root = make('main');
  const asked = [];
  const entries = [];
  const ready = [];
  const grids = [];
  const listeners = {};
  const disposers = [];
  const waits = [];
  focused = null;
  globalThis.document = {
    createElement: make,
    getElementById: (id) => (id === 'year-root' ? root : null),
    get activeElement() { return focused; },
  };
  globalThis.location = { search };
  // As the grid does: it refuses a mount that would navigate with nowhere to go.
  globalThis.DayGrid = {
    mount: (host, options) => {
      if ((options.mode || 'navigate') === 'navigate' && !options.appPath) throw new Error('DayGrid.mount requires appPath');
      grids.push(options);
      return options.data ? host : null;
    },
    legend: () => {},
  };
  const ctx = {
    disposed: false,
    fetch: (url) => {
      asked.push(url);
      const answer = answers.shift();
      const wait = (answer.after || Promise.resolve()).then(() => ({ status: answer.status, json: async () => answer.body }));
      waits.push(wait);
      return wait;
    },
    ready: (state) => ready.push(state),
    on: (target, type, fn) => { listeners[type] = fn; },
    onDispose: (fn) => disposers.push(fn),
    history: {
      push: (url) => { entries.push(['push', url]); globalThis.location.search = url.includes('?') ? url.slice(url.indexOf('?')) : ''; },
      replace: (url) => { entries.push(['replace', url]); globalThis.location.search = url.includes('?') ? url.slice(url.indexOf('?')) : ''; },
    },
  };
  year.mount(ctx);
  const settle = async () => {
    await Promise.all(waits.slice());
    for (let turn = 0; turn < 5; turn += 1) await new Promise((resolve) => setImmediate(resolve));
  };
  return { root, asked, entries, ready, grids, listeners, disposers, ctx, settle };
}

test('week columns start on sunday and january 1 is in the first', () => {
  // 2025 starts on a Wednesday.
  assert.strictEqual(year.weekIndex('20250101'), 0);
  assert.strictEqual(year.weekIndex('20250104'), 0);
  assert.strictEqual(year.weekIndex('20250105'), 1);
  assert.strictEqual(year.weekIndex('20251231'), 52);
  assert.strictEqual(year.weekCount('2025'), 53);
  // 2028 is a leap year that starts on a Saturday: 54 columns.
  assert.strictEqual(year.weekCount('2028'), 54);
  assert.deepStrictEqual(year.weekSpan('2025', 0), { from: '20250101', to: '20250104' });
  assert.deepStrictEqual(year.weekSpan('2025', 1), { from: '20250105', to: '20250111' });
  assert.deepStrictEqual(year.weekSpan('2025', 52), { from: '20251228', to: '20251231' });
});

test('the address query is the state, and defaults stay out of it', () => {
  assert.deepStrictEqual(year.stateFromSearch(''), { year: null, view: 'days', layer: 'activities' });
  assert.deepStrictEqual(year.stateFromSearch('?year=2024&view=threads&layer=location'), { year: '2024', view: 'threads', layer: 'location' });
  assert.deepStrictEqual(year.stateFromSearch('?year=24&view=weeks&layer=steps'), { year: null, view: 'days', layer: 'activities' });
  assert.strictEqual(year.urlFor({ year: null, view: 'days', layer: 'activities' }), '/app/year/');
  assert.strictEqual(year.urlFor({ year: '2024', view: 'threads', layer: 'location' }), '/app/year/?year=2024&view=threads&layer=location');
  assert.strictEqual(year.readUrl(null), '/app/year/api/view');
  assert.strictEqual(year.readUrl('2024'), '/app/year/api/view?year=2024');
});

test('days mounts the grid for one year, on the chosen count and its app', () => {
  const first = draw(ok(), '');
  assert.strictEqual(first.readiness, 'content');
  assert.strictEqual(first.grids.length, 1);
  assert.deepStrictEqual(first.grids[0].data.coverage, { start: '20250101', end: '20251231' });
  assert.deepStrictEqual(first.grids[0].data.days, PAYLOAD.layers.activities);
  assert.strictEqual(first.grids[0].appPath, '/app/activities');
  assert.strictEqual(first.grids[0].mode, 'navigate');
  assert.strictEqual(first.root.querySelector('.year-threads'), null);

  const second = draw(ok(), '?layer=location');
  assert.deepStrictEqual(second.grids[0].data.days, PAYLOAD.layers.location);
  assert.strictEqual(second.grids[0].appPath, '/app/map');
  assert.notDeepStrictEqual(second.grids[0].unit, first.grids[0].unit);

  // The current year's grid stops at today.
  const current = draw(ok({ year: '2026', years: ['2026'], layers: { activities: { 20260105: 1 }, location: {} } }), '');
  assert.deepStrictEqual(current.grids[0].data.coverage, { start: '20260101', end: '20261117' });
});

test('a count with nothing in the year mounts no grid', () => {
  const view = draw(ok({ layers: { activities: {}, location: {} } }), '');
  assert.strictEqual(view.grids.length, 0);
  assert.ok(view.root.querySelector('.year-none'));
});

test('the switch and the chips are links that move the address', () => {
  const view = draw(ok(), '');
  const tabs = view.root.querySelectorAll('.year-switch-item');
  assert.deepStrictEqual(tabs.map((tab) => tab.getAttribute('data-view')), ['days', 'threads']);
  assert.ok(tabs[0].hasAttribute('aria-current'));
  assert.ok(!tabs[1].hasAttribute('aria-current'));
  assert.strictEqual(tabs[1].getAttribute('href'), '/app/year/?year=2025&view=threads');
  tabs[1].click();
  view.root.querySelector('[data-layer="location"]').click();
  assert.deepStrictEqual(view.went, ['/app/year/?year=2025&view=threads', '/app/year/?year=2025&layer=location']);
  // A modified click is the browser's: a new tab, not a move in this one.
  tabs[1].fire('click', { metaKey: true, preventDefault() { throw new Error('prevented'); } });
  assert.strictEqual(view.went.length, 2);

  const threads = draw(ok(), '?view=threads');
  assert.ok(threads.root.querySelectorAll('.year-switch-item')[1].hasAttribute('aria-current'));
  assert.strictEqual(threads.root.querySelector('.year-days'), null);
  assert.strictEqual(threads.grids.length, 0);
});

test('the year stepper offers only held years and keeps the view', () => {
  const view = draw(ok(), '?view=threads');
  const steps = view.root.querySelectorAll('.year-step-link');
  assert.deepStrictEqual(steps.map((step) => step.getAttribute('href')), ['/app/year/?year=2024&view=threads']);
  const middle = draw(ok({ years: ['2024', '2025', '2026'] }), '');
  assert.deepStrictEqual(
    middle.root.querySelectorAll('.year-step-link').map((step) => step.getAttribute('href')),
    ['/app/year/?year=2024', '/app/year/?year=2026'],
  );
  const only = draw(ok({ years: ['2025'] }), '');
  assert.strictEqual(only.root.querySelector('.year-step'), null);
});

test('threads keeps the order the read gave, places first then kinds by name', () => {
  const view = draw(ok(), '?view=threads');
  const rows = view.root.querySelectorAll('.year-th-row').map((row) => row.getAttribute('data-thread'));
  assert.deepStrictEqual(rows, ['place:aa', 'name:company:Acme', 'name:person:Ben Okri', 'name:person:Ada Wong']);
  // One cell per held week, in the week's column.
  const bakery = view.root.querySelector('[data-thread="place:aa"]');
  assert.deepStrictEqual(bakery.querySelectorAll('.year-th-cell').map((cell) => cell.getAttribute('data-week')), ['1', '8']);
});

test('a week opens its days under its row, and steps along the row', () => {
  const view = draw(ok(), '?view=threads');
  const bakery = view.root.querySelector('[data-thread="place:aa"]');
  const cells = bakery.querySelectorAll('.year-th-cell');
  cells[0].click();
  let peek = view.root.querySelector('.year-peek');
  assert.strictEqual(peek.parent, bakery);
  assert.ok(cells[0].hasAttribute('data-selected'));
  assert.deepStrictEqual(peek.querySelectorAll('.year-peek-day').map((day) => day.getAttribute('href')),
    ['/app/map/20250105', '/app/map/20250106', '/app/map/20250107']);
  const [before, after] = peek.querySelectorAll('.year-peek-step-button');
  assert.ok(before.hasAttribute('disabled'));
  after.click();
  peek = view.root.querySelector('.year-peek');
  assert.deepStrictEqual(peek.querySelectorAll('.year-peek-day').map((day) => day.getAttribute('data-day')), ['20250301']);
  assert.ok(!cells[0].hasAttribute('data-selected'));
  assert.ok(cells[1].hasAttribute('data-selected'));

  // A name's days open on the activities day, and the one peek moves rows.
  const ada = view.root.querySelector('[data-thread="name:person:Ada Wong"]');
  ada.querySelector('.year-th-cell').click();
  assert.strictEqual(view.root.querySelectorAll('.year-peek').length, 1);
  peek = view.root.querySelector('.year-peek');
  assert.strictEqual(peek.parent, ada);
  assert.ok(!bakery.hasAttribute('data-selected'));
  assert.deepStrictEqual(peek.querySelectorAll('.year-peek-day').map((day) => day.getAttribute('href')),
    ['/app/activities/20250301']);
});

test('a row label opens its summary, and arrows walk its weeks', () => {
  const view = draw(ok(), '?view=threads');
  const bakery = view.root.querySelector('[data-thread="place:aa"]');
  const label = bakery.querySelector('.year-th-label');
  label.click();
  assert.strictEqual(view.root.querySelector('.year-peek').querySelectorAll('.year-peek-day').length, 0);
  view.root.querySelector('.year-peek-start').click();
  assert.strictEqual(view.root.querySelector('.year-peek').querySelectorAll('.year-peek-day').length, 3);
  label.fire('keydown', { key: 'ArrowRight', preventDefault() {} });
  assert.strictEqual(view.root.querySelector('.year-peek').querySelectorAll('.year-peek-day').length, 1);
  label.fire('keydown', { key: 'ArrowRight', preventDefault() {} });
  assert.strictEqual(view.root.querySelector('.year-peek').querySelectorAll('.year-peek-day').length, 1);
  label.fire('keydown', { key: 'ArrowLeft', preventDefault() {} });
  assert.strictEqual(view.root.querySelector('.year-peek').querySelectorAll('.year-peek-day').length, 3);
});

test('the find box hides rows and sections that do not match', () => {
  const view = draw(ok(), '?view=threads');
  const find = view.root.querySelector('.year-find');
  const visible = () => view.root.querySelectorAll('.year-th-row').filter(shown).map((row) => row.getAttribute('data-thread'));
  const headings = () => view.root.querySelectorAll('.year-th-section').filter(shown).length;
  const none = view.root.querySelectorAll('.year-none')[0];
  find.value = ' ADA ';
  find.fire('input');
  assert.deepStrictEqual(visible(), ['name:person:Ada Wong']);
  assert.strictEqual(headings(), 1);
  assert.ok(!shown(none));
  find.value = 'nobody';
  find.fire('input');
  assert.deepStrictEqual(visible(), []);
  assert.ok(shown(none));
  find.value = '';
  find.fire('input');
  assert.strictEqual(visible().length, 4);
  assert.strictEqual(headings(), 3);
});

test('what could not be listed is said, and the rest still shows', () => {
  const view = draw(ok({ names: [], names_unavailable: true }), '?view=threads');
  assert.strictEqual(view.root.querySelectorAll('.year-notice').length, 1);
  assert.strictEqual(view.root.querySelectorAll('.year-th-row').length, 1);
  const both = draw(ok({ names: [], places: [], names_unavailable: true, places_unavailable: true }), '?view=threads');
  assert.strictEqual(both.root.querySelectorAll('.year-notice').length, 2);
  assert.strictEqual(both.root.querySelectorAll('.year-th-row').length, 0);
  const neither = draw(ok(), '?view=threads');
  assert.strictEqual(neither.root.querySelectorAll('.year-notice').length, 0);
});

test('nothing held, a year with nothing, and a failed read are three states', () => {
  const empty = draw(ok({ years: [], layers: { activities: {}, location: {} }, places: [], names: [] }), '');
  assert.strictEqual(empty.readiness, 'empty');
  assert.ok(empty.root.querySelector('.year-empty'));
  assert.strictEqual(empty.root.querySelector('.year-switch'), null);

  const other = draw(ok({ year: '1999', layers: { activities: {}, location: {} }, places: [], names: [] }), '?year=1999');
  assert.strictEqual(other.readiness, 'empty');
  assert.strictEqual(other.root.querySelector('.year-empty'), null);
  assert.deepStrictEqual(other.root.querySelectorAll('.year-step-link').map((step) => step.getAttribute('href')), ['/app/year/?year=2024']);

  for (const read of [{ status: 503, body: { error: 'x' } }, { status: 0, body: null }, { status: 200, body: {} }]) {
    const failed = draw(read, '');
    assert.strictEqual(failed.readiness, 'error');
    assert.ok(failed.root.querySelector('.year-error'));
    assert.strictEqual(failed.grids.length, 0);
    failed.root.querySelector('.year-retry').click();
    assert.deepStrictEqual(failed.went, ['reload']);
  }
});

test('a failed read is asked again on retry, and draws when it answers', async () => {
  const asked = [];
  const answers = [{ status: 503, json: async () => ({}) }, { status: 200, json: async () => PAYLOAD }];
  const fetch = async (url) => { asked.push(url); return answers.shift(); };
  const first = await year.load({ fetch }, '2025');
  assert.strictEqual(first.status, 503);
  const second = await year.load({ fetch }, '2025');
  assert.deepStrictEqual(asked, ['/app/year/api/view?year=2025', '/app/year/api/view?year=2025']);
  assert.strictEqual(draw(second, '').readiness, 'content');
  const thrown = await year.load({ fetch: async () => { throw new Error('offline'); } }, null);
  assert.deepStrictEqual(thrown, { status: 0, body: null });
});

test('a mounted view reads once, moves by its own links, and keeps focus on them', async () => {
  const page = mounted('', [{ status: 200, body: PAYLOAD }]);
  await page.settle();
  assert.deepStrictEqual(page.asked, ['/app/year/api/view']);
  assert.deepStrictEqual(page.ready, ['content']);

  // The same view again takes the entry's place and reads nothing.
  const days = page.root.querySelector('[data-view="days"]');
  days.click();
  assert.deepStrictEqual(page.entries, [['replace', '/app/year/?year=2025']]);

  // Another view is a new entry, drawn from the read it has, with focus kept.
  const threads = page.root.querySelector('[data-view="threads"]');
  threads.focus();
  threads.click();
  assert.deepStrictEqual(page.entries[1], ['push', '/app/year/?year=2025&view=threads']);
  assert.ok(page.root.querySelector('.year-threads'));
  assert.strictEqual(focused.getAttribute('data-view'), 'threads');
  assert.notStrictEqual(focused, threads);
  assert.deepStrictEqual(page.asked.length, 1);
  assert.deepStrictEqual(page.ready, ['content']);

  // Going back restores what the address says.
  globalThis.location.search = '?year=2025';
  page.listeners.popstate();
  assert.ok(page.root.querySelector('.year-days'));
  assert.strictEqual(page.root.querySelector('.year-threads'), null);
});

test('a mounted view ends its grid when it draws again and when it goes', async () => {
  const page = mounted('', [{ status: 200, body: PAYLOAD }]);
  await page.settle();
  const live = () => page.grids.filter((options) => options.data).length;
  const ended = () => page.grids.filter((options) => !options.data).length;
  assert.strictEqual(live(), 1);
  page.root.querySelector('[data-view="threads"]').click();
  assert.strictEqual(live(), 1);
  assert.ok(ended() >= 2);
  const before = ended();
  page.disposers.forEach((fn) => fn());
  assert.strictEqual(ended(), before + 1);
});

test('another year reads again, and only the latest ask draws', async () => {
  let release;
  const slow = new Promise((resolve) => { release = resolve; });
  const page = mounted('?year=2024', [
    { status: 200, body: Object.assign({}, PAYLOAD, { year: '2024' }), after: slow },
    { status: 200, body: PAYLOAD },
  ]);
  // The owner moves on to 2025 before 2024 answers.
  globalThis.location.search = '?year=2025';
  page.listeners.popstate();
  release();
  await page.settle();
  assert.deepStrictEqual(page.asked, ['/app/year/api/view?year=2024', '/app/year/api/view?year=2025']);
  assert.strictEqual(page.root.querySelector('.year-title').textContent, '2025');
  assert.deepStrictEqual(page.ready, ['content']);
});

test('a mounted view that failed reads again on retry, and one that has gone draws nothing', async () => {
  const page = mounted('', [{ status: 503, body: {} }, { status: 200, body: PAYLOAD }]);
  await page.settle();
  assert.deepStrictEqual(page.ready, ['error']);
  page.root.querySelector('.year-retry').click();
  await page.settle();
  assert.strictEqual(page.asked.length, 2);
  assert.ok(page.root.querySelector('.year-days'));
  assert.deepStrictEqual(page.ready, ['error']);

  const gone = mounted('', [{ status: 200, body: PAYLOAD }]);
  gone.ctx.disposed = true;
  await gone.settle();
  assert.strictEqual(gone.root.children.length, 0);
  assert.deepStrictEqual(gone.ready, []);
});

test('kinds of name are in the order of their headings', () => {
  const view = draw(ok({ names: [
    { type: 'Tool', name: 'Lathe', days: ['20250105'] },
    { type: 'Place', name: 'The Yard', days: ['20250105'] },
    { type: 'Person', name: 'Ben Okri', days: ['20250105'] },
  ] }), '?view=threads');
  const titles = view.root.querySelectorAll('.year-th-section').map((heading) => heading.text.trim());
  assert.deepStrictEqual(titles.slice(1), titles.slice(1).slice().sort());
  assert.strictEqual(titles.length, 4);
});

chain.then(() => {
  console.log(`DOM CASES: ${cases} passed`);
}, (error) => {
  console.error(error);
  process.exit(1);
});
