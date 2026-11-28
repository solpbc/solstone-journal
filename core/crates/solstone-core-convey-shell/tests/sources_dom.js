// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const path = require('path');
const { journalContext } = require('./journal_catalog_dom.js');
globalThis.JournalI18n = journalContext({}).JournalI18n;
const sources = require(path.join(__dirname, '../assets/sources/sources.js'));

const NOW = Date.parse('2026-03-04T08:00:00Z');
const ZONE = 'America/Denver';
const MIN = 60 * 1000;

function iso(ms) {
  return new Date(ms).toISOString().replace('.000Z', 'Z');
}

function client(label, fields) {
  return Object.assign({
    display_label: label,
    owner_label: '',
    device_label: label,
    kind: 'cert',
    last_accepted_ingest_at: null,
    source_delivery: null,
    reported: { platform: 'linux', device_type: 'desktop' },
    capture_state: 'active',
    unassessed_reason: null,
  }, fields);
}

function at(ms, extra) {
  return Object.assign({
    state: 'current',
    elapsed_ms: 1,
    last_accepted_ingest_at: iso(ms),
    ingest_rejection: null,
  }, extra);
}

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

function make(tag) {
  const node = {
    tagName: String(tag).toUpperCase(),
    className: '',
    children: [],
    attrs: {},
    listeners: {},
    text: '',
  };
  Object.defineProperty(node, 'textContent', {
    get() {
      return node.text + node.children.map((child) => child.textContent).join('');
    },
    set(value) {
      node.text = value == null ? '' : String(value);
      node.children = [];
    },
  });
  node.setAttribute = (key, value) => { node.attrs[key] = String(value); };
  node.getAttribute = (key) => (Object.prototype.hasOwnProperty.call(node.attrs, key) ? node.attrs[key] : null);
  node.appendChild = (child) => { node.children.push(child); return child; };
  node.addEventListener = (type, fn) => { (node.listeners[type] = node.listeners[type] || []).push(fn); };
  node.click = () => Promise.all((node.listeners.click || []).map((fn) => fn()));
  node.querySelectorAll = (sel) => walk(node, []).filter((item) => matches(item, sel));
  node.querySelector = (sel) => node.querySelectorAll(sel)[0] || null;
  return node;
}

function documentFor() {
  const root = make('main');
  root.attrs.id = 'sources-root';
  return {
    root,
    doc: {
      createElement: make,
      getElementById: (id) => (id === 'sources-root' ? root : null),
    },
  };
}

function render(payload, layout, now) {
  const { doc, root } = documentFor();
  sources.render(doc, root, payload, { now: now == null ? NOW : now, timeZone: ZONE, layout: layout || 'wide' });
  return root;
}

function ok(clients) {
  return { status: 200, body: { clients } };
}

function deviceBox(root, label) {
  return root.querySelectorAll('.sources-device').find((node) => node.getAttribute('data-device') === label);
}

function cell(root, source, label) {
  const row = root.querySelectorAll('[data-source]').find((node) => node.getAttribute('data-source') === source && node.tagName === 'TR')
    || (deviceBox(root, label) && deviceBox(root, label).querySelectorAll('[data-source]').find((node) => node.getAttribute('data-source') === source));
  if (!row) return undefined;
  if (row.tagName === 'TR') {
    const found = row.querySelectorAll('td').find((td) => td.getAttribute('data-device') === label);
    return found ? found.textContent : undefined;
  }
  const found = row.querySelector('.sources-cell');
  return found ? found.textContent : '';
}

function rowLabels(root) {
  const seen = [];
  root.querySelectorAll('[data-source]').forEach((node) => {
    if (node.tagName !== 'TR' && node.tagName !== 'LI') return;
    const label = node.getAttribute('data-source');
    if (node.tagName === 'LI') {
      const box = deviceBox(root, node.parent && node.parent.parent && node.parent.parent.getAttribute ? node.parent.parent.getAttribute('data-device') : '');
      if (box && box !== root.querySelector('.sources-device') && box !== root.querySelectorAll('.sources-device')[0]) return;
    }
    if (seen.indexOf(label) === -1) seen.push(label);
  });
  return seen;
}

function count(root, phrase) {
  const text = root.textContent;
  let found = 0;
  let index = 0;
  while (text.indexOf(phrase, index) !== -1) {
    found += 1;
    index = text.indexOf(phrase, index) + phrase.length;
  }
  return found;
}

function controls(root) {
  return root.querySelectorAll('button').concat(root.querySelectorAll('input')).map((node) => node.textContent);
}

const tests = [];
function test(name, fn) { tests.push([name, fn]); }

test('relative scale uses the caller clock and zone', () => {
  assert.strictEqual(sources.relative(NOW - 59 * 1000, NOW, ZONE), 'just now');
  assert.strictEqual(sources.relative(NOW - 60 * 1000, NOW, ZONE), '1 min ago');
  assert.strictEqual(sources.relative(NOW - 90 * MIN, NOW, ZONE), '1 hour ago');
  assert.strictEqual(sources.relative(NOW - 119 * MIN, NOW, ZONE), '1 hour ago');
  assert.strictEqual(sources.relative(NOW + 5 * 3600000, NOW, ZONE), 'just now');
  assert.strictEqual(sources.relative('2026-03-03T06:00:00Z', NOW, ZONE), 'march 2');
  assert.notStrictEqual(sources.relative('2026-03-03T06:00:00Z', NOW, ZONE), 'yesterday');
  const evening = Date.parse('2026-03-04T06:30:00Z');
  assert.strictEqual(sources.relative('2026-03-02T19:00:00Z', evening, ZONE), 'yesterday');
  assert.strictEqual(sources.relative('2025-03-03T18:00:00Z', NOW, ZONE), 'march 3, 2025');
  assert.strictEqual(sources.relative('not-a-date', NOW, ZONE), '');
});

test('a named source five minutes before the clock reads 5 min ago', () => {
  const root = render(ok([client('studio', {
    last_accepted_ingest_at: iso(NOW - 5 * MIN),
    source_delivery: { audio: at(NOW - 5 * MIN) },
  })]));
  assert.strictEqual(cell(root, 'audio', 'studio'), '5 min ago');
});

