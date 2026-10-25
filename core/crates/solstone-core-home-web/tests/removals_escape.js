// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

class Control {
  constructor(action, markId) {
    this.dataset = { removalAction: action, markId: markId || '' };
    this.listeners = {};
  }

  addEventListener(name, listener) {
    this.listeners[name] = listener;
  }

  click() {
    this.listeners.click();
  }
}

class Select {
  constructor(markId) {
    this.dataset = { markId: markId || '' };
    this.listeners = {};
  }

  addEventListener(name, listener) {
    this.listeners[name] = listener;
  }

  click() {
    this.listeners.click();
  }
}

class Element {
  constructor() {
    this.children = [];
    this.controls = [];
    this.selects = [];
    this._innerHTML = '';
  }

  appendChild(child) {
    this.children.push(child);
    return child;
  }

  get innerHTML() {
    return this._innerHTML;
  }

  set innerHTML(value) {
    this._innerHTML = String(value);
    this.controls = Array.from(this._innerHTML.matchAll(
      /<button[^>]*data-removal-action="([^"]+)"(?:[^>]*data-mark-id="([^"]*)")?[^>]*>/g
    )).map((match) => new Control(match[1], match[2]));
    this.selects = Array.from(this._innerHTML.matchAll(
      /<input[^>]*data-removal-select[^>]*>/g
    )).map((match) => new Select((match[0].match(/data-mark-id="([^"]*)"/) || [])[1]));
  }

  querySelectorAll(selector) {
    if (selector === '[data-removal-action]') return this.controls;
    if (selector === '[data-removal-select]') return this.selects;
    return [];
  }

  querySelector() { return null; }

  setAttribute() {}
}

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

async function settle() {
  await flush();
  await flush();
}

function copyTable(source) {
  const [entries] = source
    .split('const COPY = Object.freeze({\n')[1]
    .split('  });\n\n  const LIST_URL');
  return Object.fromEntries(entries.trim().split('\n').map((line) => {
    const [, key, value] = line.match(/^\s*"([^"]+)": "([^"]+)",?$/);
    return [key, value];
  }));
}

// The stream label comes from the one shared formatter, not a second copy of
// its rule here: this restated the old strip-the-punctuation transform and went
// stale the moment the formatter learned to parse the key (G1-203 / S-1).
const FORMAT = (() => {
  const host = {};
  vm.runInNewContext(
    fs.readFileSync(path.join(__dirname, '../../solstone-core-convey-shell/assets/static/date_format.js'), 'utf8'),
    { window: host },
  );
  return host.JournalFormat;
})();

function rendered(copy, key, values) {
  values = { ...values, ...(values.date ? { date: 'Thu Jan 1' } : {}), ...(values.stream ? { stream: FORMAT.stream(values.stream) } : {}) };
  return copy[key].replace(/\{([^}]+)\}/g, (_, name) => String(values[name] ?? ''));
}

function defaultStreamRendered(copy, key, values) {
  values = { ...values, ...(values.date ? { date: 'Thu Jan 1' } : {}) };
  return copy[key].replace(' · {stream}', '').replace(/\{([^}]+)\}/g, (_, name) => String(values[name] ?? ''));
}

function escaped(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;'
  })[char]);
}

async function boot(source, lists, responses, labels = null) {
  let root = new Element();
  const timers = new Map();
  let nextTimer = 0;
  const listResponses = Array.isArray(lists) ? lists : [lists];
  let listIndex = 0;
  const calls = [];
  const window = {
    setTimeout(callback) { const id = ++nextTimer; timers.set(id, callback); return id; },
    clearTimeout(id) { timers.delete(id); },
    apiJson(url, options) {
      calls.push({ url, options: options || null });
      if (url.startsWith('/app/home/api/removals?')) {
        let response = listResponses[Math.min(listIndex, listResponses.length - 1)];
        if (typeof response === 'function') response = response();
        listIndex += 1;
        return Promise.resolve(response).then(response => {
          if (response && !Object.prototype.hasOwnProperty.call(response, 'review_context')
              && (response.state === 'list.ready' || response.state === 'list.empty')) {
            response = { ...response, review_context: JSON.stringify({ root_observation: 'fixture', marks: response.removals }) };
          }
          return response && { offset: 0, next_offset: null, total_rows: response.removals?.length || 0, ...response };
        });
      }
      if (url === '/app/home/api/approve') return Promise.resolve(typeof responses.approve === 'function' ? responses.approve() : responses.approve);
      if (url.startsWith('/app/home/api/approve/')) return Promise.resolve(responses.receive(url, options));
      if (url === '/app/home/api/decline') return Promise.resolve(responses.decline);
      if (url === '/app/home/api/recover') return Promise.resolve(responses.recover);
      throw new Error(`unexpected URL: ${url}`);
    }
  };
  const listeners = {};
  const document = {
    readyState: 'complete',
    addEventListener(type, listener) { (listeners[type] = listeners[type] || []).push(listener); },
    removeEventListener(type, listener) { listeners[type] = (listeners[type] || []).filter(item => item !== listener); },
    createElement() { return new Element(); },
    querySelector(selector) { return selector === '[data-home-root]' ? root : null; }
  };
  window.window = window;
  // The originals list mounts on its own page only.
  window.location = { pathname: '/app/home/originals/' };
  class FixtureDate extends Date { constructor(...args) { super(...(args.length ? args : [2026, 8, 5])); } }
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../../solstone-core-convey-shell/assets/static/date_format.js'), 'utf8'), { window, Date: FixtureDate });
  if (labels) window.JournalFormat.installStreamLabels({ status: 'complete', labels });
  const load = () => vm.runInNewContext(source, { document, Promise, setImmediate, window, Set }, { filename: 'removals.js' });
  load();
  // The shell mounts a workspace's scripts, then announces it; the card mounts on that event.
  for (const listener of listeners['workspace:mounted'] || []) listener({ detail: { appName: 'home' } });
  await settle();
  const card = root.children[0];
  assert(card, 'card mounted');
  card.calls = calls;
  card.timerCount = () => timers.size;
  card.tick = async function () {
    const entry = timers.entries().next().value;
    assert(entry, 'one observation timer exists');
    timers.delete(entry[0]); entry[1](); await settle();
  };
  card.replace = async function (appName, replay) {
    root = appName === 'home' ? new Element() : null;
    window.location.pathname = appName === 'home' ? '/app/home/originals/' : '/app/' + appName;
    if (replay) load();
    for (const listener of (listeners['workspace:mounted'] || []).slice()) listener({ detail: { appName } });
    await settle();
    const next = root?.children[0];
    if (next) { next.calls = calls; next.timerCount = card.timerCount; next.tick = card.tick; }
    return next;
  };
  card.refresh = async function () {
    for (const listener of listeners['workspace:mounted'] || []) listener({ detail: { appName: 'home' } });
    await settle();
  };
  return card;
}

