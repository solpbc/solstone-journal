// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// End-to-end DOM tests for `/app/sources/photos` running on the shared
// selective-sync frame (`SolSelectiveSync`) under plain Node.
// Asserts state transitions, attributes, API requests, and i18n keys
// without pinning copy prose or styling tokens.

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
    error.message = `photos_source_dom: ${name}: ${error.message}`;
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
    if (i !== -1) { this.childNodes.splice(i, 1); node.parentNode = null; }
    return node;
  }
  click() {
    if (this.disabled) return;
    this.fire({ type: 'click', target: this, preventDefault() {} });
  }
  focus() { this.ownerDocument.activeElement = this; }
  querySelectorAll(sel) {
    const groups = sel.split(',').map((s) => s.trim()).filter(Boolean);
    const out = [];
    const walk = (n) => {
      for (const c of n.childNodes) {
        if (c.nodeType !== 1) continue;
        if (groups.some((g) => {
          const parts = g.split(/\s+/);
          if (!simpleMatch(c, parts[parts.length - 1])) return false;
          let p = c.parentNode;
          for (let i = parts.length - 2; i >= 0; i -= 1) {
            while (p && p.nodeType === 1 && !simpleMatch(p, parts[i])) p = p.parentNode;
            if (!p || p.nodeType !== 1) return false;
            p = p.parentNode;
          }
          return true;
        })) out.push(c);
        walk(c);
      }
    };
    walk(this);
    return out;
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
}

class Document extends Target {
  constructor() {
    super();
    this.body = new Element(this, 'body');
    this.activeElement = this.body;
  }
  createElement(tag) { return new Element(this, tag); }
}

function flush() { return new Promise((r) => setImmediate(r)); }
async function settle() { for (let i = 0; i < 10; i += 1) await flush(); }

function makeHarness(manifestDir, routes) {
  const doc = new Document();
  const root = doc.createElement('main');
  root.setAttribute('id', 'sources-root');
  doc.body.appendChild(root);

  const requests = [];
  const timers = new Map();
  let nextTimer = 1;
  const listeners = [];
  const readyStates = [];

  const ctx = {
    disposed: false,
    timeZone: ZONE,
    now: () => NOW,
    ready: (s) => readyStates.push(s),
    setTimeout(fn, ms) {
      const id = nextTimer++;
      timers.set(id, { fn, ms });
      return id;
    },
    clearTimeout(id) { timers.delete(id); },
    on(target, type, fn) {
      target.addEventListener(type, fn);
      const entry = { target, type, fn };
      listeners.push(entry);
      return () => target.removeEventListener(type, fn);
    },
    async fetch(url, init) {
      const method = (init && init.method) || 'GET';
      const body = init && init.body ? JSON.parse(init.body) : null;
      requests.push({ method, url, body });
      const handler = routes[`${method} ${url}`] || routes[`${method} ${url.split('?')[0]}`];
      if (!handler) return { ok: false, status: 404, json: async () => ({ code: 'not_found' }) };
      const out = typeof handler === 'function' ? await handler(body, url) : handler;
      const status = out.status || 200;
      return { ok: status >= 200 && status < 300, status, json: async () => out.body };
    },
  };

  const sandbox = journalContext({
    document: doc,
    window: null,
    Intl,
    Date,
    Number,
    Math,
    Array,
    Object,
    String,
    Boolean,
    Map,
    Set,
    Promise,
    JSON,
    encodeURIComponent,
    decodeURIComponent,
  });
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const frameSrc = fs.readFileSync(path.join(manifestDir, 'assets/sources/selective_sync_frame.js'), 'utf8');
  const photosSrc = fs.readFileSync(path.join(manifestDir, 'assets/sources/photos_source.js'), 'utf8');
  vm.runInContext(frameSrc, sandbox, { filename: 'selective_sync_frame.js' });
  vm.runInContext(photosSrc, sandbox, { filename: 'photos_source.js' });

  const frame = sandbox.SolSelectiveSync;
  const detail = frame.getDetail('photos');
  const dispose = detail.mount(ctx, root, frame);

  return {
    doc,
    root,
    ctx,
    requests,
    timers,
    readyStates,
    i18n: sandbox.JournalI18n,
    dispose() {
      ctx.disposed = true;
      if (typeof dispose === 'function') dispose();
      listeners.forEach((l) => l.target.removeEventListener(l.type, l.fn));
    },
  };
}