test('rows follow the named order and fold only the mapped keys', () => {
  const root = render(ok([client('studio', {
    last_accepted_ingest_at: iso(NOW - 10 * MIN),
    source_delivery: {
      watch: at(NOW - 30 * MIN),
      'watch-audio': at(NOW - 10 * MIN),
      audio: at(NOW - 40 * MIN),
      tmux: at(NOW - 8 * MIN),
      terminal: at(NOW - 50 * MIN),
      browser: at(NOW - 20 * MIN),
      obsidian: at(NOW - 15 * MIN),
      'browser-safari': at(NOW - 12 * MIN),
      mic: at(NOW - 11 * MIN),
      share: at(NOW - 9 * MIN),
      mobile: at(NOW - 9 * MIN),
      'mobile-segment': at(NOW - 9 * MIN),
      '': at(NOW - 9 * MIN),
      screen: { state: 'unknown', elapsed_ms: null, last_accepted_ingest_at: 'not-a-date', ingest_rejection: null },
    },
  })]));
  assert.deepStrictEqual(rowLabels(root), ['audio', 'terminal', 'browsers', 'Obsidian', 'watch', 'browser safari', 'mic']);
  assert.strictEqual(cell(root, 'terminal', 'studio'), sources.relative(NOW - 8 * MIN, NOW, ZONE));
  assert.strictEqual(cell(root, 'watch', 'studio'), sources.relative(NOW - 10 * MIN, NOW, ZONE));
  assert.strictEqual(cell(root, 'audio', 'studio'), sources.relative(NOW - 40 * MIN, NOW, ZONE));
  assert.strictEqual(rowLabels(root).indexOf('tmux'), -1);
  assert.strictEqual(rowLabels(root).indexOf('screen'), -1);
  assert.strictEqual(root.textContent.includes('needs_attention'), false);
  assert.strictEqual(root.textContent.includes('cert'), false);
});

test('an unparseable sibling does not blank a parseable one', () => {
  const root = render(ok([client('studio', {
    last_accepted_ingest_at: iso(NOW - 8 * MIN),
    source_delivery: {
      terminal: at(NOW - 8 * MIN),
      tmux: { state: 'unknown', elapsed_ms: null, last_accepted_ingest_at: 'not-a-date', ingest_rejection: null },
    },
  })]));
  assert.strictEqual(cell(root, 'terminal', 'studio'), sources.relative(NOW - 8 * MIN, NOW, ZONE));
});

test('folds stay on the device that named the key', () => {
  const root = render(ok([
    client('studio', {
      last_accepted_ingest_at: iso(NOW - 20 * MIN),
      source_delivery: { browser: at(NOW - 20 * MIN) },
    }),
    client('phone', {
      last_accepted_ingest_at: iso(NOW - 5 * MIN),
      source_delivery: { browsers: at(NOW - 5 * MIN) },
      reported: { platform: 'ios', device_type: 'phone' },
    }),
  ]));
  assert.strictEqual(cell(root, 'browsers', 'studio'), sources.relative(NOW - 20 * MIN, NOW, ZONE));
  assert.strictEqual(cell(root, 'browsers', 'phone'), sources.relative(NOW - 5 * MIN, NOW, ZONE));
});

test('safari source maps into timestamp-only browsers cells without state words', () => {
  const root = render(ok([
    client('phone', {
      last_accepted_ingest_at: iso(NOW - 4 * MIN),
      source_delivery: {
        safari: {
          state: 'needs_attention',
          elapsed_ms: 4 * MIN,
          last_accepted_ingest_at: iso(NOW - 4 * MIN),
          ingest_rejection: { reason_code: 'browser_record_invalid', first: iso(NOW - MIN), latest: iso(NOW - MIN), active_count: 1 },
        },
      },
      reported: { platform: 'ios', device_type: 'phone' },
    }),
  ]));
  assert.strictEqual(cell(root, 'browsers', 'phone'), sources.relative(NOW - 4 * MIN, NOW, ZONE));
  assert.strictEqual(rowLabels(root).indexOf('safari'), -1);
  for (const stateWord of ['needs attention', 'needs_attention', 'paused', 'browser_record_invalid']) {
    assert.strictEqual(root.textContent.includes(stateWord), false, stateWord);
  }
});

test('an unnamed accept that says nothing it carried is the device time and not a source cell', () => {
  for (const key of ['', 'mobile-segment']) {
    const root = render(ok([client('studio', {
      last_accepted_ingest_at: iso(NOW - 5 * MIN),
      source_delivery: { [key]: at(NOW - 5 * MIN) },
    })]));
    assert.strictEqual(root.querySelector('table'), null, key);
    assert.strictEqual(deviceBox(root, 'studio').querySelector('.sources-device-time').textContent, '5 min ago');
    assert.strictEqual(count(root, 'nothing added yet'), 0, key);
    assert.strictEqual(rowLabels(root).indexOf('audio'), -1);
    assert.strictEqual(rowLabels(root).indexOf('screen'), -1);
    assert.strictEqual(rowLabels(root).indexOf('location'), -1);
  }
});

test('a device\'s own capture fills the rows its latest upload carried', () => {
  const root = render(ok([
    client('studio', {
      last_accepted_ingest_at: iso(NOW - 2 * MIN),
      source_delivery: {
        '': at(NOW - 2 * MIN, { carried: ['audio', 'screen'] }),
        browser: at(NOW - 30 * MIN),
      },
      reported: { platform: 'macos', device_type: 'desktop' },
    }),
    client('phone', {
      last_accepted_ingest_at: iso(NOW - 6 * MIN),
      source_delivery: { 'mobile-segment': at(NOW - 6 * MIN, { carried: ['audio', 'location'] }) },
      reported: { platform: 'ios', device_type: 'phone' },
    }),
  ]));
  assert.deepStrictEqual(rowLabels(root), ['audio', 'screen', 'browsers', 'location']);
  assert.strictEqual(cell(root, 'screen', 'studio'), '2 min ago');
  assert.strictEqual(cell(root, 'audio', 'studio'), '2 min ago');
  assert.strictEqual(cell(root, 'location', 'studio'), '');
  assert.strictEqual(cell(root, 'audio', 'phone'), '6 min ago');
  assert.strictEqual(cell(root, 'location', 'phone'), '6 min ago');
  assert.strictEqual(cell(root, 'screen', 'phone'), '');
  // The device time equals its newest cell, so the heading does not repeat it.
  assert.strictEqual(deviceBox(root, 'studio').querySelector('.sources-device-time'), null);
  assert.strictEqual(count(root, 'nothing added yet'), 0);
});

test('a named source newer than the own capture keeps its own time', () => {
  const root = render(ok([client('studio', {
    last_accepted_ingest_at: iso(NOW - 1 * MIN),
    source_delivery: {
      '': at(NOW - 20 * MIN, { carried: ['audio'] }),
      audio: at(NOW - 1 * MIN),
      share: at(NOW - 1 * MIN, { carried: ['screen'] }),
    },
  })]));
  assert.strictEqual(cell(root, 'audio', 'studio'), '1 min ago');
  assert.strictEqual(rowLabels(root).indexOf('screen'), -1);
});

