// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Mounts the body view the way the page does: the script only registers,
// and the cases below mount it. Reads go through the real store.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const rootDir = process.argv[2] || path.join(__dirname, '..');
const html = fs.readFileSync(path.join(rootDir, 'assets/body/workspace.html'), 'utf8');
const script = html.slice(html.lastIndexOf('<script>') + '<script>'.length, html.lastIndexOf('</script>'));

function escapeHtml(value) {
  return String(value ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function headingFrom(markup, owner) {
  const match = String(markup).match(/<h([12])\b[^>]*>([^<]*)/);
  if (!match) return null;
  return {
    text: match[2],
    hasAttribute(name) { return name === 'tabindex' && this.tab === true; },
    setAttribute(name) { if (name === 'tabindex') this.tab = true; },
    focus() { owner.ownerDocument.activeElement = this; },
    closest(selector) {
      return selector === '[hidden]' && owner.hidden ? owner : null;
    },
  };
}

function boot(fixtures = {}) {
  const reads = [];
  const writes = [];
  const loadings = [];
  const errors = [];
  const notices = [];
  const days = [];
  const dialogs = [];
  const docListeners = [];
  const queued = new Map();
  const shell = {
    attrs: {},
    toggleAttribute(name, on) { if (on) this.attrs[name] = ''; else delete this.attrs[name]; },
    removeAttribute(name) { delete this.attrs[name]; },
  };
  const document = {
    title: '',
    activeElement: null,
    ownerDocument: null,
    addEventListener(type, fn) { docListeners.push({ type, fn }); },
    removeEventListener(type, fn) {
      const index = docListeners.findIndex((row) => row.type === type && row.fn === fn);
      if (index >= 0) docListeners.splice(index, 1);
    },
    createElement() {
      const listeners = [];
      const node = {
        className: '',
        classList: { toggle() {} },
        attrs: {},
        innerHTML: '',
        removed: false,
        setAttribute(name, value) { this.attrs[name] = value; },
        getAttribute(name) { return this.attrs[name]; },
        querySelector() { return { focus() {} }; },
        addEventListener(type, fn) { listeners.push({ type, fn }); },
        removeEventListener(type, fn) {
          const index = listeners.findIndex((row) => row.type === type && row.fn === fn);
          if (index >= 0) listeners.splice(index, 1);
        },
        close() {},
        remove() {
          this.removed = true;
          const index = dialogs.indexOf(this);
          if (index >= 0) dialogs.splice(index, 1);
        },
        showModal() {},
        listeners,
        fire(selector) {
          const target = { closest: (sel) => (sel === selector ? target : null) };
          for (const row of [...listeners]) if (row.type === 'click') row.fn({ target, type: 'click' });
        },
      };
      return node;
    },
    querySelector(selector) { return selector === '.body-shell' ? shell : null; },
    querySelectorAll(selector) {
      const match = /data-strava-row="([^"]+)"/.exec(selector);
      if (!match) return [];
      return (viewRoot.rows || []).filter((row) => row.id === match[1] && !row.removed);
    },
  };
  document.ownerDocument = document;
  const viewRoot = {
    ownerDocument: document,
    hidden: true,
    rows: [],
    _html: '',
    get innerHTML() { return this._html; },
    set innerHTML(value) {
      this._html = String(value);
      const found = [];
      const list = {
        querySelector(selector) {
          return selector === '[data-strava-row]' ? found.find((row) => !row.removed) || null : null;
        },
        closest() { return card; },
      };
      const card = { removed: false, remove() { this.removed = true; } };
      for (const match of this._html.matchAll(/data-strava-row="([^"]+)"/g)) {
        found.push({
          id: match[1],
          removed: false,
          parentElement: list,
          remove() { this.removed = true; },
        });
      }
      this.rows = found;
    },
    focusables: {},
    querySelector(selector) {
      if (selector === 'h1, h2') return headingFrom(this._html, this);
      const named = /^\[(data-body-(?:un)?hide)\]$/.exec(selector);
      if (named && this._html.includes(`${named[1]}="`)) {
        if (!this.focusables[selector]) {
          this.focusables[selector] = {
            selector,
            hasAttribute() { return true; },
            setAttribute() {},
            focus() { document.activeElement = this; },
          };
        }
        return this.focusables[selector];
      }
      return null;
    },
  };
  const retry = {
    fn: null,
    addEventListener(_type, fn) { this.fn = fn; },
    click() { const fn = this.fn; this.fn = null; if (fn) fn(); },
  };
  const gate = {
    hidden: true,
    innerHTML: '',
    querySelector(selector) {
      return selector === '.surface-state-retry' && this.innerHTML.includes('surface-state-retry') ? retry : null;
    },
    addEventListener() {},
  };
  const main = {
    id: 'main-content',
    tab: true,
    hasAttribute(name) { return name === 'tabindex' && this.tab === true; },
    setAttribute(name) { if (name === 'tabindex') this.tab = true; },
    focus() { document.activeElement = this; },
  };
  document.getElementById = (id) => {
    if (document.missingRoot && (id === 'body-view-root' || id === 'body-state-gate')) return null;
    if (id === 'body-view-root') return viewRoot;
    if (id === 'body-state-gate') return gate;
    if (id === 'main-content') return main;
    return null;
  };
  document.body = {
    appendChild(node) { dialogs.push(node); return node; },
  };
  const window = {
    AppServices: {
      escapeHtml,
      notifications: { show(options) { notices.push(options); return options; } },
    },
    SurfaceState: {
      loading(options) { loadings.push(options); return `loading ${options.text}`; },
      error(options) {
        errors.push(options);
        return '<button class="surface-state-retry">try again</button>';
      },
    },
    CONVEY_COPY: { RELOAD_HINT: 'reload' },
    DateNav: { setDay(day) { days.push(day); } },
    location: { href: 'http://journal.test/app/body/', origin: 'http://journal.test', pathname: '/app/body/' },
    apiJson(url) {
      reads.push(url);
      const queue = queued.get(url);
      if (queue && queue.length) return queue.shift();
      if (Object.prototype.hasOwnProperty.call(fixtures, url)) return Promise.resolve(fixtures[url]);
      return Promise.reject(new Error(`unexpected ${url}`));
    },
    fetch(url, options = {}) {
      writes.push({ url: String(url), method: options.method, body: options.body });
      let resolve;
      let reject;
      const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
      writes[writes.length - 1].resolve = resolve;
      writes[writes.length - 1].reject = reject;
      return promise;
    },
    logError() {},
  };
  const context = vm.createContext({
    window,
    document,
    console,
    AbortController,
    AbortSignal,
    DOMException,
    URL,
    Promise,
    JSON,
    Map,
    Set,
    WeakRef,
    Error,
    TypeError,
    Number,
    String,
    Array,
    Object,
    Date,
    Math,
    Boolean,
    encodeURIComponent,
    setTimeout,
    clearTimeout,
    CSS: { escape: (value) => value },
  });
  for (const name of ['store.js', 'views.js']) {
    vm.runInContext(
      fs.readFileSync(path.join(rootDir, 'assets/static', name), 'utf8'),
      context,
      { filename: name },
    );
  }
  vm.runInContext(script, context, { filename: 'body/workspace.html' });
  return {
    window,
    document,
    context,
    root: viewRoot,
    gate,
    main,
    shell,
    reads,
    writes,
    loadings,
    errors,
    notices,
    days,
    dialogs,
    listeners: () => docListeners.length,
    click(target) {
      for (const row of [...docListeners]) if (row.type === 'click') row.fn({ target, type: 'click' });
    },
    defer(url) {
      let resolve;
      let reject;
      const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
      const queue = queued.get(url) || [];
      queue.push(promise);
      queued.set(url, queue);
      return { resolve, reject };
    },
    async settle() {
      for (let i = 0; i < 8; i += 1) await new Promise((resolve) => setImmediate(resolve));
    },
    location(day) {
      return { pathname: day ? `/app/body/${day}` : '/app/body/', search: '', hash: '' };
    },
    async mount(day) {
      return context.window.SolViews.mount('body', { location: this.location(day) });
    },
    update(day) {
      return context.window.SolViews.update({ day }, { location: this.location(day) });
    },
    toggle(attribute, day) {
      return {
        disabled: false,
        getAttribute(name) { return name === attribute ? day : ''; },
        closest(selector) { return selector === `[${attribute}]` ? this : null; },
      };
    },
    button(id) {
      return {
        isConnected: true,
        getAttribute(name) {
          if (name === 'data-strava-delete') return id;
          if (name === 'data-strava-start') return '7:00 AM';
          return '';
        },
        closest(selector) { return selector === '[data-strava-delete]' ? this : null; },
        focus() { document.activeElement = this; },
      };
    },
  };
}