function sampleRanges({ coveredUpTo = null } = {}) {
  const isCovered = (k) => {
    if (!coveredUpTo) return false;
    const order = ['30d', '6m', '1y', 'all'];
    return order.indexOf(k) !== -1 && order.indexOf(k) <= order.indexOf(coveredUpTo);
  };
  return {
    '30d': {
      photos: 42, days: 11, thoughts: 42,
      delta_photos: isCovered('30d') ? 0 : 42, delta_days: isCovered('30d') ? 0 : 11, delta_thoughts: isCovered('30d') ? 0 : 42,
      covered: isCovered('30d'), earliest_date: '2026-10-27',
      left_out: { total: 18, reasons: { near_duplicates: 9, videos: 5, screenshots: 4 } },
    },
    '6m': {
      photos: 140, days: 48, thoughts: 140,
      delta_photos: isCovered('6m') ? 0 : 98, delta_days: isCovered('6m') ? 0 : 37, delta_thoughts: isCovered('6m') ? 0 : 98,
      covered: isCovered('6m'), earliest_date: '2026-05-26',
      left_out: { total: 64, reasons: { near_duplicates: 30, videos: 16, saved_and_shared: 10, screenshots: 8 } },
    },
    '1y': {
      photos: 240, days: 85, thoughts: 240,
      delta_photos: isCovered('1y') ? 0 : 100, delta_days: isCovered('1y') ? 0 : 37, delta_thoughts: isCovered('1y') ? 0 : 100,
      covered: isCovered('1y'), earliest_date: '2025-11-26',
      left_out: { total: 115, reasons: { near_duplicates: 52, videos: 25, saved_and_shared: 18, busy_moments: 12, screenshots: 8 } },
    },
    'all': {
      photos: 920, days: 310, thoughts: 920,
      delta_photos: isCovered('all') ? 0 : 680, delta_days: isCovered('all') ? 0 : 225, delta_thoughts: isCovered('all') ? 0 : 680,
      covered: isCovered('all'), earliest_date: '2021-03-10',
      left_out: { total: 430, reasons: { near_duplicates: 200, videos: 90, saved_and_shared: 70, busy_moments: 40, screenshots: 30 } },
    },
    'none': {
      photos: 0, days: 0, thoughts: 0, delta_photos: 0, delta_days: 0, delta_thoughts: 0,
      covered: false, earliest_date: null, left_out: { total: 0, reasons: {} },
    },
  };
}

function baseState(overrides = {}) {
  return Object.assign({
    source: 'photos',
    state: 'setting_up',
    substate: 'choose',
    attention_reason: null,
    devices: [{
      cid: 'cid-phone-1',
      name: 'Test Phone',
      kind: 'iphone',
      connected: true,
      access_state: 'authorized',
      last_scan_at: new Date(NOW - 12 * MIN).toISOString(),
      last_upload_at: new Date(NOW - 15 * MIN).toISOString(),
      icloud_waiting_count: 0,
    }],
    scan: {
      items_checked: 4500,
      items_total: 4500,
      earliest_item_at: '2021-03-10T00:00:00Z',
      photos_brought_in_so_far: 0,
      paused: false,
    },
    choice: { range: null, range_start_at: null, can_widen: true },
    ranges: sampleRanges(),
    left_out: { total: 115, reasons: { near_duplicates: 52, videos: 25, saved_and_shared: 18, busy_moments: 12, screenshots: 8 } },
    progress: { chosen_total: 0, arrived: 0, in_journal: 0, waiting_on_phone: 0, moved_to_trash_by_owner: 0 },
    band4_paused: false,
    thinking_ready: true,
    profile_pics_enabled: true,
  }, overrides);
}