test('another device does not blank this device time', () => {
  const root = render(ok([
    client('studio', {
      last_accepted_ingest_at: iso(NOW - 119 * MIN),
      source_delivery: { 'mobile-segment': at(NOW - 119 * MIN) },
    }),
    client('phone', {
      last_accepted_ingest_at: iso(NOW - 90 * MIN),
      source_delivery: { browser: at(NOW - 90 * MIN) },
      reported: { platform: 'android', device_type: 'phone' },
    }),
  ]));
  assert.strictEqual(cell(root, 'browsers', 'studio'), '');
  assert.strictEqual(deviceBox(root, 'studio').querySelector('.sources-device-time').textContent, '1 hour ago');
  assert.strictEqual(count(root, 'nothing added yet'), 0);
  assert.strictEqual(deviceBox(root, 'studio').textContent.includes('desktop'), false);
});

test('the same words still show when the device time is newer', () => {
  const root = render(ok([client('studio', {
    last_accepted_ingest_at: iso(NOW - 90 * MIN),
    source_delivery: { audio: at(NOW - 119 * MIN) },
    reported: { platform: 'macos', device_type: 'desktop' },
  })]));
  assert.strictEqual(cell(root, 'audio', 'studio'), '1 hour ago');
  assert.strictEqual(deviceBox(root, 'studio').querySelector('.sources-device-time').textContent, '1 hour ago');
  assert.strictEqual(deviceBox(root, 'studio').textContent.includes('desktop'), false);
  assert.strictEqual(deviceBox(root, 'studio').querySelector('.sources-kind').textContent, 'mac');
});

test('an equal device time is not repeated', () => {
  const root = render(ok([client('studio', {
    last_accepted_ingest_at: iso(NOW - 5 * MIN),
    source_delivery: { audio: at(NOW - 5 * MIN) },
  })]));
  assert.strictEqual(cell(root, 'audio', 'studio'), '5 min ago');
  assert.strictEqual(deviceBox(root, 'studio').querySelector('.sources-device-time'), null);
  assert.strictEqual(count(deviceBox(root, 'studio'), '5 min ago'), 0);
});

test('a source time without a device time is not nothing added yet', () => {
  const root = render(ok([client('studio', {
    last_accepted_ingest_at: null,
    source_delivery: { audio: at(NOW - 5 * MIN) },
  })]));
  assert.strictEqual(cell(root, 'audio', 'studio'), '5 min ago');
  assert.strictEqual(count(root, 'nothing added yet'), 0);
});

test('a device that has added nothing says so once', () => {
  const root = render(ok([client('studio', {
    last_accepted_ingest_at: null,
    capture_state: 'no_capture',
    unassessed_reason: null,
    source_delivery: null,
  })]));
  assert.strictEqual(count(root, 'nothing added yet'), 1);
  assert.strictEqual(root.querySelector('table'), null);
});

test('a cell does not carry a rejection', () => {
  const root = render(ok([client('studio', {
    last_accepted_ingest_at: iso(NOW - 5 * MIN),
    source_delivery: {
      audio: at(NOW - 5 * MIN, {
        state: 'needs_attention',
        ingest_rejection: { reason_code: 'event_append_failed' },
      }),
    },
  })]));
  assert.strictEqual(cell(root, 'audio', 'studio'), '5 min ago');
  assert.strictEqual(root.textContent.includes('event_append_failed'), false);
  assert.strictEqual(root.textContent.includes('needs_attention'), false);
});

test('kind words come from platform and device type', () => {
  const cases = [
    [{ platform: 'ipados', device_type: 'tablet' }, 'ipad'],
    [{ platform: 'macos', device_type: 'desktop' }, 'mac'],
    [{ platform: 'linux', device_type: 'desktop' }, 'computer'],
    [{ platform: 'windows', device_type: null }, 'computer'],
    [{ platform: 'ios', device_type: 'phone' }, 'iphone'],
    [{ platform: 'android', device_type: 'phone' }, 'phone'],
    [{ platform: 'android', device_type: 'tablet' }, 'tablet'],
    [{ platform: 'android', device_type: null }, null],
    [{ platform: 'windows', device_type: 'phone' }, 'phone'],
    [{ platform: 'linux', device_type: 'tablet' }, 'tablet'],
    [{ platform: 'watchos', device_type: 'watch' }, 'watch'],
    [{ platform: 'ios', device_type: 'tablet' }, 'ipad'],
  ];
  cases.forEach(([reported, word]) => {
    const row = client('studio', { reported });
    assert.strictEqual(sources.kindWord(row), word, JSON.stringify(reported));
    const root = render(ok([row]));
    const kind = deviceBox(root, 'studio').querySelector('.sources-kind');
    if (word) assert.strictEqual(kind.textContent, word);
    else assert.strictEqual(kind, null);
    assert.strictEqual(root.textContent.includes('cert'), false);
  });
});

test('the heading uses the first non-empty label', () => {
  const root = render(ok([client('', {
    display_label: '   ',
    owner_label: '  kitchen  ',
    device_label: 'ignored',
    last_accepted_ingest_at: null,
  })]));
  assert.strictEqual(deviceBox(root, 'kitchen').querySelector('.sources-name').textContent, 'kitchen');
  assert.strictEqual(root.textContent.includes('ignored'), false);
});

test('wide layout keeps client order and cards have no table', () => {
  const clients = [
    client('zeta', { last_accepted_ingest_at: iso(NOW - 5 * MIN), source_delivery: { audio: at(NOW - 5 * MIN) } }),
    client('alpha', { last_accepted_ingest_at: iso(NOW - 8 * MIN), source_delivery: { screen: at(NOW - 8 * MIN) } }),
  ];
  const wide = render(ok(clients), 'wide');
  const headers = wide.querySelectorAll('th').filter((node) => node.className.includes('sources-device'));
  assert.deepStrictEqual(headers.map((node) => node.getAttribute('data-device')), ['zeta', 'alpha']);
  assert.strictEqual(cell(wide, 'screen', 'zeta'), '');
  assert.strictEqual(cell(wide, 'audio', 'alpha'), '');
  const cards = render(ok(clients), 'cards');
  assert.strictEqual(cards.querySelector('table'), null);
  assert.strictEqual(cards.querySelectorAll('article').length, 2);
  assert.strictEqual(cell(cards, 'audio', 'zeta'), '5 min ago');
  assert.strictEqual(cell(cards, 'screen', 'alpha'), sources.relative(NOW - 8 * MIN, NOW, ZONE));
});

