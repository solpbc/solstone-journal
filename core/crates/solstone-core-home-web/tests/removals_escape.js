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

class Element {
  constructor() {
    this.children = [];
    this.controls = [];
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
  }

  querySelectorAll(selector) {
    return selector === '[data-removal-action]' ? this.controls : [];
  }

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
  return copy[key].replace(/\{([^}]+)\}/g, (_, name) => String(values[name] ?? ''));
}

function defaultStreamRendered(copy, key, values) {
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
  const window = {
    apiJson(url) {
      if (url === '/app/home/api/removals') {
        const response = listResponses[Math.min(listIndex, listResponses.length - 1)];
        listIndex += 1;
        return Promise.resolve(response);
      }
      if (url === '/app/home/api/approve') return Promise.resolve(responses.approve);
      if (url === '/app/home/api/decline') return Promise.resolve(responses.decline);
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
  vm.runInNewContext(source, { document, Promise, setImmediate, window }, { filename: 'removals.js' });
  await settle();
  const card = root.children[0];
  assert(card, 'card mounted');
  return card;
}

function click(card, action, markId) {
  const control = card.controls.find((item) => (
    item.dataset.removalAction === action && (markId === undefined || item.dataset.markId === markId)
  ));
  assert(control, `${action} control present`);
  control.click();
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
  for (const value of [stream, staged, name, reason]) {
    assert(!escapedCard.innerHTML.includes(value), `raw journal value rendered: ${value}`);
    assert(escapedCard.innerHTML.includes(escaped(value)), `escaped journal value missing: ${value}`);
  }
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
  assert(defaultCard.innerHTML.includes('data-removal-identity>20260101</p>'));
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
    ['request.too_large', copy['done.too_many']],
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
}

main().catch((error) => {
  process.stderr.write(`${error && error.stack ? error.stack : error}\n`);
  process.exitCode = 1;
});
