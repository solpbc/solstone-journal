// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const path = require('path');
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
  assert.strictEqual(empty.textContent.includes('no devices are paired yet.'), true);
  const pair = empty.querySelector('a');
  assert.strictEqual(pair.textContent, 'pair a device');
  assert.strictEqual(pair.getAttribute('href'), '/app/network/');
  assert.strictEqual(empty.textContent.includes("this didn't load."), false);

  [
    { status: 503, body: { reason_code: 'authorization_ledger_unreadable' } },
    { status: 200, body: { clients: null } },
    { status: 200, body: {} },
    { status: 200, body: { clients: [client('studio', { capture_state: 'unknown', unassessed_reason: 'activity_unreadable', last_accepted_ingest_at: null })] } },
    { status: 200, body: { clients: [client('studio', { capture_state: 'unknown', unassessed_reason: 'activity_malformed', last_accepted_ingest_at: null })] } },
  ].forEach((payload) => {
    const root = render(payload);
    assert.strictEqual(root.textContent.includes("this didn't load."), true, JSON.stringify(payload.body));
    assert.strictEqual(root.textContent.includes('no devices are paired yet.'), false);
    assert.strictEqual(count(root, 'nothing added yet'), 0);
  });
});

test('try again replaces the error with the device it gets back', async () => {
  const { doc, root } = documentFor();
  const calls = [];
  let attempt = 0;
  const opts = {
    now: NOW,
    timeZone: ZONE,
    layout: 'wide',
    fetch: async (url, init) => {
      calls.push({ url, method: init && init.method });
      attempt += 1;
      if (attempt === 1) return { status: 503, json: async () => ({ reason_code: 'authorization_ledger_unreadable' }) };
      return { status: 200, json: async () => ok([client('studio', { last_accepted_ingest_at: iso(NOW - 5 * MIN) })]).body };
    },
  };
  await sources.load(doc, root, opts);
  assert.strictEqual(root.textContent.includes("this didn't load."), true);
  await root.querySelector('[data-retry]').click();
  assert.strictEqual(root.textContent.includes("this didn't load."), false);
  assert.strictEqual(deviceBox(root, 'studio').querySelector('.sources-device-time').textContent, '5 min ago');
  assert.deepStrictEqual(calls.map((call) => call.url), ['/app/network/api/clients', '/app/network/api/clients']);
  assert.deepStrictEqual(calls.map((call) => call.method), ['GET', 'GET']);
});

test('a paired device links to network and has no pairing controls', () => {
  const root = render(ok([client('studio', { last_accepted_ingest_at: iso(NOW - 5 * MIN) })]));
  const link = root.querySelector('a');
  assert.strictEqual(link.textContent, 'manage devices');
  assert.strictEqual(link.getAttribute('href'), '/app/network/');
  assert.deepStrictEqual(controls(root), []);
  ['rename', 'forget', 'unpair', 'pair a device'].forEach((word) => {
    assert.strictEqual(root.textContent.includes(word), false, word);
  });
});

test('phone width boots as one card per device', async () => {
  const { doc, root } = documentFor();
  const queries = [];
  const view = {
    document: doc,
    matchMedia: (query) => {
      queries.push(query);
      return { matches: true, addEventListener() {} };
    },
    fetch: async () => ({
      status: 200,
      json: async () => ok([
        client('studio', { last_accepted_ingest_at: iso(NOW - 5 * MIN), source_delivery: { audio: at(NOW - 5 * MIN) } }),
        client('phone', { last_accepted_ingest_at: iso(NOW - 8 * MIN), source_delivery: { screen: at(NOW - 8 * MIN) } }),
      ]).body,
    }),
  };
  await sources.boot(view);
  assert.deepStrictEqual(queries, ['(max-width: 700px)']);
  assert.strictEqual(root.querySelector('table'), null);
  assert.strictEqual(root.querySelectorAll('article').length, 2);
});

(async () => {
  let passed = 0;
  for (let i = 0; i < tests.length; i += 1) {
    await tests[i][1]();
    passed += 1;
  }
  console.log('DOM CASES: ' + passed + ' passed');
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