test('empty, unreadable, and failed reads stay distinct', () => {
  const empty = render({ status: 200, body: { clients: [] } });
  assert.strictEqual(empty.textContent.includes(globalThis.JournalI18n.render('journal.window.sources.copy.empty.no.devices.are.paired.yet')), true);
  const pair = empty.querySelector('.sources-pair');
  assert.strictEqual(pair.textContent, globalThis.JournalI18n.render('journal.window.sources.copy.pair.pair.a.device'));
  assert.strictEqual(pair.getAttribute('href'), '/app/network/');
  assert.strictEqual(empty.textContent.includes(globalThis.JournalI18n.render('journal.window.sources.copy.error.this.didn.t.load')), false);

  [
    { status: 503, body: { reason_code: 'authorization_ledger_unreadable' } },
    { status: 200, body: { clients: null } },
    { status: 200, body: {} },
    { status: 200, body: { clients: [client('studio', { capture_state: 'unknown', unassessed_reason: 'activity_unreadable', last_accepted_ingest_at: null })] } },
    { status: 200, body: { clients: [client('studio', { capture_state: 'unknown', unassessed_reason: 'activity_malformed', last_accepted_ingest_at: null })] } },
  ].forEach((payload) => {
    const root = render(payload);
    assert.strictEqual(root.textContent.includes(globalThis.JournalI18n.render('journal.window.sources.copy.error.this.didn.t.load')), true, JSON.stringify(payload.body));
    assert.strictEqual(root.textContent.includes(globalThis.JournalI18n.render('journal.window.sources.copy.empty.no.devices.are.paired.yet')), false);
    assert.strictEqual(count(root, globalThis.JournalI18n.render('journal.window.sources.copy.nothing.nothing.added.yet')), 0);
  });
});

test('try again replaces the error with the device it gets back', async () => {
  const { doc, root } = documentFor();
  const calls = [];
  let clientAttempts = 0;
  const opts = {
    now: NOW,
    timeZone: ZONE,
    layout: 'wide',
    fetch: async (url, init) => {
      calls.push({ url, method: init && init.method });
      if (url === '/app/settings/api/plaud' || url === '/app/settings/api/granola') {
        return { status: 200, json: async () => ({ connected: false, syncing: false }) };
      }
      if (url === '/app/settings/api/garmin') {
        return { status: 200, json: async () => ({ available: true, connected: false, syncing: false }) };
      }
      if (url === '/app/sources/api/email') {
        return { status: 200, json: async () => ({ source: 'email', state: 'ready_to_set_up', phase: 'not_connected', connected: false, syncing: false }) };
      }
      if (url === '/app/sources/photos/api/state') {
        return { status: 404, json: async () => ({ code: 'photos_library_disabled' }) };
      }
      if (url === '/app/sources/api/folders') {
        return { status: 200, json: async () => ({ folders: [] }) };
      }
      clientAttempts += 1;
      if (clientAttempts === 1) return { status: 503, json: async () => ({ reason_code: 'authorization_ledger_unreadable' }) };
      return { status: 200, json: async () => ok([client('studio', { last_accepted_ingest_at: iso(NOW - 5 * MIN) })]).body };
    },
  };
  await sources.load(doc, root, opts);
  assert.strictEqual(root.textContent.includes(globalThis.JournalI18n.render('journal.window.sources.copy.error.this.didn.t.load')), true);
  await root.querySelector('[data-retry]').click();
  assert.strictEqual(root.textContent.includes(globalThis.JournalI18n.render('journal.window.sources.copy.error.this.didn.t.load')), false);
  assert.strictEqual(deviceBox(root, 'studio').querySelector('.sources-device-time').textContent, '5 min ago');
  assert.deepStrictEqual(calls.map((call) => call.url), [
    '/app/network/api/clients',
    '/app/settings/api/plaud',
    '/app/settings/api/granola',
    '/app/settings/api/garmin',
    '/app/sources/api/email',
    '/app/sources/photos/api/state',
    '/app/sources/api/folders',
    '/app/network/api/clients',
  ]);
  assert.ok(calls.every((call) => call.method === 'GET'));
});

test('a paired device links to network and has no pairing controls', () => {
  const root = render(ok([client('studio', { last_accepted_ingest_at: iso(NOW - 5 * MIN) })]));
  const link = root.querySelector('.sources-manage');
  assert.strictEqual(link.textContent, globalThis.JournalI18n.render('journal.window.sources.copy.manage.manage.devices'));
  assert.strictEqual(link.getAttribute('href'), '/app/network/');
  assert.deepStrictEqual(controls(root), []);
  ['rename', 'forget', 'unpair', globalThis.JournalI18n.render('journal.window.sources.copy.pair.pair.a.device')].forEach((word) => {
    assert.strictEqual(root.textContent.includes(word), false, word);
  });
});

test('framed switcher links sources and imports across empty, paired, and error states', () => {
  const sourcesLabel = globalThis.JournalI18n.render('journal.window.sources.copy.title.sources');
  const importsLabel = globalThis.JournalI18n.render('journal.window.sources.switcher.tab.imports');
  const lede = globalThis.JournalI18n.render('journal.window.sources.switcher.lede');
  [ok([]), ok([client('studio', { last_accepted_ingest_at: iso(NOW - 5 * MIN) })]), { status: 503, body: {} }].forEach((payload) => {
    ['wide', 'cards'].forEach((layout) => {
      const root = render(payload, layout);
      const current = root.querySelectorAll('.sources-current');
      const imports = root.querySelectorAll('.sources-import');
      assert.strictEqual(current.length, 1, layout);
      assert.strictEqual(current[0].tagName, 'A');
      assert.strictEqual(current[0].getAttribute('href'), '/app/sources/');
      assert.strictEqual(current[0].getAttribute('aria-current'), 'page');
      assert.strictEqual(current[0].textContent, sourcesLabel);
      assert.strictEqual(imports.length, 1, layout);
      assert.strictEqual(imports[0].tagName, 'A');
      assert.strictEqual(imports[0].getAttribute('href'), '/app/import/');
      assert.strictEqual(imports[0].getAttribute('aria-current'), null);
      assert.strictEqual(imports[0].textContent, importsLabel);
      assert.strictEqual(root.querySelector('.intake-switcher-lede').textContent, lede);
    });
  });
  assert.strictEqual(render(ok([])).querySelector('.sources-manage'), null);
  assert.strictEqual(render({ status: 503, body: {} }).querySelector('.sources-manage'), null);
  assert.notStrictEqual(render(ok([client('studio', {})])).querySelector('.sources-manage'), null);
});