function click(card, action, markId) {
  const control = card.controls.find((item) => (
    item.dataset.removalAction === action && (markId === undefined || item.dataset.markId === markId)
  ));
  assert(control, `${action} control present`);
  control.click();
}

function clickSelect(card, markId) {
  const control = card.selects.find((item) => item.dataset.markId === markId);
  assert(control, `select ${markId} present`);
  control.click();
}

function posts(card, url) {
  return card.calls.filter((item) => item.url === url && item.options && item.options.method === 'POST');
}

async function approve(card, markId) {
  click(card, 'approve', markId);
  await settle();
  click(card, 'confirm');
  await settle();
}

async function confirmation(card, markId) {
  click(card, 'approve', markId);
  await settle();
}

function marked(id, origin, count, stream) {
  return {
    id,
    state: 'marked',
    origin,
    day: '20260101',
    stream,
    count,
    bytes: count,
    size: `${count} B`
  };
}

function outcome(card) {
  return card.innerHTML.match(/<section class="removals-card-outcome">([\s\S]*?)<\/section>/)?.[1] || '';
}

function assertDeclineOutcomeHasNoDeletingCopy(card, copy) {
  const deleting = [
    'done.unknown',
    'done.refused_none_one',
    'done.refused_none_many',
    'done.clause_deleted_one',
    'done.clause_deleted_many',
    'done.clause_not_removed_one',
    'done.clause_not_removed_many',
    'done.clause_halted'
  ].map((key) => copy[key]);
  const rendered = outcome(card);
  for (const value of deleting) {
    assert(!rendered.includes(value), `decline rendered deleting copy: ${value}`);
  }
}

async function reviewTransportCases(source) {
  const row = marked('same-id', 'policy', 1, 'camera');
  const first = '{"root_observation":"root-a","marks":[{"id":"same-id","bytes":9007199254740993,"text":"quote\\\" and \\\\ and é"}]}';
  const changed = first.replace('9007199254740993', '9007199254740994');
  const rootChanged = first.replace('root-a', 'root-b');
  const list = context => ({ state: 'list.ready', removals: [row], review_context: context });
  for (const next of [changed, rootChanged, null]) {
    const card = await boot(source, [list(first), list(next)], {});
    clickSelect(card, row.id);
    click(card, 'delete-selected');
    const oldConfirm = card.controls.find(item => item.dataset.removalAction === 'confirm');
    const oldDecline = card.controls.find(item => item.dataset.removalAction === 'decline');
    const oldSelect = card.selects[0];
    assert(oldConfirm && oldDecline && oldSelect);
    await card.refresh();
    assert(!card.innerHTML.includes('removals-card-confirm'), 'changed context clears confirmation');
    assert(!card.innerHTML.includes(' checked'), 'changed context clears selection');
    oldConfirm.click(); oldDecline.click(); oldSelect.click();
    await settle();
    assert.strictEqual(posts(card, '/app/home/api/approve').length, 0);
    assert.strictEqual(posts(card, '/app/home/api/decline').length, 0);
    assert(!card.innerHTML.includes(' checked'), 'detached checkbox cannot revive selection');
  }
  const unchanged = await boot(source, [list(first), list(first)], { approve: { state: 'approve.refused_before_start' } });
  clickSelect(unchanged, row.id);
  click(unchanged, 'delete-selected');
  await unchanged.refresh();
  assert(unchanged.innerHTML.includes(' checked'), 'unchanged context retains selection');
  assert(unchanged.innerHTML.includes('removals-card-confirm'), 'unchanged context retains confirmation');
  click(unchanged, 'confirm'); await settle();
  const approved = JSON.parse(posts(unchanged, '/app/home/api/approve')[0].options.body);
  assert.strictEqual(approved.review_context, first, 'opaque values survive outer JSON round trip');
  assert.deepStrictEqual(approved.mark_ids, [row.id]);
  const decline = await boot(source, list(changed), { decline: { state: 'declined.refused' } });
  click(decline, 'decline', row.id); await settle();
  assert.strictEqual(JSON.parse(posts(decline, '/app/home/api/decline')[0].options.body).review_context, changed);
  let resolveOld;
  const late = new Promise(resolve => { resolveOld = resolve; });
  const overlap = await boot(source, [list(first), () => late, list(changed)], {});
  clickSelect(overlap, row.id);
  const olderRefresh = overlap.refresh();
  await overlap.refresh();
  resolveOld(list(first)); await olderRefresh; await settle();
  assert(!overlap.innerHTML.includes(' checked'), 'older refresh cannot restore selection');
  const oldMissing = await boot(source, { state: 'list.ready', removals: [row], review_context: null }, {});
  assert(!oldMissing.controls.some(item => item.dataset.removalAction === 'approve'), 'missing context has no action fallback');
}

