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

function rendered(copy, key, values) {
  values = { ...values, ...(values.date ? { date: 'Thu Jan 1' } : {}), ...(values.stream ? { stream: values.stream.replace(/[._-]+/g, ' ').trim() } : {}) };
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

async function boot(source, lists, responses) {
  const root = new Element();
  const listResponses = Array.isArray(lists) ? lists : [lists];
  let listIndex = 0;
  const calls = [];
  const window = {
    apiJson(url, options) {
      calls.push({ url, options: options || null });
      if (url === '/app/home/api/removals') {
        const response = listResponses[Math.min(listIndex, listResponses.length - 1)];
        listIndex += 1;
        return Promise.resolve(response);
      }
      if (url === '/app/home/api/approve') return Promise.resolve(responses.approve);
      if (url === '/app/home/api/decline') return Promise.resolve(responses.decline);
      if (url === '/app/home/api/recover') return Promise.resolve(responses.recover);
      throw new Error(`unexpected URL: ${url}`);
    }
  };
  const document = {
    readyState: 'complete',
    addEventListener() {},
    createElement() { return new Element(); },
    querySelector(selector) { return selector === '[data-home-root]' ? root : null; }
  };
  window.window = window;
  class FixtureDate extends Date { constructor(...args) { super(...(args.length ? args : [2026, 8, 5])); } }
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../../solstone-core-convey-shell/assets/static/date_format.js'), 'utf8'), { window, Date: FixtureDate });
  vm.runInNewContext(source, { document, Promise, setImmediate, window, Set }, { filename: 'removals.js' });
  await settle();
  const card = root.children[0];
  assert(card, 'card mounted');
  card.calls = calls;
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

async function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) throw new Error('manifest directory required');
  const source = fs.readFileSync(path.join(manifestDir, 'assets/removals.js'), 'utf8');
  const copy = copyTable(source);

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
    ['declined.partial', copy['done.declined_failed']],
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
      { decline: { state } }
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
  click(overCard, 'delete-selected');
  await settle();
  assert(outcome(overCard).includes(rendered(copy, 'done.too_many', { n: 32 })));
  assert.strictEqual(confirmSection(overCard), '');
  assert.strictEqual(posts(overCard, '/app/home/api/approve').length, 0);

  const pagedRows = Array.from({ length: 25 }, (_, index) => marked(`page-${index}`, 'policy', 1, 'mic'));
  const pagedCard = await boot(source, { state: 'list.ready', removals: pagedRows }, { approve: { state: 'approve.refused_before_start', refusals: [] } });
  assert.strictEqual(pagedCard.selects.length, 20);
  clickSelect(pagedCard, 'page-0');
  click(pagedCard, 'next');
  assert.strictEqual(pagedCard.selects.length, 5);
  clickSelect(pagedCard, 'page-24');
  click(pagedCard, 'delete-selected');
  assert.strictEqual(posts(pagedCard, '/app/home/api/approve').length, 0);
  click(pagedCard, 'confirm');
  await settle();
  assert.deepStrictEqual(JSON.parse(posts(pagedCard, '/app/home/api/approve')[0].options.body).mark_ids, ['page-0', 'page-24']);

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
    assert(article.includes('data-removal-identity>'), `row ${id} still states its date and stream`);
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
}

main().catch((error) => {
  process.stderr.write(`${error && error.stack ? error.stack : error}\n`);
  process.exitCode = 1;
});