test('syncs renders unconfigured hint alongside paired, empty, and failed device reads', () => {
  const heading = globalThis.JournalI18n.render('journal.window.sources.syncs.heading');
  const manage = globalThis.JournalI18n.render('journal.window.sources.syncs.manage');
  const emptyHint = globalThis.JournalI18n.render('journal.window.sources.syncs.empty.hint');
  [
    ok([client('studio', { last_accepted_ingest_at: iso(NOW - 5 * MIN) })]),
    ok([]),
    { status: 503, body: {} },
  ].forEach((clientsPayload) => {
    const root = render({
      clients: clientsPayload,
      plaud: { status: 200, body: { connected: false, syncing: false } },
      granola: { status: 200, body: { connected: false, syncing: false } },
    });
    const syncs = root.querySelector('.sources-syncs');
    assert.notStrictEqual(syncs, null);
    assert.strictEqual(syncs.querySelector('.sources-syncs-title').textContent, heading);
    const manageLink = syncs.querySelector('.sources-syncs-manage');
    assert.strictEqual(manageLink.getAttribute('href'), '/app/settings/#sync');
    assert.strictEqual(manageLink.textContent, manage);
    assert.strictEqual(syncs.querySelector('.sources-syncs-empty').textContent, emptyHint);
    const rows = syncs.querySelectorAll('.sources-sync-row');
    assert.strictEqual(rows.length, 1);
    assert.strictEqual(rows[0].getAttribute('data-sync'), 'email');
    assert.strictEqual(rows[0].querySelector('.ss-pill').textContent, 'ready to set up');
  });
});

test('connected Plaud and Granola rows cover last_added, nothing added yet, syncing, paused, and last_sync', () => {
  const root = render({
    clients: ok([client('studio', { last_accepted_ingest_at: iso(NOW - 5 * MIN) })]),
    plaud: {
      status: 200,
      body: {
        connected: true,
        enabled: false,
        syncing: false,
        last_added: iso(NOW - 14 * MIN),
        last_sync: iso(NOW - 2 * MIN),
        attention: null,
        attention_code: null,
      },
    },
    granola: {
      status: 200,
      body: {
        connected: true,
        syncing: true,
        last_added: null,
        last_sync: iso(NOW - 8 * MIN),
        attention: null,
        attention_code: null,
      },
    },
  });
  assert.strictEqual(root.querySelector('.sources-syncs-empty'), null);
  const rows = root.querySelectorAll('.sources-sync-row').filter((r) => r.getAttribute('data-sync') !== 'email');
  assert.strictEqual(rows.length, 2);
  assert.strictEqual(rows[0].getAttribute('data-sync'), 'plaud');
  assert.strictEqual(
    rows[0].querySelector('.sources-sync-status').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.last.added', [['when', '14 min ago']]),
  );
  assert.strictEqual(
    rows[0].querySelector('.sources-sync-secondary').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.state.paused'),
  );
  assert.strictEqual(rows[1].getAttribute('data-sync'), 'granola');
  assert.strictEqual(
    rows[1].querySelector('.sources-sync-status').textContent,
    globalThis.JournalI18n.render('journal.window.sources.copy.nothing.nothing.added.yet'),
  );
  assert.strictEqual(
    rows[1].querySelector('.sources-sync-secondary').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.state.syncing'),
  );

  // When syncing is false and enabled is true: last_sync renders last checked; null omits secondary.
  const checkedAndIdle = render({
    clients: ok([]),
    plaud: {
      status: 200,
      body: { connected: true, enabled: true, syncing: false, last_added: iso(NOW - 30 * MIN), last_sync: iso(NOW - 3 * MIN) },
    },
    granola: {
      status: 200,
      body: { connected: true, syncing: false, last_added: null, last_sync: null },
    },
  });
  const rows2 = checkedAndIdle.querySelectorAll('.sources-sync-row').filter((r) => r.getAttribute('data-sync') !== 'email');
  assert.strictEqual(
    rows2[0].querySelector('.sources-sync-secondary').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.last.checked', [['when', '3 min ago']]),
  );
  assert.strictEqual(rows2[1].querySelector('.sources-sync-secondary'), null);
});

test('sync attention maps Granola attention_code via catalog, falls back on unknown, and renders Plaud server attention directly', () => {
  const mapped = render({
    clients: ok([]),
    plaud: {
      status: 200,
      body: {
        connected: true,
        enabled: true,
        syncing: false,
        last_added: null,
        last_sync: null,
        attention: 'server localized plaud message',
        attention_code: 'auth',
      },
    },
    granola: {
      status: 200,
      body: {
        connected: true,
        syncing: false,
        last_added: null,
        last_sync: null,
        attention: 'raw persisted english error',
        attention_code: 'invalid_key',
      },
    },
  });
  const rows = mapped.querySelectorAll('.sources-sync-row').filter((r) => r.getAttribute('data-sync') !== 'email');
  const plaudAtt = rows[0].querySelector('.sources-sync-attention');
  assert.strictEqual(plaudAtt.getAttribute('href'), '/app/settings/#sync');
  assert.strictEqual(plaudAtt.textContent, 'server localized plaud message');
  const granolaAtt = rows[1].querySelector('.sources-sync-attention');
  assert.strictEqual(granolaAtt.getAttribute('href'), '/app/settings/#sync');
  assert.strictEqual(granolaAtt.textContent, globalThis.JournalI18n.render('journal.window.settings.granola.response.invalidkey'));

  const fallback = render({
    clients: ok([]),
    plaud: { status: 200, body: { connected: false, syncing: false } },
    granola: {
      status: 200,
      body: {
        connected: true,
        syncing: false,
        last_added: null,
        last_sync: null,
        attention: 'legacy granola fault text',
        attention_code: 'unknown',
      },
    },
  });
  const oneRow = fallback.querySelectorAll('.sources-sync-row').filter((r) => r.getAttribute('data-sync') !== 'email');
  assert.strictEqual(oneRow.length, 1);
  assert.strictEqual(oneRow[0].querySelector('.sources-sync-attention').textContent, 'legacy granola fault text');
});

