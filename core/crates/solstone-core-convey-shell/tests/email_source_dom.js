// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// End-to-end DOM tests for `/app/sources/email` running on the shared
// selective-sync frame (`SolSelectiveSync`) under plain Node.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { journalContext } = require('./journal_catalog_dom.js');

const NOW = Date.parse('2026-11-25T22:10:00Z');
const ZONE = 'America/Denver';
const MIN = 60 * 1000;

let cases = 0;
async function test(name, fn) {
  try {
    await fn();
    cases += 1;
  } catch (error) {
    error.message = `email_source_dom: ${name}: ${error.message}`;
    throw error;
  }
}

class Target {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, fn) {
    const list = this.listeners.get(type) || [];
    if (!list.includes(fn)) list.push(fn);
    this.listeners.set(type, list);
  }
  removeEventListener(type, fn) {
    const list = this.listeners.get(type) || [];
    this.listeners.set(type, list.filter((f) => f !== fn));
  }
  fire(event) {
    for (const fn of [...(this.listeners.get(event.type) || [])]) fn.call(this, event);
  }
}

class TextNode {
  constructor(text) { this.nodeType = 3; this.data = String(text); this.parentNode = null; }
  get textContent() { return this.data; }
}

function simpleMatch(el, part) {
  const re = /([a-zA-Z][a-zA-Z0-9-]*)|#([\w-]+)|\.([\w-]+)|\[([\w-]+)(?:=["']?([^\]"']*)["']?)?\]/g;
  let m;
  let any = false;
  while ((m = re.exec(part))) {
    any = true;
    if (m[1] && el.tagName !== m[1].toUpperCase()) return false;
    if (m[2] && el.getAttribute('id') !== m[2]) return false;
    if (m[3] && !(el.getAttribute('class') || '').split(/\s+/).includes(m[3])) return false;
    if (m[4]) {
      if (!el.hasAttribute(m[4])) return false;
      if (m[5] !== undefined && el.getAttribute(m[4]) !== m[5]) return false;
    }
  }
  return any;
}