module.exports = async function photosSourceDomCases(manifestDir) {
  await test('choose state preselects 1y, switches ranges live, and posts choose-range', async () => {
    let posted = null;
    const h = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': { status: 200, body: baseState({ thinking_ready: false }) },
      'POST /app/sources/photos/api/choose-range': (body) => {
        posted = body;
        return {
          status: 200,
          body: baseState({
            state: 'on',
            substate: 'bringing_in',
            choice: { range: body.range, range_start_at: '2026-05-26T00:00:00Z', can_widen: true },
            progress: { chosen_total: 140, arrived: 10, in_journal: 10, waiting_on_phone: 130, moved_to_trash_by_owner: 0 },
          }),
        };
      },
    });
    await settle();

    assert.strictEqual(h.root.querySelector('.ss-pill').getAttribute('data-state'), 'setting_up');
    const selectedRadio = h.root.querySelector('input[type="radio"][checked="checked"]');
    assert.strictEqual(selectedRadio.getAttribute('value'), '1y', 'the last year is preselected');
    assert.strictEqual(h.root.querySelectorAll('.ss-est-unit').length, 3, 'three-unit non-money estimate');
    assert.ok(h.root.textContent.includes(h.i18n.render('journal.window.sources.photos.estimate.no.thinking')), 'shows no-thinking notice');

    // Switch to `6m` and confirm
    const sixMonth = h.root.querySelector('[data-range="6m"] input');
    sixMonth.checked = true;
    sixMonth.fire({ type: 'change', target: sixMonth });
    const confirmBtn = h.root.querySelector('[data-action="choose-confirm"]');
    confirmBtn.click();
    await settle();

    assert.deepStrictEqual(posted, { range: '6m' });
    assert.strictEqual(h.root.querySelector('[data-photos-substate]').getAttribute('data-photos-substate'), 'bringing_in');
    assert.strictEqual(h.timers.size, 1, 'schedules poll while bringing_in');
    h.dispose();
  });

  await test('bringing_in shows status link flipping on band4_paused, offline/iCloud notices, and pause/resume', async () => {
    let current = baseState({
      state: 'on',
      substate: 'bringing_in',
      devices: [{
        cid: 'cid-phone-1', name: 'Test Phone', kind: 'iphone',
        connected: false, access_state: 'authorized', icloud_waiting_count: 3,
      }],
      choice: { range: '1y', range_start_at: '2025-11-26T00:00:00Z', can_widen: true },
      progress: { chosen_total: 240, arrived: 80, in_journal: 78, waiting_on_phone: 160, moved_to_trash_by_owner: 0 },
      band4_paused: true,
    });
    const h = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': () => ({ status: 200, body: current }),
      'POST /app/sources/photos/api/pause': () => {
        current = Object.assign({}, current, { state: 'paused', substate: 'paused' });
        return { status: 200, body: current };
      },
      'POST /app/sources/photos/api/resume': () => {
        current = Object.assign({}, current, { state: 'on', substate: 'bringing_in' });
        return { status: 200, body: current };
      },
      'GET /app/sources/photos/api/photos': { status: 200, body: { months: [], items: [], next_cursor: null } },
    });
    await settle();

    assert.ok(h.root.querySelector('a[href="/app/health/"]') !== null, 'links to /app/health/');
    assert.ok(h.root.textContent.includes(h.i18n.render('journal.window.sources.photos.bringing.status.paused.link')), 'paused Band 4 link');
    assert.ok(h.root.querySelector('.ss-notice.warn') !== null, 'offline notice shown');

    h.root.querySelector('[data-action="pause"]').click();
    await settle();
    assert.strictEqual(h.root.querySelector('.ss-pill').getAttribute('data-state'), 'paused');

    h.root.querySelector('[data-action="resume"]').click();
    await settle();
    assert.strictEqual(h.root.querySelector('.ss-pill').getAttribute('data-state'), 'on');
    h.dispose();
  });

  await test('steady state widens range with covered presets disabled and trashes chosen set only (never select-all)', async () => {
    let widenPosted = null;
    let trashPosted = null;
    let current = baseState({
      state: 'on',
      substate: 'steady',
      choice: { range: '1y', range_start_at: '2025-11-26T00:00:00Z', can_widen: true },
      ranges: sampleRanges({ coveredUpTo: '1y' }),
      left_out: { total: 120, reasons: { near_duplicates: 52, videos: 25, saved_and_shared: 18, busy_moments: 12, screenshots: 8, noise: 5 } },
      progress: { chosen_total: 240, arrived: 240, in_journal: 235, waiting_on_phone: 0, moved_to_trash_by_owner: 2 },
    });
    const photoPage = {
      months: ['2026-11', '2026-10'],
      items: [
        { day: '20261120', stream: 'photos', segment: '091500_0', captured_at: '2026-11-20T09:15:00Z', thumb_url: '/app/transcripts/api/media/20261120/photos/091500_0/original.jpg', caption: 'coffee cup', has_location: false, device_name: 'Test Phone' },
        { day: '20261014', stream: 'photos', segment: '140000_0', captured_at: '2026-10-14T14:00:00Z', thumb_url: '/app/transcripts/api/media/20261014/photos/140000_0/original.jpg', caption: 'trail sign', has_location: true, device_name: 'Test Phone' },
      ],
      next_cursor: null,
    };

    const h = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': () => ({ status: 200, body: current }),
      'GET /app/sources/photos/api/photos': () => ({ status: 200, body: photoPage }),
      'POST /app/sources/photos/api/widen-range': (body) => {
        widenPosted = body;
        return { status: 200, body: current };
      },
      'POST /app/sources/photos/api/trash': (body) => {
        trashPosted = body;
        photoPage.items = photoPage.items.slice(1);
        current.progress.in_journal -= 1;
        current.progress.moved_to_trash_by_owner += 1;
        return { status: 200, body: { moved: body.segments.length, failed: 0, trash_ids: ['tr-1'] } };
      },
    });
    await settle();

    // Facts & chosen-set picker rendered; strictly no select-all button
    assert.ok(h.root.querySelector('[data-fact="trashed"]') !== null, 'trashed fact shown when > 0');
    assert.strictEqual(h.root.querySelectorAll('[data-photo-checkbox]').length, 2, 'two individual checkboxes');
    assert.strictEqual(h.root.querySelector('[data-action="select-all"]'), null, 'no select-all control');

    // Select one photo and trash via confirmation modal
    const chk = h.root.querySelector('[data-photo-checkbox="20261120/photos/091500_0"]');
    chk.checked = true;
    chk.fire({ type: 'change', target: chk });
    h.root.querySelector('[data-action="picker-trash"]').click();
    const modal = h.root.querySelector('[data-modal="trash-chosen-photos"]');
    assert.notStrictEqual(modal, null, 'trash confirm modal opened');
    modal.querySelector('[data-modal-confirm="true"]').click();
    await settle();

    assert.deepStrictEqual(trashPosted, { segments: [{ day: '20261120', stream: 'photos', segment: '091500_0' }] });
    assert.ok(h.root.querySelector('a[href="/app/settings/#trash"]') !== null, 'receipt links to trash');

    // Open widen panel -> covered ranges disabled, `all` selectable
    h.root.querySelector('[data-action="open-widen"]').click();
    const widenPanel = h.root.querySelector('[data-widen-panel="true"]');
    assert.notStrictEqual(widenPanel, null, 'widen panel opened');
    assert.strictEqual(widenPanel.querySelector('[data-range="1y"] input').disabled, true, 'covered range disabled');
    assert.strictEqual(widenPanel.querySelector('[data-range="all"] input').disabled, false, 'wider range enabled');
    widenPanel.querySelector('[data-action="widen-confirm"]').click();
    await settle();
    assert.deepStrictEqual(widenPosted, { range: 'all' });
    h.dispose();
  });

  await test('turn off confirms in modal and transitions cleanly to off state', async () => {
    const h = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': { status: 200, body: baseState({ state: 'needs_attention', substate: 'album_contract_error', attention_reason: 'smart_album_contract_failed', progress: { chosen_total: 10, arrived: 10, in_journal: 10, waiting_on_phone: 0, moved_to_trash_by_owner: 0 } }) },
      'GET /app/sources/photos/api/photos': { status: 200, body: { months: [], items: [], next_cursor: null } },
      'POST /app/sources/photos/api/turn-off': {
        status: 200,
        body: baseState({ state: 'off', substate: 'off', devices: [], progress: { chosen_total: 0, arrived: 0, in_journal: 10, waiting_on_phone: 0, moved_to_trash_by_owner: 0 } }),
      },
    });
    await settle();

    assert.strictEqual(h.root.querySelector('.ss-pill').getAttribute('data-state'), 'needs_attention');
    h.root.querySelector('[data-action="turn-off"]').click();
    const modal = h.root.querySelector('[data-modal="turn-off-photos"]');
    assert.notStrictEqual(modal, null, 'turn-off modal opened');
    modal.querySelector('[data-modal-confirm="true"]').click();
    await settle();

    assert.strictEqual(h.root.querySelector('.ss-pill').getAttribute('data-state'), 'off');
    assert.ok(h.root.textContent.includes(h.i18n.render('journal.window.sources.photos.off.reentry.nodevice')), 'falls back cleanly when devices list is empty after turn-off');
    h.dispose();
  });

  await test('scanning distinguishes initial setup from second-device rescan', async () => {
    const h1 = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': {
        status: 200,
        body: baseState({
          state: 'setting_up',
          substate: 'scanning',
          choice: { range: null, range_start_at: null, can_widen: true },
          scan: { items_checked: 200, items_total: 1000, earliest_item_at: '2023-04-01T00:00:00Z', photos_brought_in_so_far: 0, paused: false },
        }),
      },
    });
    await settle();
    assert.ok(h1.root.textContent.includes(h1.i18n.render('journal.window.sources.photos.scan.note')));
    h1.dispose();

    const h2 = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': {
        status: 200,
        body: baseState({
          state: 'setting_up',
          substate: 'scanning',
          choice: { range: '1y', range_start_at: '2025-11-26T00:00:00Z', can_widen: true },
          scan: { items_checked: 200, items_total: 1000, earliest_item_at: '2023-04-01T00:00:00Z', photos_brought_in_so_far: 40, paused: false },
        }),
      },
    });
    await settle();
    assert.ok(h2.root.textContent.includes(h2.i18n.render('journal.window.sources.photos.scan.note.rescan')));
    assert.strictEqual(h2.root.textContent.includes(h2.i18n.render('journal.window.sources.photos.scan.note')), false);
    h2.dispose();
  });

  await test('restricted access uses non-Settings notice, thinking-off uses no-thinking notice, and zero-total scan_paused uses empty paused key', async () => {
    const hRestricted = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': {
        status: 200,
        body: baseState({
          state: 'needs_attention',
          substate: 'access_off',
          attention_reason: 'access_off',
          devices: [{
            cid: 'cid-1',
            name: 'Sample iPad',
            kind: 'ipad',
            connected: true,
            access_state: 'restricted',
            last_scan_at: null,
            last_upload_at: null,
            icloud_waiting_count: 0,
          }],
        }),
      },
    });
    await settle();
    assert.ok(hRestricted.root.textContent.includes(
      hRestricted.i18n.render('journal.window.sources.photos.attention.restricted', [['device', 'Sample iPad']])
    ));
    hRestricted.dispose();

    const hThinkingOff = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': {
        status: 200,
        body: baseState({
          state: 'on',
          substate: 'bringing_in',
          thinking_ready: false,
          band4_paused: true,
          progress: { chosen_total: 10, arrived: 2, in_journal: 2, waiting_on_phone: 8, moved_to_trash_by_owner: 0 },
        }),
      },
    });
    await settle();
    assert.ok(hThinkingOff.root.textContent.includes(
      hThinkingOff.i18n.render('journal.window.sources.photos.estimate.no.thinking')
    ));
    hThinkingOff.dispose();

    const hScanPausedZero = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': {
        status: 200,
        body: baseState({
          state: 'setting_up',
          substate: 'scan_paused',
          scan: { items_checked: 0, items_total: 0, earliest_item_at: null, photos_brought_in_so_far: 0, paused: true },
        }),
      },
    });
    await settle();
    assert.ok(hScanPausedZero.root.textContent.includes(
      hScanPausedZero.i18n.render('journal.window.sources.photos.scan.paused.empty', [['device', 'Test Phone']])
    ));
    hScanPausedZero.dispose();

    // Multi-device attention names only the affected device, not authorized siblings.
    const hMultiAttn = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': {
        status: 200,
        body: baseState({
          state: 'needs_attention',
          substate: 'access_limited',
          attention_reason: 'access_limited',
          devices: [
            { cid: 'cid-ok', name: 'Healthy Phone', kind: 'iphone', connected: true, access_state: 'authorized', last_scan_at: null, last_upload_at: null, icloud_waiting_count: 0 },
            { cid: 'cid-bad', name: 'Limited Pad', kind: 'ipad', connected: true, access_state: 'limited', last_scan_at: null, last_upload_at: null, icloud_waiting_count: 0 },
          ],
        }),
      },
    });
    await settle();
    assert.ok(hMultiAttn.root.textContent.includes(
      hMultiAttn.i18n.render('journal.window.sources.photos.attention.access', [['device', 'Limited Pad']])
    ));
    assert.strictEqual(hMultiAttn.root.textContent.includes('Healthy Phone, Limited Pad is'), false);
    hMultiAttn.dispose();

    // Re-enable scan with existing journal photos preserves existing count rather than 0 (details only).
    const hReenableScan = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': {
        status: 200,
        body: baseState({
          state: 'setting_up',
          substate: 'scanning',
          choice: { range: null, range_start_at: null, can_widen: true },
          scan: { items_checked: 100, items_total: 500, earliest_item_at: null, photos_brought_in_so_far: 0, paused: false },
          progress: { chosen_total: 0, arrived: 0, in_journal: 50, waiting_on_phone: 0, moved_to_trash_by_owner: 0 },
        }),
      },
    });
    await settle();
    assert.strictEqual(
      hReenableScan.root.textContent.includes(hReenableScan.i18n.render('journal.window.sources.photos.scan.stat.brought.zero')),
      false
    );
    hReenableScan.dispose();
  });

  await test('404 disabled state renders unavailable fallback without crashing', async () => {
    const h = makeHarness(manifestDir, {
      'GET /app/sources/photos/api/state': { status: 404, body: { code: 'photos_library_disabled' } },
    });
    await settle();
    assert.strictEqual(h.root.querySelector('.ss-pill').getAttribute('data-state'), 'unavailable');
    assert.deepStrictEqual(h.readyStates, ['empty']);
    h.dispose();
  });

  return cases;
};