test('sync endpoint failure isolates per tool, never claims unconfigured, and retry refetches clients plus failed syncs only', async () => {
  const { doc, root } = documentFor();
  const calls = [];
  let clientAttempts = 0;
  let granolaAttempts = 0;
  const opts = {
    now: NOW,
    timeZone: ZONE,
    layout: 'wide',
    fetch: async (url, init) => {
      calls.push({ url, method: init && init.method });
      if (url === '/app/network/api/clients') {
        clientAttempts += 1;
        if (clientAttempts === 1) return { status: 503, json: async () => ({}) };
        return { status: 200, json: async () => ({ clients: [client('studio', { last_accepted_ingest_at: iso(NOW - 4 * MIN) })] }) };
      }
      if (url === '/app/settings/api/plaud') {
        return {
          status: 200,
          json: async () => ({ connected: false, enabled: false, syncing: false }),
        };
      }
      if (url === '/app/settings/api/garmin') {
        return {
          status: 200,
          json: async () => ({ available: true, connected: false, syncing: false }),
        };
      }
      if (url === '/app/sources/api/email') {
        return {
          status: 200,
          json: async () => ({ source: 'email', state: 'ready_to_set_up', phase: 'not_connected', connected: false, syncing: false }),
        };
      }
      if (url === '/app/sources/photos/api/state') {
        return {
          status: 404,
          json: async () => ({ code: 'photos_library_disabled' }),
        };
      }
      if (url === '/app/sources/api/folders') {
        return {
          status: 200,
          json: async () => ({ folders: [] }),
        };
      }
      granolaAttempts += 1;
      if (granolaAttempts === 1) return { status: 400, json: async () => ({ error: 'storage', code: 'storage' }) };
      return {
        status: 200,
        json: async () => ({ connected: true, syncing: false, last_added: iso(NOW - 10 * MIN), last_sync: iso(NOW - 1 * MIN), attention: null, attention_code: null }),
      };
    },
  };
  await sources.load(doc, root, opts);
  // Devices failed, Plaud is unconfigured, Granola returned HTTP 400:
  // must NOT show the unconfigured hint, and must show Granola's unavailable link.
  assert.notStrictEqual(root.querySelector('[data-retry]'), null);
  assert.strictEqual(root.querySelector('.sources-syncs-empty'), null);
  const unavailableRows = root.querySelectorAll('.sources-sync-unavailable');
  assert.strictEqual(unavailableRows.length, 1);
  assert.strictEqual(unavailableRows[0].getAttribute('data-sync'), 'granola');
  const unavailableLink = unavailableRows[0].querySelector('.sources-sync-link');
  assert.strictEqual(unavailableLink.getAttribute('href'), '/app/settings/#sync');
  assert.strictEqual(
    unavailableLink.querySelector('.sources-sync-status').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.unavailable'),
  );

  // Clicking retry refetches clients + failed Granola, skipping healthy Plaud/Garmin/Email.
  await root.querySelector('[data-retry]').click();
  assert.strictEqual(root.querySelector('[data-retry]'), null);
  assert.strictEqual(deviceBox(root, 'studio').querySelector('.sources-device-time').textContent, '4 min ago');
  assert.strictEqual(root.querySelectorAll('.sources-sync-unavailable').length, 0);
  const connectedRow = root.querySelector('[data-sync="granola"]');
  assert.strictEqual(connectedRow.getAttribute('data-sync'), 'granola');
  assert.strictEqual(
    connectedRow.querySelector('.sources-sync-status').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.last.added', [['when', '10 min ago']]),
  );
  assert.deepStrictEqual(calls.map((c) => c.url), [
    '/app/network/api/clients',
    '/app/settings/api/plaud',
    '/app/settings/api/granola',
    '/app/settings/api/garmin',
    '/app/sources/api/email',
    '/app/sources/photos/api/state',
    '/app/sources/api/folders',
    '/app/network/api/clients',
    '/app/settings/api/granola',
  ]);
});

test('Garmin sync row renders on desktop, isolates failures, shows attention/expiring links, and hides cleanly when unavailable on mobile', () => {
  // Desktop unconfigured mentions Garmin alongside Plaud and Granola.
  const desktopEmpty = render({
    clients: ok([]),
    plaud: { status: 200, body: { connected: false, syncing: false } },
    granola: { status: 200, body: { connected: false, syncing: false } },
    garmin: { status: 200, body: { available: true, connected: false, syncing: false } },
  });
  assert.strictEqual(
    desktopEmpty.querySelector('.sources-syncs-empty').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.empty.hint.with.garmin'),
  );

  // Embedded mobile (available: false) omits Garmin cleanly and keeps the mobile hint.
  const mobileHidden = render({
    clients: ok([]),
    plaud: { status: 200, body: { connected: false, syncing: false } },
    granola: { status: 200, body: { connected: false, syncing: false } },
    garmin: { status: 200, body: { available: false, connected: false, syncing: false } },
  });
  assert.strictEqual(
    mobileHidden.querySelector('.sources-syncs-empty').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.empty.hint'),
  );
  assert.strictEqual(mobileHidden.querySelector('[data-sync="garmin"]'), null);

  // Connected Garmin row with last_added, last_sync, and key_expiring_soon fallback.
  const connected = render({
    clients: ok([]),
    plaud: { status: 200, body: { connected: false, syncing: false } },
    granola: { status: 200, body: { connected: false, syncing: false } },
    garmin: {
      status: 200,
      body: {
        available: true,
        connected: true,
        syncing: false,
        last_added: iso(NOW - 12 * MIN),
        last_sync: iso(NOW - 2 * MIN),
        attention: null,
        attention_code: null,
        key_expiring_soon: true,
      },
    },
  });
  const row = connected.querySelector('[data-sync="garmin"]');
  assert.notStrictEqual(row, null);
  assert.strictEqual(
    row.querySelector('.sources-sync-name').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.provider.garmin'),
  );
  assert.strictEqual(
    row.querySelector('.sources-sync-status').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.last.added', [['when', '12 min ago']]),
  );
  assert.strictEqual(
    row.querySelector('.sources-sync-secondary').textContent,
    globalThis.JournalI18n.render('journal.window.sources.syncs.last.checked', [['when', '2 min ago']]),
  );
  const expLink = row.querySelector('.sources-sync-attention-garmin');
  assert.notStrictEqual(expLink, null);
  assert.strictEqual(expLink.getAttribute('href'), '/app/settings/#sync');
  assert.strictEqual(expLink.textContent, globalThis.JournalI18n.render('journal.window.sources.syncs.garmin.expiring'));

  // Server attention overrides expiring fallback; endpoint error isolates to Garmin row only.
  const failedGarmin = render({
    clients: ok([]),
    plaud: { status: 200, body: { connected: true, enabled: true, syncing: false, last_added: iso(NOW - 5 * MIN), last_sync: iso(NOW - 1 * MIN) } },
    granola: { status: 200, body: { connected: false, syncing: false } },
    garmin: { status: 503, body: null },
  });
  const rows = failedGarmin.querySelectorAll('.sources-sync-row').filter((r) => r.getAttribute('data-sync') !== 'email');
  assert.strictEqual(rows.length, 2);
  assert.strictEqual(rows[0].getAttribute('data-sync'), 'plaud');
  assert.strictEqual(rows[1].getAttribute('data-sync'), 'garmin');
  assert.notStrictEqual(rows[1].querySelector('.sources-sync-unavailable-garmin'), null);
});