class Element extends Target {
  constructor(doc, tagName) {
    super();
    this.ownerDocument = doc;
    this.nodeType = 1;
    this.tagName = tagName.toUpperCase();
    this.attrs = new Map();
    this.childNodes = [];
    this.parentNode = null;
    this.style = {};
    this.value = '';
    this.checked = false;
    this.disabled = false;
  }
  get children() { return this.childNodes.filter((n) => n.nodeType === 1); }
  get className() { return this.getAttribute('class') || ''; }
  set className(v) { this.setAttribute('class', v); }
  get id() { return this.getAttribute('id') || ''; }
  setAttribute(k, v) { this.attrs.set(k, String(v)); }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  hasAttribute(k) { return this.attrs.has(k); }
  removeAttribute(k) { this.attrs.delete(k); }
  get textContent() { return this.childNodes.map((n) => n.textContent).join(''); }
  set textContent(v) {
    [...this.childNodes].forEach((n) => this.removeChild(n));
    if (String(v ?? '')) this.appendChild(new TextNode(v));
  }
  appendChild(node) {
    if (node.parentNode) node.parentNode.removeChild(node);
    node.parentNode = this;
    this.childNodes.push(node);
    return node;
  }
  removeChild(node) {
    const i = this.childNodes.indexOf(node);
    if (i >= 0) this.childNodes.splice(i, 1);
    node.parentNode = null;
    return node;
  }
  querySelectorAll(selector) {
    const parts = selector.split(',').map((one) => one.trim());
    const out = [];
    const pending = [this];
    while (pending.length) {
      const el = pending.pop();
      if (el !== this) {
        for (const part of parts) {
          if (simpleMatch(el, part)) { out.push(el); break; }
        }
      }
      const kids = el.children;
      for (let i = kids.length - 1; i >= 0; i -= 1) pending.push(kids[i]);
    }
    return out;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  focus() { this.ownerDocument.activeElement = this; }
  click() {
    if (this.disabled) return;
    const event = { type: 'click', target: this, preventDefault() {} };
    this.fire(event);
    if (this.tagName === 'BUTTON' && this.getAttribute('type') === 'submit') {
      for (let at = this.parentNode; at; at = at.parentNode) {
        if (at.tagName === 'FORM') {
          at.fire({ type: 'submit', target: at, preventDefault() {} });
          break;
        }
      }
    }
  }
  input(val) {
    this.value = val;
    this.fire({ type: 'input', target: this });
  }
  change() {
    this.fire({ type: 'change', target: this });
  }
}

function makeDoc() {
  const doc = new Target();
  doc.createElement = (tag) => new Element(doc, tag);
  doc.body = new Element(doc, 'body');
  const root = doc.createElement('main');
  root.setAttribute('id', 'sources-root');
  doc.body.appendChild(root);
  doc.activeElement = doc.body;
  doc.getElementById = (id) => (id === 'sources-root' ? root : doc.body.querySelector('#' + id));
  return { doc, root };
}

function flush() { return new Promise((r) => setImmediate(r)); }
async function settle() { for (let i = 0; i < 8; i += 1) await flush(); }

function harness(manifestDir, handler) {
  const { doc, root } = makeDoc();
  const requests = [];
  const timers = new Map();
  let nextTimer = 1;
  const ready = [];
  const win = {
    document: doc,
    location: { pathname: '/app/sources/email', href: 'http://journal.test/app/sources/email' },
    logError: () => {},
  };
  const ctx = journalContext({
    window: win, document: doc, location: win.location,
    Map, Set, Promise, Intl, Date, Math, Number, String, Object, Array, Boolean, JSON, Error, RegExp, encodeURIComponent, decodeURIComponent,
  });
  ctx.globalThis = win;
  vm.runInContext(fs.readFileSync(path.join(manifestDir, 'assets/sources/selective_sync_frame.js'), 'utf8'), ctx);
  vm.runInContext(fs.readFileSync(path.join(manifestDir, 'assets/sources/email_source.js'), 'utf8'), ctx);

  const viewCtx = {
    disposed: false,
    route: { pathname: '/app/sources/email' },
    now: () => NOW,
    timeZone: ZONE,
    ready: (s) => ready.push(s),
    on(target, type, fn) {
      target.addEventListener(type, fn);
      return () => target.removeEventListener(type, fn);
    },
    setTimeout(fn, ms) {
      const id = nextTimer++;
      timers.set(id, { fn, ms });
      return id;
    },
    clearTimeout(id) { timers.delete(id); },
    async fetch(url, init = {}) {
      const method = String(init.method || 'GET').toUpperCase();
      const body = init.body ? JSON.parse(init.body) : null;
      requests.push({ url: String(url), method, body });
      const res = await handler({ url: String(url), method, body });
      return {
        ok: res.status >= 200 && res.status < 300,
        status: res.status,
        json: async () => res.body,
      };
    },
  };

  const detail = win.SolSelectiveSync.getDetail('email');
  assert.notStrictEqual(detail, null, 'email detail registered on SolSelectiveSync');
  const handle = detail.mount(viewCtx, root, win.SolSelectiveSync);
  return { doc, root, requests, timers, ready, viewCtx, handle };
}

function iso(ms) { return new Date(ms).toISOString().replace('.000Z', 'Z'); }

module.exports = async function emailSourceDomCases(manifestDir) {
  await test('connect screen renders Trust Claim Disclosure, 3-step guide, handles typed errors, and transitions to scanning', async () => {
    let connectAttempt = 0;
    const h = harness(manifestDir, async ({ url, method, body }) => {
      if (url === '/app/sources/api/email' && method === 'GET') {
        return { status: 200, body: { source: 'email', state: 'ready_to_set_up', phase: 'not_connected', provider: 'gmail', connected: false, syncing: false } };
      }
      if (url === '/app/sources/api/email/connect' && method === 'POST') {
        connectAttempt += 1;
        if (connectAttempt === 1) {
          return { status: 400, body: { code: 'auth_failed', error: 'bad pw' } };
        }
        if (connectAttempt === 2) {
          return { status: 400, body: { code: 'all_mail_hidden', error: 'hidden' } };
        }
        assert.strictEqual(body.provider, 'gmail');
        assert.strictEqual(body.email, 'maya.chen@gmail.com');
        return {
          status: 200,
          body: {
            success: true,
            status: {
              source: 'email', state: 'setting_up', phase: 'scanning', provider: 'gmail', account: 'maya.chen@gmail.com',
              connected: true, syncing: true,
              scan: { complete: false, paused: false, checked: 500, total: 2000, sent_found: 42, eligible_sent: 40, earliest_month: 'march 2020', bodies_read: 0 },
              sync: { active: false, done: 0, total: 0 },
              accounting: { sent_total: 42, in_journal: 0, removed_by_owner: 0, outside_choice: 40, left_out: { total: 2, calendar_responses: 2, automatic_replies: 0, drafts: 0, delivery_reports: 0 } },
              runs: [],
            },
          },
        };
      }
      throw new Error('unexpected ' + method + ' ' + url);
    });
    await settle();

    // Check Trust Claim Disclosure, link to Google app passwords, help fold, show/hide toggle
    assert.ok(h.root.textContent.includes('a Google app password gives full access to your Google account'), 'surface trust claim');
    assert.ok(h.root.textContent.includes('how this works and how to revoke it'), 'trust disclosure fold');
    assert.ok(h.root.textContent.includes("don't see app passwords in Google?"), 'help fold');
    const googleLink = h.root.querySelectorAll('a').find((a) => a.getAttribute('href') === 'https://myaccount.google.com/apppasswords');
    assert.notStrictEqual(googleLink, null, 'direct Google app passwords link');

    // Toggle password visibility
    const pwToggle = h.root.querySelector('[data-pw-toggle="email-app-password"]');
    assert.strictEqual(h.root.querySelector('#email-app-password').getAttribute('type'), 'password');
    pwToggle.click();
    assert.strictEqual(h.root.querySelector('#email-app-password').getAttribute('type'), 'text');

    // Fill inputs & trigger auth_failed error (keeps focus on password input)
    h.root.querySelector('#email-address').input('maya.chen@gmail.com');
    h.root.querySelector('#email-app-password').input('wrong pass');
    h.root.querySelector('[data-email-action="connect"]').click();
    await settle();
    assert.ok(h.root.textContent.includes("Gmail didn't accept this email and app password"), 'auth_failed inline error');
    assert.strictEqual(h.doc.activeElement, h.root.querySelector('#email-app-password'), 'focuses password input on auth error');

    // Second attempt: all_mail_hidden error
    h.root.querySelector('[data-email-action="connect"]').click();
    await settle();
    assert.ok(h.root.textContent.includes("couldn't read All Mail over IMAP"), 'all_mail_hidden error');

    // Third attempt: succeeds and enters scanning state (with bodies_read = 0)
    h.root.querySelector('#email-app-password').input('abcd efgh ijkl mnop');
    h.root.querySelector('[data-email-action="connect"]').click();
    await settle();
    assert.ok(h.root.textContent.includes('scanning your mailbox headers'), 'scanning heading');
    assert.ok(h.root.textContent.includes('500 of 2,000 messages checked'), 'scan progress');
    assert.ok(h.root.textContent.includes('message bodies read'), '0 bodies read stat');
    assert.strictEqual(h.timers.size, 1, 'poll timer active while scanning');
    h.handle.dispose();
  });

  await test('choice-before-sync previews ranges/types, validates non-empty types, shows 3-unit estimate without money, and confirms into fetching', async () => {
    const h = harness(manifestDir, async ({ url, method, body }) => {
      if (url === '/app/sources/api/email' && method === 'GET') {
        return {
          status: 200,
          body: {
            source: 'email', state: 'setting_up', phase: 'awaiting_choice', provider: 'gmail', account: 'maya.chen@gmail.com',
            connected: true, syncing: false,
            scan: { complete: true, paused: false, checked: 10000, total: 10000, sent_found: 16443, eligible_sent: 16410, earliest_month: 'march 2011', bodies_read: 0, folder_cap_warning: 10000 },
            sync: { active: false, done: 0, total: 0 },
            accounting: { sent_total: 16443, in_journal: 0, removed_by_owner: 0, outside_choice: 16410, left_out: { total: 33, calendar_responses: 24, automatic_replies: 5, drafts: 3, delivery_reports: 1 } },
            runs: [],
          },
        };
      }
      if (url.startsWith('/app/sources/api/email/choice-preview') && method === 'GET') {
        const is1y = url.includes('range=1y');
        const isNone = url.includes('range=none');
        const emptyTypes = url.endsWith('types=');
        return {
          status: 200,
          body: {
            range: isNone ? 'none' : (is1y ? '1y' : '30d'),
            types: emptyTypes ? [] : ['replies', 'new_messages', 'forwards'],
            ranges: { '30d': 184, '6m': 912, '1y': 1740, all: 16410, none: 0 },
            range_deltas: { '30d': 184, '6m': 912, '1y': 1740, all: 16410, none: 0 },
            types_in_selected_range: { replies: 1042, new_messages: 386, forwards: 312 },
            selected_messages: emptyTypes || isNone ? 0 : (is1y ? 1740 : 184),
            delta_messages: emptyTypes || isNone ? 0 : (is1y ? 1740 : 184),
            estimate: emptyTypes || isNone ? { segments: 0, days: 0, thoughts: 0 } : (is1y ? { segments: 1510, days: 318, thoughts: 1660 } : { segments: 160, days: 24, thoughts: 176 }),
            delta_estimate: emptyTypes || isNone ? { segments: 0, days: 0, thoughts: 0 } : (is1y ? { segments: 1510, days: 318, thoughts: 1660 } : { segments: 160, days: 24, thoughts: 176 }),
            left_out: { total: 33, calendar_responses: 24, automatic_replies: 5, drafts: 3, delivery_reports: 1 },
            folder_cap_warning: 10000,
            earliest_month: 'march 2011',
          },
        };
      }
      if (url === '/app/sources/api/email/choice' && method === 'POST') {
        assert.strictEqual(body.range, '1y');
        assert.deepStrictEqual(body.types, ['replies', 'new_messages', 'forwards']);
        return {
          status: 200,
          body: {
            success: true,
            status: {
              source: 'email', state: 'on', phase: 'fetching', provider: 'gmail', account: 'maya.chen@gmail.com',
              connected: true, syncing: true,
              scan: { complete: true, paused: false, checked: 10000, total: 10000, sent_found: 16443, eligible_sent: 16410, earliest_month: 'march 2011', bodies_read: 0 },
              choice: { range: '1y', types: ['replies', 'new_messages', 'forwards'], covered_ranges: ['30d', '6m', '1y'], updated_at: iso(NOW) },
              sync: { active: true, done: 612, total: 1740 },
              accounting: { sent_total: 16443, in_journal: 612, removed_by_owner: 0, outside_choice: 15798, left_out: { total: 33, calendar_responses: 24, automatic_replies: 5, drafts: 3, delivery_reports: 1 } },
              runs: [],
            },
          },
        };
      }
      throw new Error('unexpected ' + method + ' ' + url);
    });
    await settle();

    assert.ok(h.root.textContent.includes('choose what comes in'));
    assert.ok(h.root.textContent.includes('IMAP folder limit'), 'folder cap advisory shown');
    assert.ok(h.root.textContent.includes('33 left out automatically'), 'left out fold');
    assert.ok(h.root.textContent.includes('~160 segments'), '30d estimate');
    assert.strictEqual(h.root.textContent.includes('$'), false, 'never mentions money');
    assert.strictEqual(h.root.querySelector('[data-email-action="confirm-choice"]').textContent, 'bring in 184 messages');

    // Select `none` -> zero state + `start from today`
    h.root.querySelector('[data-range="none"]').querySelector('input').change();
    await settle();
    assert.ok(h.root.textContent.includes('0 older segments · 0 past days · 0 thoughts to catch up'));
    assert.strictEqual(h.root.querySelector('[data-email-action="confirm-choice"]').textContent, 'start from today');

    // Untick all 3 types -> inline validation + disabled confirm
    ['replies', 'new_messages', 'forwards'].forEach((mt) => {
      h.root.querySelector(`[data-type="${mt}"]`).querySelector('input').change();
    });
    await settle();
    assert.ok(h.root.textContent.includes('choose at least one type of message.'));
    assert.strictEqual(h.root.querySelector('[data-email-action="confirm-choice"]').disabled, true);

    // Re-tick all 3 types + choose 1y -> confirms into fetching
    ['replies', 'new_messages', 'forwards'].forEach((mt) => {
      h.root.querySelector(`[data-type="${mt}"]`).querySelector('input').change();
    });
    h.root.querySelector('[data-range="1y"]').querySelector('input').change();
    await settle();
    assert.ok(h.root.textContent.includes('~1,510 segments'));
    assert.strictEqual(h.root.querySelector('[data-email-action="confirm-choice"]').textContent, 'bring in 1,740 messages');

    h.root.querySelector('[data-email-action="confirm-choice"]').click();
    await settle();
    assert.ok(h.root.textContent.includes('bringing in your chosen messages'));
    assert.ok(h.root.textContent.includes('612 of 1,740 messages in your journal'));
    const statusLink = h.root.querySelectorAll('a').find((a) => a.getAttribute('href') === '/app/health/');
    assert.notStrictEqual(statusLink, null, 'reuses /app/health/ status link');
    assert.strictEqual(h.root.textContent.includes('pause'), true, 'link mentions follow and pause on status');
    assert.strictEqual(h.root.querySelectorAll('button').length, 0, 'no second pause button on fetching screen');
    h.handle.dispose();
  });

  await test('steady state renders AC 5 facts, widen delta + untick note, per-run Trash modal + recover receipt, and disconnect modal', async () => {
    let removedByOwner = 0;
    let runs = [{ id: 'run_261125_01', synced_at: iso(NOW - 3 * 60 * MIN), range_label: '1y', messages: 1740, live_messages: 1740, live_segments: 1510 }];
    let inJournal = 1740;
    const h = harness(manifestDir, async ({ url, method, body }) => {
      if (url === '/app/sources/api/email' && method === 'GET') {
        return {
          status: 200,
          body: {
            source: 'email', state: 'on', phase: 'steady', provider: 'gmail', account: 'maya.chen@gmail.com',
            connected: true, syncing: false,
            scan: { complete: true, paused: false, checked: 287063, total: 287063, sent_found: 16443, eligible_sent: 16410, earliest_month: 'march 2011', bodies_read: 0 },
            choice: { range: '1y', types: ['replies', 'new_messages', 'forwards'], covered_ranges: ['30d', '6m', '1y'], updated_at: iso(NOW - 60 * MIN) },
            sync: { active: false, done: 1740, total: 1740, last_added_at: iso(NOW - 14 * MIN), last_checked_at: iso(NOW - 6 * MIN) },
            accounting: { sent_total: 16443, in_journal: inJournal, removed_by_owner: removedByOwner, outside_choice: 14670, left_out: { total: 33, calendar_responses: 24, automatic_replies: 5, drafts: 3, delivery_reports: 1 } },
            runs,
          },
        };
      }
      if (url.startsWith('/app/sources/api/email/choice-preview') && method === 'GET') {
        const isAll = url.includes('range=all');
        return {
          status: 200,
          body: {
            range: isAll ? 'all' : '1y',
            types: ['replies', 'new_messages', 'forwards'],
            ranges: { '30d': 184, '6m': 912, '1y': 1740, all: 16410, none: 0 },
            range_deltas: { '30d': 0, '6m': 0, '1y': 0, all: 14670, none: 0 },
            types_in_selected_range: { replies: 10000, new_messages: 4000, forwards: 2410 },
            selected_messages: isAll ? 16410 : 1740,
            delta_messages: isAll ? 14670 : 0,
            estimate: { segments: 14202, days: 4172, thoughts: 15600 },
            delta_estimate: isAll ? { segments: 12692, days: 3854, thoughts: 13940 } : { segments: 0, days: 0, thoughts: 0 },
            left_out: { total: 33, calendar_responses: 24, automatic_replies: 5, drafts: 3, delivery_reports: 1 },
            covered_ranges: ['30d', '6m', '1y'],
            earliest_month: 'march 2011',
          },
        };
      }
      if (url === '/app/sources/api/email/runs/run_261125_01/delete' && method === 'GET') {
        return { status: 200, body: { id: 'run_261125_01', synced_at: iso(NOW - 3 * 60 * MIN), range_label: '1y', messages: 1740, segments: 1510, erase_after_days: 7 } };
      }
      if (url === '/app/sources/api/email/runs/run_261125_01' && method === 'DELETE') {
        assert.deepStrictEqual(body, { erase: false, backups: false });
        inJournal = 0;
        removedByOwner = 1740;
        runs = [];
        return { status: 200, body: { success: true, id: 'run_261125_01', moved_messages: 1740, moved_segments: 1510, trash_ids: ['tr_1', 'tr_2'] } };
      }
      if (url === '/app/settings/api/trash/recover' && method === 'POST') {
        assert.deepStrictEqual(body, { ids: ['tr_1', 'tr_2'] });
        inJournal = 1740;
        removedByOwner = 0;
        return { status: 200, body: { done: ['tr_1', 'tr_2'], failed: [] } };
      }
      if (url === '/app/sources/api/email' && method === 'DELETE') {
        return {
          status: 200,
          body: {
            success: true,
            status: {
              source: 'email', state: 'off', phase: 'disconnected', provider: 'gmail', account: 'maya.chen@gmail.com',
              connected: false, syncing: false,
              scan: { complete: true, paused: false, checked: 287063, total: 287063, sent_found: 16443, eligible_sent: 16410, earliest_month: 'march 2011', bodies_read: 0 },
              sync: { active: false, done: 0, total: 0 },
              accounting: { sent_total: 16443, in_journal: 1740, removed_by_owner: 0, outside_choice: 14670, left_out: { total: 33, calendar_responses: 24, automatic_replies: 5, drafts: 3, delivery_reports: 1 } },
              runs: [],
            },
          },
        };
      }
      throw new Error('unexpected ' + method + ' ' + url);
    });
    await settle();

    // Steady facts block: deleted by you hidden when 0
    assert.strictEqual(h.root.querySelector('[data-fact="deleted"]'), null, 'deleted by you hidden at 0');
    assert.ok(h.root.textContent.includes('16,443'), 'sent total');
    assert.ok(h.root.textContent.includes('1,740'), 'in journal');
    assert.ok(h.root.textContent.includes('14,670'), 'outside choice');
    assert.ok(h.root.textContent.includes('to move an email to trash, open it in transcripts and delete it.'));

    // Open widen mode -> covered ranges + delta + untick note
    h.root.querySelector('[data-email-action="open-widen"]').click();
    await settle();
    assert.ok(h.root.querySelector('[data-range="1y"]').textContent.includes('already brought in'));
    assert.ok(h.root.querySelector('[data-range="all"]').textContent.includes('+14,670 more messages'));
    assert.strictEqual(h.root.querySelector('[data-email-action="confirm-choice"]').textContent, 'save choice');

    h.root.querySelector('[data-range="all"]').querySelector('input').change();
    await settle();
    assert.ok(h.root.textContent.includes('~12,692 new segments'));
    assert.strictEqual(h.root.querySelector('[data-email-action="confirm-choice"]').textContent, 'bring in 14,670 more messages');

    h.root.querySelector('[data-type="forwards"]').querySelector('input').change();
    await settle();
    assert.notStrictEqual(h.root.querySelector('[data-untick-note]'), null, 'untick future-only note shown');

    // Cancel back to steady
    h.root.querySelector('[data-email-action="cancel-widen"]').click();
    await settle();

    // Past syncs -> move to trash modal -> inline receipt + deleted by you > 0 -> recover
    h.root.querySelector('[data-trash-run="run_261125_01"]').click();
    await settle();
    assert.ok(h.root.textContent.includes("move this sync's messages to trash?"));
    h.root.querySelector('[data-modal-confirm]').click();
    await settle();
    assert.ok(h.root.textContent.includes('moved 1,740 messages to trash.'));
    assert.notStrictEqual(h.root.querySelector('[data-fact="deleted"]'), null, 'deleted by you shown when > 0');

    h.root.querySelector('[data-email-action="recover-run"]').click();
    await settle();
    assert.ok(h.root.textContent.includes('recovered messages from trash.'));

    // Disconnect modal -> disconnected state -> reconnect prefills account
    h.root.querySelector('[data-email-action="disconnect"]').click();
    assert.ok(h.root.textContent.includes('disconnect Gmail?'));
    h.root.querySelector('[data-modal-confirm]').click();
    await settle();
    assert.ok(h.root.textContent.includes('Gmail is disconnected. 1,740 messages already in your journal stay.'));
    h.root.querySelector('[data-email-action="reconnect"]').click();
    assert.strictEqual(h.root.querySelector('#email-address').value, 'maya.chen@gmail.com');
    h.handle.dispose();
  });

  await test('needs_attention splits password_rejected (inline password form, no check now) from all_mail_hidden (try again, no password prompt)', async () => {
    // 1. password_rejected
    const pw = harness(manifestDir, async ({ url, method, body }) => {
      if (url === '/app/sources/api/email' && method === 'GET') {
        return {
          status: 200,
          body: { source: 'email', state: 'needs_attention', phase: 'needs_attention', provider: 'gmail', account: 'maya.chen@gmail.com', attention_reason: 'password_rejected', connected: true, syncing: false },
        };
      }
      if (url === '/app/sources/api/email/password' && method === 'PUT') {
        assert.strictEqual(body.app_password, 'newp assw ord1 2345');
        return {
          status: 200,
          body: {
            success: true,
            status: { source: 'email', state: 'on', phase: 'steady', provider: 'gmail', account: 'maya.chen@gmail.com', connected: true, syncing: false, scan: { complete: true }, sync: { active: false }, accounting: { sent_total: 10, in_journal: 10, removed_by_owner: 0, outside_choice: 0, left_out: { total: 0 } }, runs: [] },
          },
        };
      }
      throw new Error('unexpected ' + method + ' ' + url);
    });
    await settle();
    assert.ok(pw.root.textContent.includes('changing your Google password revokes app passwords automatically'));
    assert.strictEqual(pw.root.querySelector('[data-email-action="check-now"]'), null, 'check now hidden on password_rejected');
    assert.notStrictEqual(pw.root.querySelector('#email-new-password'), null, 'password field auto-opened');
    pw.root.querySelector('#email-new-password').input('newp assw ord1 2345');
    pw.root.querySelector('[data-email-action="save-password"]').click();
    await settle();
    assert.ok(pw.root.textContent.includes('new app password saved.'));
    pw.handle.dispose();

    // 2. all_mail_hidden
    let retried = false;
    const imap = harness(manifestDir, async ({ url, method }) => {
      if (url === '/app/sources/api/email' && method === 'GET') {
        return {
          status: 200,
          body: { source: 'email', state: 'needs_attention', phase: 'needs_attention', provider: 'gmail', account: 'maya.chen@gmail.com', attention_reason: 'all_mail_hidden', connected: true, syncing: false },
        };
      }
      if (url === '/app/sources/api/email/sync' && method === 'POST') {
        retried = true;
        return {
          status: 200,
          body: {
            success: true,
            status: { source: 'email', state: 'on', phase: 'steady', provider: 'gmail', account: 'maya.chen@gmail.com', connected: true, syncing: false, scan: { complete: true }, sync: { active: false }, accounting: { sent_total: 10, in_journal: 10, removed_by_owner: 0, outside_choice: 0, left_out: { total: 0 } }, runs: [] },
          },
        };
      }
      throw new Error('unexpected ' + method + ' ' + url);
    });
    await settle();
    assert.ok(imap.root.textContent.includes("couldn't read All Mail over IMAP"));
    assert.strictEqual(imap.root.querySelector('#email-new-password'), null, 'no password prompt for IMAP block');
    imap.root.querySelector('[data-email-action="retry-sync"]').click();
    await settle();
    assert.strictEqual(retried, true, 'try again triggers sync');
    imap.handle.dispose();
  });

  return cases;
};