async function load({ day, responses }) {
  const page = boot(responses);
  await page.mount(day);
  await page.settle();
  assert.strictEqual(page.gate.innerHTML, '', 'no error or loading state left on the gate');
  return page.root.innerHTML;
}

function moved(id = 't1') {
  return { ok: true, status: 200, json: async () => ({ trash: { id, erase_at: '2026-02-01T00:00:00Z' } }) };
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

const NOTHING_DAY = { ...EMPTY_DAY, day: '20260316', earlier_imports: [] };

function answered(body, ok = true) {
  return { ok, status: ok ? 200 : 409, json: async () => body };
}

const HIDDEN_DAY = {
  day: '20261103',
  day_label: 'November 3, 2026',
  hidden: true,
  hidden_unreadable: false,
  hidden_with: [],
  has_data: false,
  nearest: { prev: '20261102', prev_label: 'Nov 2', next: '20261104', next_label: 'Nov 4' },
};

const DAY_AFTER = {
  ...DAY,
  day: '20261104',
  hidden: false,
  hidden_unreadable: false,
  hidden_with: [{ day: '20261103', label: 'Nov 3, 2026' }],
  earlier_imports: [],
};

const keyed = (page, key) => new RegExp(`data-copy-key="${key}"`).test(page);

function gridOf(cells) {
  return [{ year: 2026, month_labels: [], weeks: [cells.concat(Array(7 - cells.length).fill(null))] }];
}

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
    const page = await load({ day: '20260316', responses: { '/app/body/api/day/20260316': NOTHING_DAY } });
    assert.ok(page.includes('nothing for this day'), page);
  }],
  ['nothing runs until the view is mounted, and a loading line is not busy', async () => {
    const page = boot({ '/app/body/api/status': EMPTY_STATUS });
    assert.strictEqual(page.listeners(), 0, 'no document listener at load');
    assert.strictEqual(page.reads.length, 0, 'no read at load');
    assert.strictEqual(page.root.innerHTML, '');
    assert.strictEqual(page.window.SolViews.registered('body'), true);
    await page.mount();
    assert.strictEqual(page.loadings[0].busy, false);
    assert.strictEqual(page.loadings[0].text, 'loading body data…');
    await page.settle();
    assert.ok(page.root.innerHTML.includes('nothing here yet'));
  }],
  ['dispose takes the listener and an open dialog, and a late read draws nothing', async () => {
    const page = boot();
    const status = page.defer('/app/body/api/status');
    await page.mount();
    assert.strictEqual(page.listeners(), 1);
    page.click(page.button('9001'));
    assert.strictEqual(page.dialogs.length, 1);
    const dialog = page.dialogs[0];
    page.window.SolViews.dispose();
    assert.strictEqual(page.listeners(), 0);
    assert.strictEqual(page.dialogs.length, 0, 'the dialog is gone');
    assert.strictEqual(dialog.listeners.length, 0, 'a closed dialog does not keep its handlers');
    status.resolve(STATUS);
    await page.settle();
    assert.strictEqual(page.root.innerHTML, '', 'a cut-off read does not draw');
    assert.strictEqual(page.window.SolViews.current(), null);
  }],
  ['a mount that throws part-way takes back the listener it added', async () => {
    const page = boot();
    page.document.missingRoot = true;
    await assert.rejects(() => page.mount(), /body has no root/);
    assert.strictEqual(page.listeners(), 0);
    assert.strictEqual(page.window.SolViews.current(), null);
  }],
  ['overview to a day to another day is an update, and a late answer does not draw', async () => {
    const page = boot();
    const status = page.defer('/app/body/api/status');
    const dayA = page.defer('/app/body/api/day/20260110');
    const dayB = page.defer('/app/body/api/day/20260316');
    await page.mount();
    const id = page.window.SolViews.current().id;
    assert.strictEqual(page.update('20260110'), true);
    assert.strictEqual(page.update('20260316'), true);
    assert.strictEqual(page.window.SolViews.current().id, id, 'the same mount took both days');
    assert.deepStrictEqual(page.days, ['20260110', '20260316']);
    dayB.resolve(NOTHING_DAY);
    await page.settle();
    assert.ok(page.root.innerHTML.includes('nothing for this day'), page.root.innerHTML);
    status.resolve(STATUS);
    dayA.resolve(DAY);
    await page.settle();
    assert.ok(!page.root.innerHTML.includes('sources held'), page.root.innerHTML);
    assert.ok(!page.root.innerHTML.includes('Morning Run'), page.root.innerHTML);
    assert.ok(page.root.innerHTML.includes('nothing for this day'));
  }],
  ['arriving at a day from the overview focuses the day\'s heading, and a day step does not', async () => {
    const page = boot({ '/app/body/api/status': STATUS, '/app/body/api/day/20260316': NOTHING_DAY });
    await page.mount();
    await page.settle();
    const link = { focus() { page.document.activeElement = link; } };
    link.focus();
    const day = page.defer('/app/body/api/day/20260110');
    page.update('20260110');
    assert.strictEqual(page.document.activeElement, page.main, 'focus leaves the link that is about to go');
    day.resolve(DAY);
    await page.settle();
    assert.strictEqual(page.document.activeElement.text, 'Apple Health');
    const pin = { focus() { page.document.activeElement = pin; } };
    pin.focus();
    page.update('20260316');
    await page.settle();
    assert.strictEqual(page.document.activeElement, pin, 'a day step leaves focus alone');
    assert.ok(page.root.innerHTML.includes('nothing for this day'));
  }],
  ['an empty day arrived from the overview focuses the workspace', async () => {
    const page = boot({ '/app/body/api/status': STATUS, '/app/body/api/day/20260316': NOTHING_DAY });
    await page.mount();
    await page.settle();
    page.update('20260316');
    await page.settle();
    assert.strictEqual(page.document.activeElement, page.main);
    assert.ok(page.root.innerHTML.includes('nothing for this day'));
  }],
  ['a closed dialog drops its handlers and the button that opened it', async () => {
    const page = boot({ '/app/body/api/day/20260110': DAY });
    await page.mount('20260110');
    await page.settle();
    vm.runInContext(`window.__trigger = {
      isConnected: true,
      getAttribute(name) {
        if (name === 'data-strava-delete') return '9001';
        if (name === 'data-strava-start') return '7:00 AM';
        return '';
      },
      closest(selector) { return selector === '[data-strava-delete]' ? this : null; },
      focus() { document.activeElement = this; },
    };`, page.context);
    page.click(page.window.__trigger);
    const dialog = page.dialogs[0];
    assert.strictEqual(dialog.listeners.length, 2);
    dialog.fire('[data-workout-cancel]');
    assert.strictEqual(dialog.listeners.length, 0);
    assert.strictEqual(page.dialogs.length, 0);
    page.document.activeElement = null;
    vm.runInContext('window.__ref = new WeakRef(window.__trigger); window.__trigger = null;', page.context);
    await page.settle();
    gc();
    gc();
    assert.strictEqual(vm.runInContext('window.__ref.deref() === undefined', page.context), true);
  }],
  ['a delete in flight still finishes after the view is gone, and its notice does not keep the view', async () => {
    const page = boot({ '/app/body/api/day/20260110': DAY });
    await page.mount('20260110');
    await page.settle();
    const row = page.root.rows[0];
    page.click(page.button('9001'));
    page.dialogs[0].fire('[data-workout-move]');
    assert.strictEqual(page.writes[0].method, 'DELETE');
    assert.ok(page.writes[0].url.endsWith('/workout/9001'), page.writes[0].url);
    page.window.SolViews.dispose();
    page.writes[0].resolve(moved());
    await page.settle();
    assert.strictEqual(row.removed, false, 'the row of a day the owner left stays');
    assert.strictEqual(page.notices[0].title, 'moved to the trash');
    const before = page.reads.length;
    page.notices[0].buttons[0].onClick();
    await page.settle();
    assert.strictEqual(page.writes[1].method, 'POST');
    assert.ok(page.writes[1].url.endsWith('/trash/recover'), page.writes[1].url);
    assert.ok(page.writes[1].body.includes('t1'));
    assert.strictEqual(page.reads.length, before, 'recover does not read a view that is gone');
  }],
  ['an open notice does not keep a disposed mount', async () => {
    const page = boot({ '/app/body/api/day/20260110': DAY });
    await page.mount('20260110');
    await page.settle();
    page.click(page.button('9001'));
    page.dialogs[0].fire('[data-workout-move]');
    page.writes[0].resolve(moved());
    await page.settle();
    assert.strictEqual(page.notices[0].title, 'moved to the trash');
    vm.runInContext('{ window.__ref = new WeakRef(window.SolViews.current().ctx); }', page.context);
    page.window.SolViews.dispose();
    await page.settle();
    gc();
    gc();
    assert.strictEqual(vm.runInContext('window.__ref.deref() === undefined', page.context), true);
    assert.strictEqual(page.notices.length, 1, 'the notice is still open');
    const htmlBefore = page.root.innerHTML;
    page.notices[0].buttons[0].onClick();
    await page.settle();
    assert.strictEqual(page.root.innerHTML, htmlBefore, 'recover does not draw on the view that was left');
  }],
  ['a delete started on one day does not draw on the next, and recover from that day does not reload it', async () => {
    const page = boot({ '/app/body/api/day/20260110': DAY, '/app/body/api/day/20260316': NOTHING_DAY });
    await page.mount('20260110');
    await page.settle();
    page.click(page.button('9001'));
    page.dialogs[0].fire('[data-workout-move]');
    page.update('20260316');
    await page.settle();
    assert.ok(page.root.innerHTML.includes('nothing for this day'), page.root.innerHTML);
    page.writes[0].resolve(moved());
    await page.settle();
    assert.ok(page.root.innerHTML.includes('nothing for this day'));
    assert.ok(!page.root.innerHTML.includes('Morning Run'), page.root.innerHTML);
    assert.strictEqual(page.notices[0].title, 'moved to the trash');
    const html = page.root.innerHTML;
    const reads = page.reads.length;
    page.notices[0].buttons[0].onClick();
    await page.settle();
    assert.strictEqual(page.writes[1].method, 'POST');
    assert.strictEqual(page.root.innerHTML, html, 'recover does not reload the day now showing');
    assert.strictEqual(page.reads.length, reads);
    assert.strictEqual(page.gate.hidden, true);
  }],
  ['a hidden day draws its mark with unhide, its nearby days, and nothing of the day', async () => {
    const page = await load({ day: '20261103', responses: { '/app/body/api/day/20261103': HIDDEN_DAY } });
    assert.ok(/<h2[^>]*data-copy-key="markHead"/.test(page), page);
    assert.ok(keyed(page, 'markLine1') && keyed(page, 'markLine2'), page);
    assert.ok(/<button[^>]*data-body-unhide="20261103"[^>]*data-copy-key="markUnhide"/.test(page), page);
    assert.ok(/role="status"/.test(page), page);
    assert.ok(page.includes('href="/app/body/20261102"') && page.includes('href="/app/body/20261104"'), page);
    assert.ok(!page.includes('data-body-hide='), 'a hidden day has no hide button');
    assert.ok(!page.includes('body-source-section') && !keyed(page, 'unreadable'), page);
  }],
  ['a day with data has the hide button, and a day with nothing has none', async () => {
    const page = await load({ day: '20260110', responses: { '/app/body/api/day/20260110': DAY } });
    assert.ok(/<button[^>]*data-body-hide="20260110"[^>]*data-copy-key="hide"[^>]*aria-label="[^"]+"/.test(page), page);
    assert.ok(page.includes('body-day-nav'), page);
    const empty = await load({ day: '20260316', responses: { '/app/body/api/day/20260316': NOTHING_DAY } });
    assert.ok(!empty.includes('data-body-hide='), empty);
  }],
  ['the day after a hidden one says which day took part of a night, above its data', async () => {
    const page = await load({ day: '20261104', responses: { '/app/body/api/day/20261104': DAY_AFTER } });
    const line = /<p[^>]*data-copy-key="dayAfter"[^>]*>.*?<\/p>/.exec(page);
    assert.ok(line, page);
    assert.ok(line[0].includes('href="/app/body/20261103"'), line[0]);
    assert.ok(page.indexOf('data-copy-key="dayAfter"') < page.indexOf('body-source-section'), page);
    assert.ok(page.includes('data-body-hide="20261104"'), 'the day after can still be hidden');
    const bare = await load({ day: '20261104', responses: { '/app/body/api/day/20261104': { ...NOTHING_DAY, day: '20261104', hidden_with: DAY_AFTER.hidden_with } } });
    assert.ok(bare.indexOf('data-copy-key="dayAfter"') >= 0, bare);
    assert.ok(bare.indexOf('data-copy-key="dayAfter"') < bare.indexOf('class="body-empty"'), bare);
    const plain = await load({ day: '20260110', responses: { '/app/body/api/day/20260110': DAY } });
    assert.ok(!keyed(plain, 'dayAfter'), plain);
  }],
  ['when the hidden days can\'t be read, the day and overview say so, draw everything and offer no hide', async () => {
    const page = await load({ day: '20260110', responses: { '/app/body/api/day/20260110': { ...DAY, hidden_unreadable: true } } });
    assert.ok(/class="[^"]*body-freshness[^"]*"[^>]*role="status"[^>]*data-copy-key="unreadable"/.test(page), page);
    assert.ok(page.includes('body-source-section'), page);
    assert.ok(!page.includes('data-body-hide='), page);
    const overview = await load({ responses: { '/app/body/api/status': { ...STATUS, hidden_unreadable: true } } });
    assert.ok(keyed(overview, 'unreadable'), overview);
    const plain = await load({ responses: { '/app/body/api/status': STATUS } });
    assert.ok(!keyed(plain, 'unreadable'), plain);
  }],
  ['a hidden day\'s cell is striped and opens its day, with the footer only when one is', async () => {
    const cells = [
      { day: '20260110', count: 1, intensity: 0.33, title: 't1', earlier: false, hidden: false },
      { day: '20260111', count: 0, intensity: 0, title: 't2', earlier: false, hidden: true },
    ];
    const page = await load({ responses: { '/app/body/api/status': { ...STATUS, day_grid: gridOf(cells) } } });
    assert.ok(/<a class="body-day-cell body-day-cell--hidden" href="\/app\/body\/20260111" title="t2" aria-label="t2"/.test(page), page);
    assert.ok(/<p class="body-card-footer" data-copy-key="gridFoot">/.test(page), page);
    const open = await load({ responses: { '/app/body/api/status': { ...STATUS, day_grid: gridOf([cells[0]]) } } });
    assert.ok(!keyed(open, 'gridFoot') && !open.includes('body-day-cell--hidden'), open);
    // Every day hidden: no coverage, and the grid still shows where unhide is.
    const all = await load({ responses: { '/app/body/api/status': { ...EMPTY_STATUS, day_grid: gridOf([cells[1]]) } } });
    assert.ok(all.includes('body-day-cell--hidden') && !all.includes('nothing here yet'), all);
  }],
  ['a hide redraws the day as hidden, moves focus to its unhide, and its receipt unhides', async () => {
    const fixtures = { '/app/body/api/day/20260110': DAY };
    const page = boot(fixtures);
    await page.mount('20260110');
    await page.settle();
    const button = page.toggle('data-body-hide', '20260110');
    page.click(button);
    assert.strictEqual(button.disabled, true, 'a second tap waits for the first');
    assert.strictEqual(page.writes[0].method, 'POST');
    assert.ok(page.writes[0].url.endsWith('/app/body/api/hidden'), page.writes[0].url);
    assert.deepStrictEqual(JSON.parse(page.writes[0].body), { days: ['20260110'], hidden: true });
    fixtures['/app/body/api/day/20260110'] = { ...HIDDEN_DAY, day: '20260110' };
    page.writes[0].resolve(answered({ days: ['20260110'], search_queued: true }));
    await page.settle();
    assert.ok(keyed(page.root.innerHTML, 'markHead'), page.root.innerHTML);
    assert.strictEqual(page.document.activeElement && page.document.activeElement.selector, '[data-body-unhide]');
    assert.strictEqual(page.notices.length, 1);
    const receipt = page.notices[0];
    assert.strictEqual(receipt.copyKey, 'receipt');
    assert.strictEqual(receipt.app, 'body');
    assert.strictEqual(receipt.icon, 'eye-off');
    assert.strictEqual(receipt.autoDismiss, 8000);
    assert.strictEqual(receipt.buttons.length, 1);
    fixtures['/app/body/api/day/20260110'] = DAY;
    receipt.buttons[0].onClick();
    await page.settle();
    assert.deepStrictEqual(JSON.parse(page.writes[1].body), { days: ['20260110'], hidden: false });
    page.writes[1].resolve(answered({ days: [], search_queued: true }));
    await page.settle();
    assert.strictEqual(page.notices.length, 1, 'an unhide shows no second card');
    assert.ok(page.root.innerHTML.includes('body-source-section'), page.root.innerHTML);
    assert.strictEqual(page.document.activeElement && page.document.activeElement.selector, '[data-body-hide]');
  }],
  ['a hide search wasn\'t told of stays up and can be tried again', async () => {
    const fixtures = { '/app/body/api/day/20260110': DAY };
    const page = boot(fixtures);
    await page.mount('20260110');
    await page.settle();
    page.click(page.toggle('data-body-hide', '20260110'));
    fixtures['/app/body/api/day/20260110'] = { ...HIDDEN_DAY, day: '20260110' };
    page.writes[0].resolve(answered({ days: ['20260110'], search_queued: false }));
    await page.settle();
    const receipt = page.notices[0];
    assert.strictEqual(receipt.copyKey, 'receiptNoSearch');
    assert.strictEqual(receipt.autoDismiss, undefined, 'it stays until dismissed');
    assert.strictEqual(receipt.buttons.length, 2);
    receipt.buttons[0].onClick();
    assert.deepStrictEqual(JSON.parse(page.writes[1].body), { days: ['20260110'], hidden: true });
    page.writes[1].resolve(answered({ days: ['20260110'], search_queued: true }));
    await page.settle();
    assert.strictEqual(page.notices[1].copyKey, 'receipt');
    receipt.buttons[1].onClick();
    assert.deepStrictEqual(JSON.parse(page.writes[2].body), { days: ['20260110'], hidden: false });
  }],
  ['a refused or unanswered hide or unhide says so and leaves the day as it was', async () => {
    const page = boot({ '/app/body/api/day/20260110': DAY, '/app/body/api/day/20261103': HIDDEN_DAY });
    await page.mount('20260110');
    await page.settle();
    const reads = page.reads.length;
    page.click(page.toggle('data-body-hide', '20260110'));
    page.writes[0].resolve(answered({ error: 'no', reason_code: 'body_hidden_unreadable' }, false));
    await page.settle();
    page.click(page.toggle('data-body-hide', '20260110'));
    page.writes[1].reject(new Error('offline'));
    await page.settle();
    page.update('20261103');
    await page.settle();
    page.click(page.toggle('data-body-unhide', '20261103'));
    page.writes[2].resolve(answered({ error: 'no' }, false));
    await page.settle();
    page.click(page.toggle('data-body-unhide', '20261103'));
    page.writes[3].reject(new Error('offline'));
    await page.settle();
    assert.deepStrictEqual(page.notices.map((notice) => notice.copyKey), ['hideFailed', 'noAnswerHide', 'unhideFailed', 'noAnswerUnhide']);
    assert.ok(page.notices.every((notice) => notice.icon === 'triangle-alert' && !notice.autoDismiss && !(notice.buttons || []).length));
    assert.strictEqual(page.reads.length, reads + 1, 'only the step to the hidden day read');
    assert.ok(keyed(page.root.innerHTML, 'markHead'), page.root.innerHTML);
  }],
  ['a hide finished after the owner left draws nothing, and its receipt still unhides', async () => {
    const page = boot({ '/app/body/api/day/20260110': DAY, '/app/body/api/day/20260316': NOTHING_DAY });
    await page.mount('20260110');
    await page.settle();
    page.click(page.toggle('data-body-hide', '20260110'));
    page.update('20260316');
    await page.settle();
    const reads = page.reads.length;
    page.writes[0].resolve(answered({ days: ['20260110'], search_queued: true }));
    await page.settle();
    assert.strictEqual(page.reads.length, reads, 'the day now showing is not read again');
    assert.ok(page.root.innerHTML.includes('class="body-empty"'), page.root.innerHTML);
    assert.ok(!keyed(page.root.innerHTML, 'markHead'), page.root.innerHTML);
    page.notices[0].buttons[0].onClick();
    assert.deepStrictEqual(JSON.parse(page.writes[1].body), { days: ['20260110'], hidden: false });
  }],
  ['a failed read can be tried again', async () => {
    const page = boot();
    const first = page.defer('/app/body/api/status');
    await page.mount();
    first.reject(new Error('offline'));
    await page.settle();
    assert.strictEqual(page.errors[0].heading, "couldn't load your body data");
    assert.ok(page.gate.innerHTML.includes('surface-state-retry'));
    page.defer('/app/body/api/status').resolve(EMPTY_STATUS);
    page.gate.querySelector('.surface-state-retry').click();
    await page.settle();
    assert.ok(page.root.innerHTML.includes('nothing here yet'), page.root.innerHTML);
    assert.strictEqual(page.gate.innerHTML, '');
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