test('email selective-sync row covers all 8 states, inline left-out counts-only fold, and isolated API failure', () => {
  // 1. ready_to_set_up
  const ready = render({
    clients: ok([]),
    email: { status: 200, body: { source: 'email', state: 'ready_to_set_up', phase: 'not_connected', connected: false, syncing: false } },
  });
  const readyRow = ready.querySelector('[data-sync="email"]');
  assert.strictEqual(readyRow.querySelector('.ss-pill').textContent, 'ready to set up');
  assert.ok(readyRow.textContent.includes('connect a mailbox to bring in messages you sent.'));
  assert.strictEqual(readyRow.querySelector('.sources-sync-action-email').getAttribute('href'), '/app/sources/email');

  // 2. scanning
  const scanning = render({
    clients: ok([]),
    email: {
      status: 200,
      body: {
        source: 'email', state: 'setting_up', phase: 'scanning', account: 'maya.chen@gmail.com',
        connected: true, syncing: true,
        scan: { complete: false, paused: false, checked: 142800, total: 287063, bodies_read: 0 },
      },
    },
  });
  const scanRow = scanning.querySelector('[data-sync="email"]');
  assert.strictEqual(scanRow.querySelector('.ss-pill').textContent, 'setting up');
  assert.ok(scanRow.textContent.includes('142,800 of 287,063 messages. no message bodies read yet.'));
  assert.strictEqual(scanRow.querySelector('.sources-sync-action-email').textContent, 'open email');

  // 3. awaiting_choice
  const awaiting = render({
    clients: ok([]),
    email: {
      status: 200,
      body: {
        source: 'email', state: 'setting_up', phase: 'awaiting_choice', account: 'maya.chen@gmail.com',
        connected: true, syncing: false, scan: { complete: true, eligible_sent: 16410 },
      },
    },
  });
  const awaitRow = awaiting.querySelector('[data-sync="email"]');
  assert.ok(awaitRow.textContent.includes('scan done. 16,410 sent messages ready to choose.'));
  assert.strictEqual(awaitRow.querySelector('.sources-sync-action-email').textContent, 'choose what comes in');

  // 4. fetching (with status link)
  const fetching = render({
    clients: ok([]),
    email: {
      status: 200,
      body: {
        source: 'email', state: 'on', phase: 'fetching', account: 'maya.chen@gmail.com',
        connected: true, syncing: true, sync: { active: true, done: 612, total: 1740 },
      },
    },
  });
  const fetchRow = fetching.querySelector('[data-sync="email"]');
  assert.strictEqual(fetchRow.querySelector('.ss-pill').textContent, 'on');
  assert.ok(fetchRow.textContent.includes('bringing in 612 of 1,740 messages'));
  assert.strictEqual(fetchRow.querySelector('.ss-inline-link').getAttribute('href'), '/app/health/');

  // 5. steady on with inline left-out counts-only fold
  const steady = render({
    clients: ok([]),
    email: {
      status: 200,
      body: {
        source: 'email', state: 'on', phase: 'steady', account: 'maya.chen@gmail.com',
        connected: true, syncing: false,
        sync: { active: false, done: 1740, total: 1740, last_added_at: iso(NOW - 14 * MIN), last_checked_at: iso(NOW - 6 * MIN) },
        accounting: {
          sent_total: 16443, in_journal: 1740, removed_by_owner: 0, outside_choice: 14670,
          left_out: { total: 33, calendar_responses: 24, automatic_replies: 5, drafts: 3, delivery_reports: 1 },
        },
      },
    },
  });
  const steadyRow = steady.querySelector('[data-sync="email"]');
  assert.ok(steadyRow.textContent.includes('1,740 in your journal · last added 14 min ago · last checked 6 min ago'));
  const fold = steadyRow.querySelector('details');
  assert.notStrictEqual(fold, null);
  assert.strictEqual(fold.querySelector('summary').textContent, '33 left out');
  assert.ok(fold.textContent.includes('calendar responses'));
  assert.ok(fold.textContent.includes('no message content is kept for mail left out.'));
  assert.strictEqual(steadyRow.querySelector('.sources-sync-action-email').textContent, 'manage email');

  // 6 & 7. needs_attention split: password_rejected vs all_mail_hidden
  const pwAttn = render({
    clients: ok([]),
    email: {
      status: 200,
      body: { source: 'email', state: 'needs_attention', phase: 'needs_attention', account: 'maya.chen@gmail.com', attention_reason: 'password_rejected', connected: true, syncing: false },
    },
  });
  assert.ok(pwAttn.querySelector('[data-sync="email"]').textContent.includes('Gmail stopped accepting this app password. nothing new is coming in.'));
  assert.strictEqual(pwAttn.querySelector('.sources-sync-action-email').textContent, 'fix in email');

  const imapAttn = render({
    clients: ok([]),
    email: {
      status: 200,
      body: { source: 'email', state: 'needs_attention', phase: 'needs_attention', account: 'maya.chen@gmail.com', attention_reason: 'all_mail_hidden', connected: true, syncing: false },
    },
  });
  assert.ok(imapAttn.querySelector('[data-sync="email"]').textContent.includes("Gmail isn't letting your journal read All Mail over IMAP. nothing new is coming in."));

  // 8. disconnected (off) & isolated API failure
  const off = render({
    clients: ok([]),
    email: {
      status: 200,
      body: { source: 'email', state: 'off', phase: 'disconnected', account: 'maya.chen@gmail.com', connected: false, syncing: false, accounting: { in_journal: 1740 } },
    },
  });
  const offRow = off.querySelector('[data-sync="email"]');
  assert.strictEqual(offRow.querySelector('.ss-pill').textContent, 'off');
  assert.ok(offRow.textContent.includes('disconnected. 1,740 messages already in your journal stay.'));
  assert.strictEqual(offRow.querySelector('.sources-sync-action-email').textContent, 'reconnect email');

  const broken = render({
    clients: ok([]),
    email: { status: 503, body: null },
  });
  assert.notStrictEqual(broken.querySelector('.sources-sync-unavailable-email'), null);
});