// The first page and the week page carry no originals list: it mounts only on
// its own page, and nowhere else reads it through this script.
async function assertStaysOff(source, pathname) {
  const root = new Element();
  const calls = [];
  const listeners = {};
  const window = {
    location: { pathname },
    apiJson(url) { calls.push(url); return new Promise(() => {}); },
    setTimeout() { return 1; },
    clearTimeout() {},
  };
  window.window = window;
  const document = {
    readyState: 'complete',
    addEventListener(type, listener) { (listeners[type] = listeners[type] || []).push(listener); },
    removeEventListener() {},
    createElement() { return new Element(); },
    querySelector(selector) { return selector === '[data-home-root]' ? root : null; }
  };
  vm.runInNewContext(source, { document, Promise, setImmediate, window, Set }, { filename: 'removals.js' });
  for (const listener of listeners['workspace:mounted'] || []) listener({ detail: { appName: 'home' } });
  await settle();
  assert.strictEqual(root.children.length, 0, `nothing mounts on ${pathname}`);
  assert.deepStrictEqual(calls, [], `nothing is read on ${pathname}`);
}

async function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) throw new Error('manifest directory required');
  const source = fs.readFileSync(path.join(manifestDir, 'assets/removals.js'), 'utf8');
  const copy = copyTable(source);
  await reviewTransportCases(source);
  await assertStaysOff(source, '/app/home/');
  await assertStaysOff(source, '/app/home/week/20261019');

  const stream = '<img src=x onerror=stream>';
  const staged = 'chronicle/<img src=x onerror=staged>';
  const name = '<img src=x onerror=name>';
  const reason = '<img src=x onerror=reason>';
  const markedRow = marked('marked', 'policy', 1, stream);
  const failed = { id: 'failed', state: 'failed', day: '20260101', stream, staged };
  const escapedCard = await boot(
    source,
    { state: 'list.ready', removals: [markedRow, failed] },
    {
      approve: {
        state: 'approve.refused_after_start',
        removed_count: 0,
        not_removed_count: 1,
        refusals: [{ state: 'refusal.item_named', name, reason }]
      }
    }
  );
  await approve(escapedCard, 'marked');
  for (const value of [stream, name, reason]) {
    assert(!escapedCard.innerHTML.includes(value), `raw journal value rendered: ${value}`);
    assert(escapedCard.innerHTML.includes(escaped(value)), `escaped journal value missing: ${value}`);
  }
  assert(!escapedCard.innerHTML.includes(staged), 'raw staged path must not render');
  assert(escapedCard.innerHTML.includes(copy['failed.body']), 'failed.body bytes missing');
  assert(!escapedCard.innerHTML.includes('<img'), 'journal markup must not become live DOM');

  const confirmationRows = [
    ['policy-one', 'policy', 1, 'kitchen-mic', 'confirm.body_policy_one'],
    ['policy-many', 'policy', 2, 'kitchen-mic', 'confirm.body_policy_many'],
    ['offload-one', 'offload', 1, 'kitchen-mic', 'confirm.body_offload_one'],
    ['offload-many', 'offload', 2, 'kitchen-mic', 'confirm.body_offload_many']
  ];
  for (const [id, origin, count, streamName, key] of confirmationRows) {
    const row = marked(id, origin, count, streamName);
    const card = await boot(
      source,
      { state: 'list.ready', removals: [row] },
      { approve: { state: 'approve.refused_before_start', refusals: [] } }
    );
    await confirmation(card, id);
    assert(card.innerHTML.includes(rendered(copy, key, { n: count, date: row.day, stream: streamName })));
  }

  // A seeded provenance label names the row; the stream key stays in its title.
  const labelledRow = marked('labelled', 'policy', 2, 'device_9e92ab54');
  const labelledCard = await boot(
    source,
    { state: 'list.ready', removals: [labelledRow] },
    { approve: { state: 'approve.refused_before_start', refusals: [] } },
    { device_9e92ab54: 'LABEL-ONE' }
  );
  const identityMatch = labelledCard.innerHTML.match(/<p class="removals-card-identity" data-removal-identity title="([^"]*)">([^<]*)<\/p>/);
  assert(identityMatch, 'labelled row states its identity');
  assert.strictEqual(identityMatch[1], 'device_9e92ab54');
  assert(identityMatch[2].includes('LABEL-ONE') && !identityMatch[2].includes('device_9e92ab54'), identityMatch[2]);

  const defaultRow = marked('default', 'policy', 2, '_default');
  const defaultCard = await boot(
    source,
    { state: 'list.ready', removals: [defaultRow] },
    { approve: { state: 'approve.refused_before_start', refusals: [] } }
  );
  assert(defaultCard.innerHTML.includes('data-removal-identity>Thu Jan 1</p>'));
  assert(!defaultCard.innerHTML.includes('20260101 ·'));
  assert(!defaultCard.innerHTML.includes('_default'));
  await confirmation(defaultCard, 'default');
  assert(defaultCard.innerHTML.includes(defaultStreamRendered(copy, 'confirm.body_policy_many', {
    n: 2,
    date: '20260101',
    stream: '_default'
  })));
  assert(!defaultCard.innerHTML.includes('20260101 ·'));

  const outcomeRow = marked('outcome', 'policy', 5, 'kitchen-mic');
  const outcomeCard = await boot(
    source,
    { state: 'list.ready', removals: [outcomeRow] },
    {
      approve: {
        state: 'approve.halted',
        removed_count: 2,
        not_removed_count: 3,
        halted: true,
        refusals: [
          { state: 'refusal.item_named', name: 'left.flac', reason: 'kept' },
          { state: 'refusal.item_unnamed', reason: 'unnamed reason' }
        ]
      }
    }
  );
  await approve(outcomeCard, 'outcome');
  const clauses = [
    rendered(copy, 'done.clause_deleted_many', { n: 2 }),
    rendered(copy, 'done.clause_not_removed_many', { m: 3 }),
    copy['done.clause_halted']
  ].join(' ');
  assert(outcomeCard.innerHTML.includes(clauses));
  assert(outcomeCard.innerHTML.includes(rendered(copy, 'done.refused_item', { name: 'left.flac', reason: 'kept' })));
  assert(outcomeCard.innerHTML.includes(rendered(copy, 'done.refused_item_unnamed', { reason: 'unnamed reason' })));
  assert(outcomeCard.innerHTML.indexOf(clauses) < outcomeCard.innerHTML.indexOf('<ul>'));

  const declineStates = [
    ['declined.done', copy['done.kept_policy']],
    ['declined.partial', [
      rendered(copy, 'done.clause_kept_many', { n: 2 }),
      copy['done.clause_not_kept_one'],
      copy['done.clause_nothing_deleted'],
    ].join(' ')],
    ['declined.refused', copy['done.declined_failed']],
    ['declined.unknown', copy['done.declined_unknown']],
    ['tool.unavailable', copy['done.declined_failed']],
    ['request.too_large', rendered(copy, 'done.too_many', { n: 32 })],
    ['outcome.unknown', copy['done.declined_unknown']],
    ['request.invalid', '']
  ];
  for (const [state, expected] of declineStates) {
    const row = marked(`decline-${state}`, 'policy', 1, 'kitchen-mic');
    const card = await boot(
      source,
      { state: 'list.ready', removals: [row] },
      { decline: state === 'declined.partial' ? { state, declined_count: 2, refused_count: 1 } : { state } }
    );
    click(card, 'decline', row.id);
    await settle();
    assert(expected === '' || outcome(card).includes(expected), `decline renders ${state}`);
    assert(expected !== '' || outcome(card) === '', `decline renders nothing for ${state}`);
    assertDeclineOutcomeHasNoDeletingCopy(card, copy);
  }

  const row = marked('decline-refresh', 'policy', 1, 'kitchen-mic');
  const declineRefreshCard = await boot(
    source,
    [
      { state: 'list.ready', removals: [row] },
      { state: 'outcome.unknown', removals: [] }
    ],
    { decline: { state: 'declined.done' } }
  );
  click(declineRefreshCard, 'decline', row.id);
  await settle();
  assert(declineRefreshCard.innerHTML.includes(copy['card.unavailable']));
  assert(outcome(declineRefreshCard).includes(copy['done.kept_policy']));
  assertDeclineOutcomeHasNoDeletingCopy(declineRefreshCard, copy);

  function confirmSection(card) {
    return card.innerHTML.match(/<section class="removals-card-confirm"[\s\S]*?<\/section>/)?.[0] || '';
  }

  assert(escapedCard.innerHTML.includes('data-removal-select'), 'marked row is selectable');
  assert(
    !escapedCard.innerHTML.split('data-removal-row')[2]?.includes('data-removal-select'),
    'failed row is not selectable'
  );
  assert.strictEqual(
    (escapedCard.innerHTML.match(/data-removal-action="finish"/g) || []).length,
    1,
    'finish control is card-level'
  );

  const two = [
    marked('bulk-a', 'policy', 2, 'kitchen-mic'),
    marked('bulk-b', 'policy', 5, 'kitchen-mic')
  ];
  const bulkCard = await boot(
    source,
    { state: 'list.ready', removals: two },
    { approve: { state: 'approve.refused_before_start', refusals: [] } }
  );
  clickSelect(bulkCard, 'bulk-a');
  await settle();
  clickSelect(bulkCard, 'bulk-b');
  await settle();
  click(bulkCard, 'delete-selected');
  await settle();
  const selectedConfirm = confirmSection(bulkCard);
  assert(selectedConfirm.includes(copy['confirm.heading_many']));
  assert(selectedConfirm.includes(copy['confirm.go_many']));
  assert(selectedConfirm.includes(rendered(copy, 'confirm.body_policy_selected', { n: 7 })));
  assert(!selectedConfirm.includes(copy['confirm.heading_one']));
  assert(!selectedConfirm.includes('20260101'));
  assert(!selectedConfirm.includes('kitchen-mic'));
  click(bulkCard, 'confirm');
  await settle();
  assert.strictEqual(posts(bulkCard, '/app/home/api/approve').length, 1);
  assert.strictEqual(posts(bulkCard, '/app/home/api/decline').length, 0);
  assert.deepStrictEqual(
    JSON.parse(posts(bulkCard, '/app/home/api/approve')[0].options.body).mark_ids,
    ['bulk-a', 'bulk-b']
  );

  const over = Array.from({ length: 33 }, (_, index) => marked(`cap-${index}`, 'policy', 1, 'kitchen-mic'));
  const overCard = await boot(
    source,
    { state: 'list.ready', removals: over },
    { approve: { state: 'approve.deleted' } }
  );
  click(overCard, 'select-all');
  await settle();
  // The cap is stated on the live count the moment the selection passes it,
  // not as a standing number beside a 20-row page. G1-204.
  assert(overCard.innerHTML.includes(rendered(copy, 'bulk.selected_over_cap', { n: 33, max: 32 })),
    'over-cap selection states the cap');
  assert(!/choose up to \d+ items per action/.test(overCard.innerHTML),
    'the standing scope sentence no longer names a cap');
  click(overCard, 'delete-selected');
  await settle();
  assert(outcome(overCard).includes(rendered(copy, 'done.too_many', { n: 32 })));
  assert.strictEqual(confirmSection(overCard), '');
  assert.strictEqual(posts(overCard, '/app/home/api/approve').length, 0);

  const pagedRows = Array.from({ length: 25 }, (_, index) => marked(`page-${index}`, 'policy', 1, 'mic'));
  const pagedCard = await boot(source, [{ state: 'list.ready', removals: pagedRows.slice(0, 20), offset: 0, next_offset: 20, total_rows: 25 }, { state: 'list.ready', removals: pagedRows.slice(20), offset: 20, next_offset: null, total_rows: 25 }], { approve: { state: 'approve.refused_before_start', refusals: [] } });
  assert.strictEqual(pagedCard.selects.length, 20);
  clickSelect(pagedCard, 'page-0');
  click(pagedCard, 'delete-selected');
  const previousPageConfirm = pagedCard.controls.find(control => control.dataset.removalAction === 'confirm');
  click(pagedCard, 'next');
  previousPageConfirm.click();
  assert.strictEqual(posts(pagedCard, '/app/home/api/approve').length, 0, 'old page confirmation cannot submit during navigation');
  await settle();
  assert.strictEqual(pagedCard.selects.length, 5);
  assert(pagedCard.calls.some(call => call.url.endsWith('?offset=20')));
  assert(!pagedCard.innerHTML.includes('1 selected'), 'page navigation clears selection');
  clickSelect(pagedCard, 'page-24');
  click(pagedCard, 'delete-selected');
  assert.strictEqual(posts(pagedCard, '/app/home/api/approve').length, 0);
  click(pagedCard, 'confirm');
  await settle();
  assert.deepStrictEqual(JSON.parse(posts(pagedCard, '/app/home/api/approve')[0].options.body).mark_ids, ['page-24'], 'approval includes only the explicitly selected current-page item');

  const oversizedCard = await boot(source, [
    { state: 'list.review_too_large', removals: [pagedRows[0]], review_context: null, offset: 0, next_offset: 1, total_rows: 2 },
    { state: 'list.ready', removals: [pagedRows[1]], offset: 1, next_offset: null, total_rows: 2 },
  ], {});
  assert(oversizedCard.innerHTML.includes(copy['card.too_large']));
  assert(oversizedCard.innerHTML.includes('/app/support/'));
  assert(!oversizedCard.controls.some(control => ['approve', 'decline', 'select-all'].includes(control.dataset.removalAction)), 'oversized evidence never grants an action');
  click(oversizedCard, 'next');
  await settle();
  assert.strictEqual(oversizedCard.selects.length, 1, 'an unavailable item does not block reviewing the next item');

  const failedOnly = { id: 'failed-only', state: 'failed', day: '20260101', stream: 'kitchen-mic' };
  const recoverCard = await boot(
    source,
    [
      { state: 'list.ready', removals: [failedOnly] },
      { state: 'list.empty', removals: [] }
    ],
    { recover: { state: 'recover.done', finished_count: 1 } }
  );
  click(recoverCard, 'finish');
  await settle();
  assert(confirmSection(recoverCard).includes(copy['confirm.recover.heading']));
  assert(confirmSection(recoverCard).includes(copy['confirm.recover.body']));
  click(recoverCard, 'confirm-finish');
  await settle();
  assert.strictEqual(posts(recoverCard, '/app/home/api/recover').length, 1);
  assert.strictEqual(posts(recoverCard, '/app/home/api/recover')[0].options.body, '{}');
  assert.strictEqual(posts(recoverCard, '/app/home/api/approve').length, 0);
  assert(outcome(recoverCard).includes(copy['done.recovered']));

  const leftoverCard = await boot(
    source,
    [
      { state: 'list.ready', removals: [failedOnly] },
      { state: 'list.ready', removals: [] }
    ],
    { recover: { state: 'recover.failed', finished_count: 1, not_finished_count: 1 } }
  );
  click(leftoverCard, 'finish');
  await settle();
  click(leftoverCard, 'confirm-finish');
  await settle();
  assert(outcome(leftoverCard).includes(copy['done.recovered_leftover']));

  const unknownRefresh = await boot(
    source,
    [
      { state: 'list.ready', removals: [failedOnly] },
      { state: 'outcome.unknown', removals: [] }
    ],
    { recover: { state: 'recover.done', finished_count: 1 } }
  );
  click(unknownRefresh, 'finish');
  await settle();
  click(unknownRefresh, 'confirm-finish');
  await settle();
  assert(outcome(unknownRefresh).includes(copy['done.recover_unknown']));
  assert(!outcome(unknownRefresh).includes(copy['done.recovered']));

  const noneCard = await boot(
    source,
    [
      { state: 'list.ready', removals: [failedOnly] },
      { state: 'list.empty', removals: [] }
    ],
    { recover: { state: 'recover.none', finished_count: 0 } }
  );
  click(noneCard, 'finish');
  await settle();
  click(noneCard, 'confirm-finish');
  await settle();
  assert(outcome(noneCard).includes(copy['done.recovered_none']));

  const unavailableCard = await boot(
    source,
    [
      { state: 'list.ready', removals: [failedOnly] },
      { state: 'list.ready', removals: [failedOnly] }
    ],
    { recover: { state: 'tool.unavailable' } }
  );
  click(unavailableCard, 'finish');
  await settle();
  click(unavailableCard, 'confirm-finish');
  await settle();
  assert(outcome(unavailableCard).includes(copy['done.recover_failed']));

  const keepRows = [
    marked('keep-a', 'policy', 1, 'kitchen-mic'),
    marked('keep-b', 'policy', 1, 'kitchen-mic')
  ];
  const keepCard = await boot(
    source,
    { state: 'list.ready', removals: keepRows },
    { decline: { state: 'declined.done' } }
  );
  clickSelect(keepCard, 'keep-a');
  await settle();
  clickSelect(keepCard, 'keep-b');
  await settle();
  click(keepCard, 'keep-selected');
  await settle();
  assert.strictEqual(posts(keepCard, '/app/home/api/decline').length, 1);
  assert.strictEqual(posts(keepCard, '/app/home/api/approve').length, 0);
  assert.deepStrictEqual(
    JSON.parse(posts(keepCard, '/app/home/api/decline')[0].options.body).mark_ids,
    ['keep-a', 'keep-b']
  );

  const perRow = marked('per-row', 'policy', 1, 'kitchen-mic');
  const perRowApprove = await boot(
    source,
    { state: 'list.ready', removals: [perRow] },
    { approve: { state: 'approve.refused_before_start', refusals: [] } }
  );
  await approve(perRowApprove, 'per-row');
  assert.strictEqual(posts(perRowApprove, '/app/home/api/approve').length, 1);
  assert.deepStrictEqual(
    JSON.parse(posts(perRowApprove, '/app/home/api/approve')[0].options.body).mark_ids,
    ['per-row']
  );
  const perRowDecline = await boot(
    source,
    { state: 'list.ready', removals: [perRow] },
    { decline: { state: 'declined.done' } }
  );
  click(perRowDecline, 'decline', 'per-row');
  await settle();
  assert.strictEqual(posts(perRowDecline, '/app/home/api/decline').length, 1);
  assert.deepStrictEqual(
    JSON.parse(posts(perRowDecline, '/app/home/api/decline')[0].options.body).mark_ids,
    ['per-row']
  );

  const leftoverZero = await boot(
    source,
    [
      { state: 'list.ready', removals: [failedOnly] },
      { state: 'list.ready', removals: [failedOnly] }
    ],
    { recover: { state: 'recover.done', finished_count: 0 } }
  );
  click(leftoverZero, 'finish');
  await settle();
  click(leftoverZero, 'confirm-finish');
  await settle();
  assert(outcome(leftoverZero).includes(copy['done.recover_failed']));
  assert(!outcome(leftoverZero).includes(copy['done.recovered']));
  assert(!outcome(leftoverZero).includes(copy['done.recovered_none']));

  const oneOfTwo = [
    marked('one-select', 'policy', 1, 'kitchen-mic'),
    marked('other-select', 'policy', 5, 'kitchen-mic')
  ];
  const oneSelectCard = await boot(
    source,
    { state: 'list.ready', removals: oneOfTwo },
    { approve: { state: 'approve.refused_before_start', refusals: [] } }
  );
  clickSelect(oneSelectCard, 'one-select');
  await settle();
  click(oneSelectCard, 'delete-selected');
  await settle();
  const oneSelectConfirm = confirmSection(oneSelectCard);
  assert(oneSelectConfirm.includes(rendered(copy, 'confirm.body_policy_one', {
    n: 1,
    date: '20260101',
    stream: 'kitchen-mic'
  })));
  assert(!oneSelectConfirm.includes(rendered(copy, 'confirm.body_policy_selected', { n: 1 })));

  const selectMix = [
    marked('sel-a', 'policy', 1, 'kitchen-mic'),
    marked('sel-b', 'policy', 1, 'kitchen-mic'),
    { id: 'sel-failed', state: 'failed', day: '20260101', stream: 'kitchen-mic' }
  ];
  const selectCard = await boot(
    source,
    { state: 'list.ready', removals: selectMix },
    { approve: { state: 'approve.deleted' }, decline: { state: 'declined.done' } }
  );
  click(selectCard, 'select-all');
  await settle();
  assert(selectCard.innerHTML.includes(rendered(copy, 'bulk.selected_many', { n: 2 })));
  const failedArticle = selectCard.innerHTML.match(
    /<article[^>]*data-mark-id="sel-failed"[^>]*>[\s\S]*?<\/article>/
  )?.[0] || '';
  assert(failedArticle, 'failed row present after select-all');
  assert(!failedArticle.includes(' checked'), 'failed row is not checked');
  click(selectCard, 'clear-selection');
  await settle();
  assert(!selectCard.innerHTML.includes(rendered(copy, 'bulk.selected_many', { n: 2 })));
  assert(!selectCard.innerHTML.includes(copy['bulk.selected_one']));
  click(selectCard, 'delete-selected');
  await settle();
  click(selectCard, 'keep-selected');
  await settle();
  assert.strictEqual(posts(selectCard, '/app/home/api/approve').length, 0);
  assert.strictEqual(posts(selectCard, '/app/home/api/decline').length, 0);
  assert.strictEqual(confirmSection(selectCard), '');

  // G1-30. Four identical rows used to repeat the same two shared sentences
  // four times. They are stated once above the list now; the row keeps only
  // what differs between rows -- its date and stream, its count and size, and
  // its own actions -- and none of the selection behaviour moves.
  const occurrences = (haystack, needle) => haystack.split(needle).length - 1;
  const sharedNote = (card) => (
    card.innerHTML.match(/<p class="removals-card-origin" data-removals-note>([\s\S]*?)<\/p>/)?.[1] || ''
  );
  const identical = Array.from(
    { length: 4 },
    (_, index) => marked(`same-${index}`, 'policy', 2, 'kitchen-mic')
  );
  const noteCard = await boot(
    source,
    { state: 'list.ready', removals: identical },
    { approve: { state: 'approve.deleted' }, decline: { state: 'declined.done' } }
  );

  // The origin sentences carry their own count now, so the expectation is the
  // rendered sentence (4 rows x 2 originals = 8), not the raw template.
  for (const key of ['row.origin_policy_many', 'row.kept_many']) {
    const expected = rendered(copy, key, { n: 8 });
    assert.strictEqual(
      occurrences(noteCard.innerHTML, expected),
      1,
      `${key} is stated once for the list, not once per row`
    );
    assert(sharedNote(noteCard).includes(expected), `the shared note carries ${key}`);
  }
  assert(
    noteCard.innerHTML.indexOf('</summary>') < noteCard.innerHTML.indexOf('data-removals-note'),
    'the shared note sits inside the review disclosure'
  );
  assert(
    noteCard.innerHTML.indexOf('data-removals-note') < noteCard.innerHTML.indexOf('data-removal-row'),
    'the shared note sits above the first row'
  );

  const articles = Array.from(noteCard.innerHTML.matchAll(
    /<article[^>]*data-removal-row[^>]*>[\s\S]*?<\/article>/g
  )).map((match) => match[0]);
  assert.strictEqual(articles.length, identical.length, 'every marked record still renders its own row');
  articles.forEach((article, index) => {
    const id = `same-${index}`;
    assert(article.includes(`data-mark-id="${id}"`), `row ${id} keeps its mark id`);
    for (const key of ['row.origin_policy_one', 'row.origin_policy_many', 'row.kept_one', 'row.kept_many']) {
      assert(!article.includes(copy[key]), `row ${id} must not repeat ${key}`);
    }
    assert(article.includes('data-removal-identity'), `row ${id} still states its date and stream`);
    // G1-203: the label names the source the way the owner says it, so the
    // exact stream key rides in the title where the decision can be checked.
    assert(article.includes('title="kitchen-mic"'), `row ${id} carries its exact stream key`);
    assert(
      article.includes(rendered(copy, 'row.what_many', { n: 2, size: '2 B' })),
      `row ${id} still states its count and size`
    );
    assert(article.includes('data-removal-select'), `row ${id} is still individually selectable`);
    for (const action of ['approve', 'decline']) {
      assert(
        noteCard.controls.some((control) => (
          control.dataset.removalAction === action && control.dataset.markId === id
        )),
        `row ${id} still exposes its ${action} action`
      );
    }
  });

  // Selection still spans the whole list and a bulk action still means "every
  // selected record", counted in originals rather than rows.
  click(noteCard, 'select-all');
  await settle();
  assert(noteCard.innerHTML.includes(rendered(copy, 'bulk.selected_many', { n: 4 })));
  click(noteCard, 'delete-selected');
  await settle();
  assert(
    confirmSection(noteCard).includes(rendered(copy, 'confirm.body_policy_selected', { n: 8 })),
    'the bulk confirmation still counts originals across the selection'
  );
  click(noteCard, 'confirm');
  await settle();
  assert.deepStrictEqual(
    JSON.parse(posts(noteCard, '/app/home/api/approve')[0].options.body).mark_ids,
    ['same-0', 'same-1', 'same-2', 'same-3']
  );

  // A mixed list keeps both origins distinguishable, still once each.
  const mixedCard = await boot(
    source,
    {
      state: 'list.ready',
      removals: [
        marked('mix-policy', 'policy', 1, 'kitchen-mic'),
        marked('mix-offload', 'offload', 3, 'kitchen-mic')
      ]
    },
    { approve: { state: 'approve.refused_before_start', refusals: [] } }
  );
  // 1 policy original and 3 offload originals, so 4 kept. Each clause names its
  // own count, which is what tells the reader which rows it covers.
  const mixedCounts = { 'row.origin_policy_one': 1, 'row.origin_offload_many': 3, 'row.kept_many': 4 };
  for (const [key, n] of Object.entries(mixedCounts)) {
    const expected = rendered(copy, key, { n });
    assert(sharedNote(mixedCard).includes(expected), `the mixed note carries ${key}`);
    assert.strictEqual(occurrences(mixedCard.innerHTML, expected), 1, `${key} is stated once`);
  }
  // Rendered with the counts this list actually has, so the negative still has
  // teeth: an unrendered `{n}` would be trivially absent.
  const mixedAbsent = { 'row.origin_policy_many': 1, 'row.origin_offload_one': 3, 'row.kept_one': 4 };
  for (const [key, n] of Object.entries(mixedAbsent)) {
    assert(
      !sharedNote(mixedCard).includes(rendered(copy, key, { n })),
      `the mixed note must not carry ${key}`
    );
  }

  // Nothing marked means no shared note, and the unfinished row still explains
  // itself: "nothing is waiting on you" and "a deletion stopped" stay distinct.
  const failedOnlyCard = await boot(
    source,
    { state: 'list.ready', removals: [failedOnly] },
    { recover: { state: 'recover.done', finished_count: 1 } }
  );
  assert(!failedOnlyCard.innerHTML.includes('data-removals-note'), 'no shared note when nothing is marked');
  assert(failedOnlyCard.innerHTML.includes(copy['failed.body']), 'the unfinished row still explains itself');
  assert(!failedOnlyCard.innerHTML.includes(copy['card.empty']), 'unfinished is not the empty state');
  await asyncReceiving(source);
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function asyncReceiving(source) {
  const copy = copyTable(source);
  const gen = '9007199254740993';
  const base = { state: 'list.ready', review_context: 'opaque-original', removals: [marked('typed', 'offload', 1, '_default'), marked('policy', 'policy', 1, '_default')] };
  const effect = { outcome: { targets: [{ removed: ['chronicle/<raw>.flac'], not_removed: [{ entry: 'policy.flac', reason: '<unproven>' }] }], halted: null } };
  const status = (state, settled, cancelled = false, report = null) => ({ generation: gen, state, worker_settled: settled, cancellation_requested: cancelled, actor_phase: 'done', report });
  const submit = async card => { click(card, 'approve', 'typed'); click(card, 'confirm'); await settle(); };
  const urls = card => card.calls.filter(call => call.url.startsWith('/app/home/api/approve/'));

  // The first unresolved POST is already protected, including retained controls.
  {
    const start = deferred();
    const card = await boot(source, base, { approve: () => start.promise });
    click(card, 'approve', 'typed');
    const retained = card.controls.find(control => control.dataset.removalAction === 'confirm');
    retained.click(); retained.click(); await settle();
    await card.refresh();
    click(card, 'approve', 'typed');
    assert.strictEqual(posts(card, '/app/home/api/approve').length, 1);
    start.resolve({ state: 'approve.refused_before_start' }); await settle();
    assert.strictEqual(posts(card, '/app/home/api/approve').length, 1);
  }
  // Initial resolve AND reject after replacement cannot touch the successor.
  for (const reject of [false, true]) {
    const start = deferred();
    const card = await boot(source, base, { approve: () => start.promise });
    await submit(card);
    await card.replace('health', false);
    const next = await card.replace('home', true);
    const before = next.innerHTML;
    const calls = card.calls.length;
    if (reject) start.reject(Error('detached')); else start.resolve({ state: 'approve.started', generation: gen });
    await settle();
    assert.strictEqual(next.innerHTML, before);
    assert.strictEqual(card.calls.length, calls);
    assert.strictEqual(card.timerCount(), 0);
  }
  // A status sent before stop can arrive after the actual true acknowledgment.
  {
    const oldStatus = deferred(), stop = deferred();
    let reads = 0;
    const tail = { state: 'list.ready', review_context: 'same-ID-reoffer', removals: [base.removals[1], { id: 'failed', state: 'failed', day: '20260101', stream: '_default', count: 1 }] };
    const card = await boot(source, [base, tail], {
      approve: { state: 'approve.started', generation: gen },
      receive(url) {
        if (url.endsWith('/cancel')) return stop.promise;
        reads += 1;
        return reads === 1 ? status('running', false) : reads === 2 ? oldStatus.promise : status('held', true, true, effect);
      }
    });
    await submit(card);
    assert.strictEqual(card.timerCount(), 1);
    await card.tick();
    assert.strictEqual(card.timerCount(), 0, 'no second poll while status is in flight');
    click(card, 'stop'); click(card, 'stop'); await settle();
    assert.strictEqual(urls(card).filter(call => call.url.endsWith('/cancel')).length, 1);
    stop.resolve(status('running', false, true)); await settle();
    oldStatus.resolve(status('running', false, false)); await settle();
    assert(card.innerHTML.includes(copy['operation.stopping']), 'old false cannot undo observed true');
    assert.strictEqual(card.timerCount(), 1);
    await card.tick();
    assert(card.innerHTML.includes(copy['operation.held']));
    assert(card.innerHTML.includes('chronicle/&lt;raw&gt;.flac'));
    assert(card.innerHTML.includes('&lt;unproven&gt;'));
    assert(!card.innerHTML.includes('chronicle/<raw>.flac'));
    assert(card.innerHTML.includes('data-mark-id="policy"') && card.innerHTML.includes('data-mark-id="failed"'));
    assert.strictEqual(card.timerCount(), 0);
    click(card, 'approve', 'policy'); await settle();
    assert.strictEqual(posts(card, '/app/home/api/approve').length, 1, 'Held stays blocked after context refresh');
    for (const call of urls(card)) assert.strictEqual(call.options.cache, 'no-store');
    assert(urls(card).every(call => call.url.includes(gen)), 'opaque generation never rounds');
  }
  // Older same-generation cancel snapshots AND errors cannot regress settlement.
  for (const reject of [false, true]) {
    const stop = deferred(); let reads = 0;
    const card = await boot(source, base, {
      approve: { state: 'approve.started', generation: gen },
      receive(url) { if (url.endsWith('/cancel')) return stop.promise; return ++reads === 1 ? status('running', false) : status('completed', true, true, effect); }
    });
    await submit(card); click(card, 'stop'); await settle();
    await card.tick();
    const before = card.innerHTML;
    if (reject) stop.reject(Error('late cancel')); else stop.resolve(status('running', false, false));
    await settle();
    assert.strictEqual(card.innerHTML, before, 'late cancel cannot downgrade settled report');
    assert(card.innerHTML.includes(copy['operation.completed']));
    assert.strictEqual(card.timerCount(), 0);
  }
  // Held/Unknown before closure settlement continues observation. Malformed
  // transport and a failed terminal list refresh keep earlier actual facts.
  {
    let reads = 0;
    const statuses = [status('held', false, false, effect), { state: 'approve.stale' }, status('unknown', true, false, null)];
    const card = await boot(source, [base, { state: 'list.register_unavailable' }], {
      approve: { state: 'approve.started', generation: gen }, receive() { return statuses[reads++]; }
    });
    await submit(card); assert.strictEqual(card.timerCount(), 1);
    await card.tick(); assert(card.innerHTML.includes('chronicle/&lt;raw&gt;.flac'));
    assert.strictEqual(card.timerCount(), 1);
    await card.tick();
    assert.strictEqual(card.timerCount(), 0);
    assert(card.innerHTML.includes('data-removals-list-unavailable'));
    assert(card.innerHTML.includes('chronicle/&lt;raw&gt;.flac'));
    assert(card.innerHTML.includes(copy['operation.unknown']));
  }
  // Unknown initial transport never creates a generation or retries execution.
  for (const response of [null, { state: 'approve.started', generation: '01' }, { state: 'unrecognized' }]) {
    const card = await boot(source, base, { approve: response });
    await submit(card); await card.refresh(); click(card, 'approve', 'typed'); await settle();
    assert.strictEqual(posts(card, '/app/home/api/approve').length, 1);
    assert.strictEqual(urls(card).length, 0);
    assert(card.innerHTML.includes(copy['operation.unknown']));
  }
  // Delayed status/list/stop callbacks from a retired script cannot render or
  // request anything on the successor. The real shell replay is checked in Chrome.
  {
    const pending = deferred(), stop = deferred(); let reads = 0;
    const card = await boot(source, base, {
      approve: { state: 'approve.started', generation: gen },
      receive(url) { if (url.endsWith('/cancel')) return stop.promise; return ++reads === 1 ? status('running', false) : pending.promise; }
    });
    await submit(card); click(card, 'stop'); await settle(); await card.tick();
    const retiredKeep = card.controls.find(control => control.dataset.removalAction === 'decline');
    const beforeDetach = card.calls.length;
    await card.replace('health', false);
    retiredKeep.click(); await settle();
    assert.strictEqual(card.calls.length, beforeDetach, 'retired controls cannot act between workspaces');
    const next = await card.replace('home', true);
    const before = next.innerHTML, calls = card.calls.length;
    pending.resolve(status('completed', true, true, effect)); stop.resolve(status('running', false, true)); await settle();
    assert.strictEqual(next.innerHTML, before); assert.strictEqual(card.calls.length, calls);
    assert.strictEqual(card.timerCount(), 0);
  }
  // Immutable predecessor statuses are data only. The added Boolean below is
  // an explicitly synthetic supplement, qualified by separate owning controls.
  if (process.env.HOME_APPROVAL_STATUS_RECEIPT) {
    const bytes = fs.readFileSync(process.env.HOME_APPROVAL_STATUS_RECEIPT);
    const receipt = JSON.parse(bytes);
    for (const mode of ['complete', 'cancel_before_finish']) {
      const literal = receipt.cases.find(item => item.mode === mode).result.status;
      const supplemented = { ...literal, cancellation_requested: false };
      const card = await boot(source, base, {
        approve: { state: 'approve.started', generation: literal.generation }, receive() { return supplemented; }
      });
      await submit(card);
      assert(card.innerHTML.includes(copy[literal.state === 'completed' ? 'operation.completed' : 'operation.held']));
      assert.strictEqual(card.timerCount(), 0);
      assert.strictEqual(literal.actor_phase, 'done');
    }
    console.log(JSON.stringify({ predecessor_receipt_sha256: require('crypto').createHash('sha256').update(bytes).digest('hex'), supplement: 'synthetic cancellation_requested=false; original receipt untouched', modes: ['complete', 'cancel_before_finish'] }));
  }
  console.log('async approval receiving controls passed');
}

main().catch((error) => {
  process.stderr.write(`${error && error.stack ? error.stack : error}\n`);
  process.exitCode = 1;
});