test('photos sync row stays hidden when disabled (404) and renders every enabled state on the shared frame', () => {
  // Default OFF (404): zero photos row in the DOM.
  const hidden = render({
    clients: ok([]),
    photos: { status: 404, body: { code: 'photos_library_disabled' } },
  });
  assert.strictEqual(hidden.querySelector('[data-sync="photos"]'), null);

  // Ready to set up
  const ready = render({
    clients: ok([]),
    photos: { status: 200, body: { source: 'photos', state: 'ready_to_set_up', substate: 'never_set_up', devices: [], scan: {}, ranges: {}, left_out: { total: 0, reasons: {} }, progress: {} } },
  });
  const readyRow = ready.querySelector('[data-sync="photos"]');
  assert.notStrictEqual(readyRow, null);
  assert.strictEqual(readyRow.querySelector('.ss-pill').getAttribute('data-state'), 'ready_to_set_up');
  assert.strictEqual(readyRow.querySelector('.sources-sync-action-photos').getAttribute('href'), '/app/sources/photos');

  // Choose state (accent action)
  const choose = render({
    clients: ok([]),
    photos: { status: 200, body: { source: 'photos', state: 'setting_up', substate: 'choose', devices: [{ name: 'Test Phone' }], scan: { items_checked: 200, items_total: 200 }, ranges: { '1y': { photos: 40 } }, left_out: { total: 0, reasons: {} }, progress: {} } },
  });
  assert.ok(choose.querySelector('.sources-sync-action-photos').className.includes('accent'));

  // Bringing in (status link)
  const bringing = render({
    clients: ok([]),
    photos: { status: 200, body: { source: 'photos', state: 'on', substate: 'bringing_in', devices: [{ name: 'Test Phone' }], scan: {}, ranges: {}, left_out: { total: 0, reasons: {} }, progress: { chosen_total: 40, arrived: 12, in_journal: 12, waiting_on_phone: 28 } } },
  });
  const bringingRow = bringing.querySelector('[data-sync="photos"]');
  assert.strictEqual(bringingRow.querySelector('.ss-inline-link').getAttribute('href'), '/app/health/');

  // Steady with left-out fold
  const steady = render({
    clients: ok([]),
    photos: { status: 200, body: { source: 'photos', state: 'on', substate: 'steady', devices: [{ name: 'Test Phone', last_upload_at: iso(NOW - 10 * MIN), last_scan_at: iso(NOW - 5 * MIN) }], scan: {}, ranges: {}, left_out: { total: 15, reasons: { near_duplicates: 10, videos: 5 } }, progress: { in_journal: 38 } } },
  });
  assert.notStrictEqual(steady.querySelector('[data-sync="photos"]').querySelector('details'), null);

  // Needs attention & off & isolated failure
  const attn = render({
    clients: ok([]),
    photos: { status: 200, body: { source: 'photos', state: 'needs_attention', substate: 'access_limited', attention_reason: 'access_limited', devices: [{ name: 'Test Phone' }], scan: {}, ranges: {}, left_out: { total: 0, reasons: {} }, progress: {} } },
  });
  assert.strictEqual(attn.querySelector('[data-sync="photos"]').querySelector('.ss-pill').getAttribute('data-state'), 'needs_attention');

  const off = render({
    clients: ok([]),
    photos: { status: 200, body: { source: 'photos', state: 'off', substate: 'off', devices: [], scan: {}, ranges: {}, left_out: { total: 0, reasons: {} }, progress: { in_journal: 38 } } },
  });
  assert.strictEqual(off.querySelector('[data-sync="photos"]').querySelector('.ss-pill').getAttribute('data-state'), 'off');

  const broken = render({
    clients: ok([]),
    photos: { status: 503, body: null },
  });
  assert.notStrictEqual(broken.querySelector('.sources-sync-unavailable-photos'), null);

  // Case (b): old iOS app paired (in clients list) + journal returns 404 -> no photos row.
  const oldIphoneClient = {
    display_label: 'Old Phone',
    reported: { platform: 'ios', device_type: 'phone' },
    sources: [],
  };
  const hiddenWithOldIos = render({
    clients: ok([oldIphoneClient]),
    photos: { status: 404, body: { code: 'photos_library_disabled' } },
  });
  assert.strictEqual(hiddenWithOldIos.querySelector('[data-sync="photos"]'), null);

  // Capable iPad reported in while an old iPhone is also paired -> names ipad, not old iphone.
  const capableIpad = render({
    clients: ok([oldIphoneClient]),
    photos: {
      status: 200,
      body: {
        source: 'photos',
        state: 'ready_to_set_up',
        substate: 'never_set_up',
        capable_device_kinds: ['ipad'],
        devices: [],
        scan: {},
        ranges: {},
        left_out: { total: 0, reasons: {} },
        progress: {},
      },
    },
  });
  const ipadStatus = capableIpad.querySelector('[data-sync="photos"]').textContent;
  assert.ok(ipadStatus.includes('ipad'), 'expected ready row to name capable ipad, got: ' + ipadStatus);
  assert.ok(!ipadStatus.includes('iphone'), 'expected ready row not to name old iphone, got: ' + ipadStatus);
});

test('folders band hides when empty or 404 and renders active, backfill, and attention rows', () => {
  const empty = render({
    clients: ok([client('work laptop', { source_delivery: { folder: { last_accepted_ingest_at: iso(NOW - 3 * MIN) } } })]),
    folders: { status: 200, body: { folders: [] } },
  });
  assert.strictEqual(empty.querySelector('.sources-folders'), null);

  const missing = render({
    clients: ok([]),
    folders: { status: 404, body: null },
  });
  assert.strictEqual(missing.querySelector('.sources-folders'), null);

  const populated = render({
    clients: ok([client('work laptop', { source_delivery: { folder: { last_accepted_ingest_at: iso(NOW - 3 * MIN) } } })]),
    folders: {
      status: 200,
      body: {
        folders: [
          {
            id: 'dl-1',
            name: 'Downloads',
            device_id: 'dev-1',
            device_label: 'work laptop',
            state: 'on',
            facet: { name: 'work', title: 'Work', color: '#2E6F9E', icon: null, emoji: null },
            kinds: ['notes', 'documents', 'other'],
            items_count: 14,
            segments_count: 3,
            last_added_at: iso(NOW - 3 * MIN),
            backfill_active: true,
          },
          {
            id: 'docs-2',
            name: 'Docs',
            device_id: 'dev-1',
            device_label: 'work laptop',
            state: 'needs_attention',
            facet: null,
            kinds: ['documents'],
            items_count: 1,
            segments_count: 1,
            last_added_at: null,
            backfill_active: false,
          },
        ],
      },
    },
  });
  const band = populated.querySelector('.sources-folders');
  assert.notStrictEqual(band, null);
  const rows = band.querySelectorAll('.sources-folder-row');
  assert.strictEqual(rows.length, 2);
  assert.strictEqual(rows[0].querySelector('.sources-folder-name').textContent, 'Downloads');
  assert.notStrictEqual(rows[0].querySelector('.fchip'), null);
  assert.strictEqual(rows[0].querySelectorAll('.sources-folder-kind').length, 3);
  const statusLink = rows[0].querySelector('.sources-folder-status');
  assert.notStrictEqual(statusLink, null);
  assert.strictEqual(statusLink.getAttribute('href'), '/app/health/');
  assert.strictEqual(rows[0].querySelector('.sources-folder-attn'), null);

  assert.ok(rows[1].className.includes('is-attention'));
  assert.strictEqual(rows[1].querySelector('.sources-folder-status'), null);
  assert.notStrictEqual(rows[1].querySelector('.sources-folder-attn'), null);
});

(async () => {
  let passed = 0;
  for (let i = 0; i < tests.length; i += 1) {
    await tests[i][1]();
    passed += 1;
  }
  passed += await require(path.join(__dirname, 'sources_contract_dom.js'))(path.join(__dirname, '..'));
  passed += await require(path.join(__dirname, 'email_source_dom.js'))(path.join(__dirname, '..'));
  passed += await require(path.join(__dirname, 'photos_source_dom.js'))(path.join(__dirname, '..'));
  console.log('DOM CASES: ' + passed + ' passed');
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
