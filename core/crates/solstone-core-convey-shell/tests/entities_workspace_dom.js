// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

let cases = 0;
function check(condition, message) {
  cases += 1;
  assert.ok(condition, message);
}

class ClassList {
  constructor() {
    this.values = new Set();
  }

  add(...names) {
    names.forEach((name) => this.values.add(name));
  }

  remove(...names) {
    names.forEach((name) => this.values.delete(name));
  }

  contains(name) {
    return this.values.has(name);
  }
}

class Style {
  constructor() {
    this.values = new Map();
  }

  setProperty(name, value) {
    this.values.set(name, value);
  }

  removeProperty(name) {
    this.values.delete(name);
  }

  getPropertyValue(name) {
    return this.values.get(name) || '';
  }
}

class Element {
  constructor(document, id = '') {
    this.ownerDocument = document;
    this.id = id;
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.attributes = {};
    this.classList = new ClassList();
    this.style = new Style();
    this.hidden = false;
    this.tabIndex = 0;
    this.textContent = '';
    this.className = '';
    this.type = '';
  }

  get childNodes() {
    return this.children;
  }

  append(...children) {
    children.forEach((child) => this.appendChild(child));
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  remove() {
    if (!this.parentElement) return;
    this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
    this.parentElement = null;
  }

  contains(node) {
    return node === this || this.children.some((child) => child.contains(node));
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }

  getAttribute(name) {
    return Object.hasOwn(this.attributes, name) ? this.attributes[name] : null;
  }

  addEventListener(name, listener) {
    (this.listeners[name] ||= []).push(listener);
  }

  emit(name, event = {}) {
    for (const listener of this.listeners[name] || []) {
      listener({target: this, preventDefault() {}, stopPropagation() {}, ...event});
    }
  }

  focus() {
    this.ownerDocument.activeElement = this;
  }

  matches(selector) {
    if (selector.startsWith('#')) return this.id === selector.slice(1);
    if (selector === '[role="option"]') return this.attributes.role === 'option';
    return false;
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }

  querySelectorAll(selector) {
    const matches = [];
    const visit = (node) => {
      node.children.forEach((child) => {
        if (child.matches(selector)) matches.push(child);
        visit(child);
      });
    };
    visit(this);
    return matches;
  }
}

function setLocation(window, value) {
  const [beforeHash, hash = ''] = String(value).split('#', 2);
  const [pathname, search = ''] = beforeHash.split('?', 2);
  window.location.pathname = pathname || '/app/entities';
  window.location.search = search ? `?${search}` : '';
  window.location.hash = hash ? `#${hash}` : '';
}

function findById(root, id) {
  if (root.id === id) return root;
  for (const child of root.children) {
    const found = findById(child, id);
    if (found) return found;
  }
  return null;
}

// Deleting entities: the pane's button, the dialog, select mode and the
// receipts. Asserts which copy key reaches which element and which state
// renders; no assertion pins the words.
class PickNode {
  constructor(doc, tag) {
    this.ownerDocument = doc;
    this.tagName = String(tag).toUpperCase();
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.attributes = {};
    this.dataset = {};
    this.style = {};
    this.hidden = false;
    this.disabled = false;
    this.checked = false;
    this.value = '';
    this._text = '';
    this._class = '';
    const node = this;
    this.classList = {
      toggle(name, force) {
        const set = new Set(node._class.split(/\s+/).filter(Boolean));
        const on = force === undefined ? !set.has(name) : Boolean(force);
        if (on) set.add(name); else set.delete(name);
        node._class = [...set].join(' ');
        return on;
      },
      contains(name) { return node._class.split(/\s+/).includes(name); },
      add(name) { this.toggle(name, true); },
      remove(name) { this.toggle(name, false); },
    };
  }
  get className() { return this._class; }
  set className(value) { this._class = String(value || ''); }
  get id() { return this.attributes.id || ''; }
  set id(value) { this.attributes.id = String(value); }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return this._text + this.children.map((child) => child.textContent).join(''); }
  set innerHTML(value) { this._text = ''; this.children = []; this.html = String(value); }
  get innerHTML() { return this.html || ''; }
  appendChild(child) { child.parentElement = this; this.children.push(child); return child; }
  append(...nodes) { nodes.forEach((node) => this.appendChild(node)); }
  replaceChildren(...nodes) { this.children = []; this._text = ''; this.append(...nodes); }
  remove() { if (this.parentElement) this.parentElement.children = this.parentElement.children.filter((c) => c !== this); this.parentElement = null; }
  contains(node) { return node === this || this.children.some((child) => child.contains(node)); }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return Object.hasOwn(this.attributes, name) ? this.attributes[name] : null; }
  removeAttribute(name) { delete this.attributes[name]; }
  addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); }
  fire(name, extra = {}) {
    const event = { target: this, currentTarget: this, key: extra.key, preventDefault() {}, stopPropagation() {}, ...extra };
    if (typeof this['on' + name] === 'function') this['on' + name](event);
    (this.listeners[name] || []).forEach((fn) => fn(event));
    return event;
  }
  focus() { this.ownerDocument.activeElement = this; }
  scrollIntoView() {}
  walk(fn) { this.children.forEach((child) => { fn(child); child.walk(fn); }); }
  matches(selector) {
    if (selector === 'input[data-entity-pick]') return this.tagName === 'INPUT' && this.dataset.entityPick === 'true';
    if (selector.startsWith('.')) return this.classList.contains(selector.slice(1));
    return false;
  }
  querySelector(selector) { let found = null; this.walk((node) => { if (!found && node.matches(selector)) found = node; }); return found; }
  querySelectorAll(selector) { const out = []; this.walk((node) => { if (node.matches(selector)) out.push(node); }); return out; }
  all(cls) { const out = []; this.walk((node) => { if (node.classList.contains(cls)) out.push(node); }); return out; }
}

function pickHarness() {
  const doc = { activeElement: null, listeners: {}, registry: new Map() };
  doc.createElement = (tag) => new PickNode(doc, tag);
  doc.body = new PickNode(doc, 'body');
  doc.getElementById = (id) => doc.registry.get(id) || null;
  doc.contains = () => true;
  doc.addEventListener = (name, fn) => { (doc.listeners[name] ||= []).push(fn); };
  const make = (id, tag = 'div') => { const node = new PickNode(doc, tag); node.id = id; doc.registry.set(id, node); return node; };
  [
    'entityDeleteModal', 'entityDeleteModalHeader', 'entityDeleteModalTitle', 'entityDeleteModalBody',
    'entityDeleteModalMoveFoot', 'entityDeleteModalEraseFoot', 'entityDeleteModalClose', 'entityDeleteModalErase',
    'entityDeleteModalCancel', 'entityDeleteModalConfirm', 'entityDeleteModalBack', 'entityDeleteModalEraseConfirm',
    'journal-detail-delete-btn', 'journal-detail-delete-help', 'journal-detail-delete-help-name', 'journal-detail-delete-help-text',
    'journal-select-btn', 'journal-pick-bar', 'journal-pick-idle', 'journal-pick-some', 'journal-pick-all', 'journal-pick-all-label',
    'journal-pick-count', 'journal-pick-delete', 'journal-pick-done-idle', 'journal-pick-done-some',
    'journal-entity-cards-content', 'no-journal-entities', 'journal-card-search-count', 'no-journal-card-matches',
    'journal-list-panel', 'journal-ids-panel', 'journal-dups-panel', 'journal-entity-card-search', 'journal-list-pager', 'journal-folders',
  ].forEach((id) => make(id, id.endsWith('-btn') || /Modal(Close|Erase|Cancel|Confirm|Back|EraseConfirm)$/.test(id) || id.startsWith('journal-pick-d') ? 'button' : id === 'journal-pick-all' ? 'input' : 'div'));
  const log = { fetches: [], receipts: [], navigated: [], toList: 0, reloads: 0 };
  const answers = {};
  const fetchStub = async (url, options = {}) => {
    log.fetches.push({ url, method: options.method || 'GET', body: options.body ? JSON.parse(options.body) : undefined });
    const answer = answers[`${options.method || 'GET'} ${url}`];
    if (answer instanceof Error) throw answer;
    const value = typeof answer === 'function' ? await answer() : answer;
    if (!value) return { ok: false, status: 500, json: async () => null };
    return { ok: (value.status || 200) < 400, status: value.status || 200, json: async () => value.body };
  };
  const context = {
    document: doc, console, Promise, Map, Set, Number, String, Array, Object, JSON, Date, URLSearchParams,
    fetch: fetchStub,
    window: {
      AppServices: { notifications: { show: (notice) => { log.receipts.push(notice); return log.receipts.length; } } },
      JournalClock: { dayKey: (instant) => instant.toISOString().slice(0, 10).replace(/-/g, '') },
      JournalFormat: { day: (key) => `day:${key}` },
      location: { href: '' },
      SurfaceState: { loading: () => '' },
    },
    journalSummary: null,
    journalFolder: '',
    journalEntitySearch: '',
    journalEntityPage: 0,
    journalSearchDebounce: null,
    scopeMetadataDirty: false,
    facetRoster: [],
    tidyState: { data: null },
    JOURNAL_ENTITY_PAGE_SIZE: 50,
    // The mount's context and focus helper, as the slices call them.
    ctx: { on: (target, type, fn, options) => { target.addEventListener?.(type, fn, options); return () => {}; } },
    holdFocus: (_own, done) => Promise.resolve(done),
    reportEntitiesReady: () => {},
  };
  vm.createContext(context);
  return { context, doc, log, answers };
}

// The notices' module-level part, and the view looked up as showing: what a
// notice's button reaches while entities is the view on the page.
function noticesSource(workspace) {
  const start = workspace.indexOf('// ENTITIES NOTICES (module level)');
  const end = workspace.indexOf('// END ENTITIES NOTICES', start);
  if (start === -1 || end < start) throw new Error('the notices are marked');
  return `
    const ENTITIES_PATH = '/app/entities/';
    function entitiesHere() { return true; }
    ${workspace.slice(start, end)}
  `;
}
const SHOWING_HOOKS = `
  entitiesShowing = () => true;
  entitiesHooks = {
    recover: (ids) => recoverEntities(ids, reloadEntityList),
    open: (entityId) => navigateToEntity(entityId),
    find: (entityId) => { selectEntityScope(null); navigateToEntity(entityId); },
    facet: () => currentFacet,
    unhide: (name) => unhideDetectedName(name, null),
    retryHide: (name) => retryHide({ name }),
  };
`;

async function entityDeleteCases(workspace) {
  const start = workspace.indexOf('// ========== ENTITY DELETE ==========');
  const end = workspace.indexOf('// ========== END ENTITY DELETE ==========');
  check(start !== -1 && end > start, 'the entity delete block is marked');
  const listStart = workspace.indexOf('function renderJournalEntities()');
  const listEnd = workspace.indexOf('function appendTypeIndicator(');
  check(listStart !== -1 && listEnd > listStart, 'the journal list rendering is reachable');
  check(!workspace.includes('deleteJournalEntityModal') && !workspace.includes('cancelJournalEntityDelete')
    && !workspace.includes('watchEntityDeleteOutcome') && !workspace.includes('reportUnfinishedEntityDeletes')
    && !workspace.includes('api/delete-status') && !workspace.includes('api/cancel-delete'),
  'the old permanent delete, its cancel window and its outcome polling are gone');
  check(!workspace.includes('id="deleteModal"') && !workspace.includes('detected/preview')
    && !workspace.includes('previewEntityDelete') && !workspace.includes('confirmEntityDelete'),
  'the detections delete, its dialog and its preview are gone');
  const pageButton = /<button[^>]*id="journal-detail-delete-btn"[^>]*>/.exec(workspace);
  check(pageButton && pageButton[0].includes('btn-secondary') && !pageButton[0].includes('btn-danger'), 'the pane delete is secondary: it leads to the reversible act');

  const flush = async () => { for (let i = 0; i < 6; i += 1) await new Promise((resolve) => setImmediate(resolve)); };
  const setup = () => {
    const h = pickHarness();
    vm.runInContext(`
      function groupedCount(n) { return String(n); }
      function formatDateShort() { return ''; }
      function formatMentionedDate() { return ''; }
      function renderTidyBand() {}
      function renderIdsPanel() {}
      function renderDupsPanel() {}
      function captureIdSelection() {}
      function loadTidyGroups() {}
      function writeJournalListState() { return true; }
      async function loadJournalEntities() { renderJournalEntities(); }
      function navigateToEntity(id) { __log.navigated.push(id); }
      function navigateToList() { __log.toList += 1; }
      async function refreshEntitiesAfterMutation() {
        __log.reloads += 1;
        if (__nextSummary) { journalSummary = __nextSummary; __nextSummary = null; renderJournalEntities(); }
        else scopeMetadataDirty = true;
      }
      var __nextSummary = null;
      ${noticesSource(workspace)}
      ${workspace.slice(listStart, listEnd)}
      ${workspace.slice(start, end)}
      ${SHOWING_HOOKS}
      this.api = { ENTITY_DELETE_COPY, entityPick, entitySetOutcome, entityOneListLines, renderEntityDeleteAction, renderJournalEntities,
        selectJournalFolder, deleteSetFromEntityList, deleteEntityFromPane, setSummary(s) { journalSummary = s; }, setNext(s) { __nextSummary = s; },
        setFolder(f) { journalFolder = f; }, dirty(v) { scopeMetadataDirty = v; } };
    `.replace(/__log/g, 'this.__log'), h.context);
    h.context.__log = h.log;
    return h;
  };
  const ent = (id, extra = {}) => ({ id, name: `name-${id}`, type: 'Person', type_bucket: 'person', facets: [], ...extra });
  const summary = (items, total = items.length) => ({ items, total, entity_total: total, offset: 0 });
  const bodyKeys = (node) => node.children.map((child) => child.textContent);

  // The pane: none for you; for anyone else, the button and its helper sentence.
  {
    const h = setup();
    const C = h.context.api.ENTITY_DELETE_COPY;
    h.context.api.renderEntityDeleteAction(ent('me', { is_principal: true }));
    check(h.doc.getElementById('journal-detail-delete-btn').hidden === true && h.doc.getElementById('journal-detail-delete-help').hidden === true,
      'your own entity has no delete and no delete sentence');
    h.context.api.renderEntityDeleteAction(ent('a'));
    const button = h.doc.getElementById('journal-detail-delete-btn');
    check(button.hidden === false && button.disabled === false && button.textContent === C.button, 'another entity has the delete button');
    check(h.doc.getElementById('journal-detail-delete-help').hidden === false
      && h.doc.getElementById('journal-detail-delete-help-text').textContent === C.paneHelp, 'the helper says delete moves it to the trash');
    h.context.api.renderEntityDeleteAction(null);
    check(button.hidden === true, 'nothing to delete while the entity loads');
  }

  // One entity's dialog: opens at once; the preview fills only true lines.
  {
    const h = setup();
    const C = h.context.api.ENTITY_DELETE_COPY;
    const L = C.list;
    const lines = (preview) => Array.from(h.context.api.entityOneListLines(preview));
    check(lines(undefined).length === 0, 'the list waits for the preview');
    check(JSON.stringify(lines(null)) === JSON.stringify([L.details, L.facetUnknown, L.voiceUnknown]), 'a failed preview falls back to words without counts');
    check(JSON.stringify(lines({ type: 'person', facets: 0, voice: false })) === JSON.stringify([L.details]), 'no facets and no voice: only the true line');
    check(JSON.stringify(lines({ type: 'person', facets: 1, voice: true })) === JSON.stringify([L.details, L.facetOne, L.voice]), 'one facet and a voice');
    check(lines({ facets: 3 })[1] === L.facetMany.replace('{n}', 3), 'several facets carry their count');

    let release;
    h.answers['GET /app/entities/api/journal/entity/a/delete'] = () => new Promise((resolve) => { release = resolve; });
    h.context.api.deleteEntityFromPane({ id: 'a', name: 'Ada', type: 'person' });
    const modal = h.doc.getElementById('entityDeleteModal');
    const body = h.doc.getElementById('entityDeleteModalBody');
    check(modal.style.display === 'flex' && h.doc.activeElement === h.doc.getElementById('entityDeleteModalConfirm'),
      'the dialog opens without waiting, focus on move to trash');
    check(h.doc.getElementById('entityDeleteModalTitle').textContent === C.move.title, 'the move title');
    const metaText = body.children[0].textContent;
    check(metaText.includes('Ada') && metaText.includes('person'), 'the meta names it and its type');
    check(JSON.stringify(bodyKeys(body).slice(1)) === JSON.stringify([C.move.reach, C.move.lead, '', C.move.stays, C.again]), 'reach, lead, an empty list, what stays, found again');
    release({ body: { type: 'person', facets: 2, voice: true } });
    await flush();
    check(body.children[3].children.length === 3, 'the preview fills the list when it arrives');
    h.doc.getElementById('entityDeleteModalErase').fire('click');
    check(h.doc.getElementById('entityDeleteModalTitle').textContent === C.erase.title && h.doc.getElementById('entityDeleteModalHeader').classList.contains('danger'),
      'erase now swaps to the erase step');
    check(JSON.stringify(bodyKeys(body).slice(1, 3)) === JSON.stringify([C.erase.reach, C.erase.lead]) && bodyKeys(body)[5] === C.again,
      'the erase step has its own words and the found-again line');
    check(h.doc.activeElement === h.doc.getElementById('entityDeleteModalBack'), 'focus goes to back on the erase step');

    // Erase now: the body asks for it; the page goes to the list and the receipt shows.
    h.answers['DELETE /app/entities/api/journal/entity/a'] = { body: { success: true, erased: true } };
    h.doc.getElementById('entityDeleteModalEraseConfirm').fire('click');
    await flush();
    const del = h.log.fetches.find((f) => f.method === 'DELETE');
    check(del && del.body && del.body.erase === true, 'erase now sends erase');
    check(h.log.toList === 1 && h.log.reloads === 1, 'after an erase the page goes to the list and reloads it');
    check(h.log.receipts.at(-1).title === C.receipt.erased.title.replace('{noun}', C.noun), 'the erased receipt');
    check(modal.style.display === 'none', 'the dialog closed');
  }

  // A move: no body; the receipt offers recover, which posts the trash id from this page.
  {
    const h = setup();
    const C = h.context.api.ENTITY_DELETE_COPY;
    h.answers['GET /app/entities/api/journal/entity/b/delete'] = null;
    h.answers['DELETE /app/entities/api/journal/entity/b'] = { body: { success: true, trash: { id: 'T1', erase_at: '2099-11-17T00:00:00Z' } } };
    h.context.api.deleteEntityFromPane({ id: 'b', name: 'Bo', type: 'person' });
    await flush();
    check(h.doc.getElementById('entityDeleteModalBody').children[3].children.length === 3, 'a failed preview never blocks the dialog');
    h.doc.getElementById('entityDeleteModalConfirm').fire('click');
    await flush();
    const del = h.log.fetches.find((f) => f.method === 'DELETE');
    check(del.body === undefined, 'a move sends no body');
    const receipt = h.log.receipts.at(-1);
    check(receipt.title === C.receipt.moved.title && receipt.message.includes('day:20991117'), 'the moved receipt carries the erase date');
    check(receipt.buttons.map((b) => b.label).join() === [C.buttons.recover, C.buttons.openTrash].join(), 'recover and open trash');
    h.answers['POST /app/settings/api/trash/recover'] = { body: { done: ['T1'], failed: [] } };
    await receipt.buttons[0].onClick();
    const recover = h.log.fetches.find((f) => f.url === '/app/settings/api/trash/recover');
    check(JSON.stringify(recover.body) === JSON.stringify({ ids: ['T1'], page: 'entities' }), 'recover posts the trash id and this page');
    check(h.log.reloads === 2 && h.log.receipts.at(-1).title === C.receipt.recovered.title.replace('{noun}', C.noun), 'recover reloads the list and says so');
  }

  // Refusals: each code reaches its own words; the server's own words never show.
  {
    const C = (setup()).context.api.ENTITY_DELETE_COPY;
    const refusedWith = async (reason, erase) => {
      const h = setup();
      h.answers['GET /app/entities/api/journal/entity/c/delete'] = null;
      h.answers['DELETE /app/entities/api/journal/entity/c'] = { status: 409, body: { success: false, reason, error: 'SERVER WORDS' } };
      h.context.api.deleteEntityFromPane({ id: 'c', name: 'Cy', type: 'person' });
      await flush();
      if (erase) h.doc.getElementById('entityDeleteModalErase').fire('click');
      h.doc.getElementById(erase ? 'entityDeleteModalEraseConfirm' : 'entityDeleteModalConfirm').fire('click');
      await flush();
      check(h.log.toList === 0, `a refusal (${reason}) stays on the pane`);
      return h.log.receipts.at(-1);
    };
    for (const reason of ['busy', 'record_damaged', 'gone', 'principal', 'last_facet']) {
      const receipt = await refusedWith(reason, false);
      check(receipt.title === C.receipt.moveRefused.title && receipt.message === C.refused[reason], `a refused move keyed ${reason}`);
      check(!JSON.stringify(receipt).includes('SERVER WORDS'), 'no server reason reaches the owner');
    }
    const odd = await refusedWith('something-new', false);
    check(odd.message === C.receipt.moveRefused.message, 'an unknown code is other');
    const damagedErase = await refusedWith('record_damaged', true);
    check(damagedErase.title === C.receipt.eraseRefused.title.replace('{noun}', C.noun) && damagedErase.message === C.eraseRefused.record_damaged,
      'a refused erase has its own title and its own record line');
    const busyErase = await refusedWith('busy', true);
    check(busyErase.message === C.refused.busy, 'a refused erase shares the other messages');
  }

  // No answer at all: it may have happened.
  {
    const h = setup();
    const C = h.context.api.ENTITY_DELETE_COPY;
    h.answers['GET /app/entities/api/journal/entity/d/delete'] = null;
    h.answers['DELETE /app/entities/api/journal/entity/d'] = new TypeError('Failed to fetch');
    h.context.api.deleteEntityFromPane({ id: 'd', name: 'Di', type: 'person' });
    await flush();
    h.doc.getElementById('entityDeleteModalConfirm').fire('click');
    await flush();
    check(h.log.receipts.at(-1).title === C.receipt.noAnswerMove.title && h.log.receipts.at(-1).message === C.receipt.noAnswerMove.message,
      'no answer says it may already be in the trash');
  }

  // Select mode on the journal-wide list.
  {
    const h = setup();
    const api = h.context.api;
    const C = api.ENTITY_DELETE_COPY;
    const byId = (id) => h.doc.getElementById(id);
    const page1 = [ent('me', { is_principal: true }), ent('a'), ent('b'), ent('c')];
    api.setSummary(summary(page1, 120));
    api.renderJournalEntities();
    const container = byId('journal-entity-cards-content');
    check(byId('journal-select-btn').hidden === false && byId('journal-select-btn').textContent === C.select, 'select ends the list head');
    check(byId('journal-pick-bar').hidden === true && container.classList.contains('journal-rows-plain'), 'no strip and no ticks outside select mode');
    byId('journal-select-btn').onclick();
    const rows = () => container.children;
    check(byId('journal-pick-bar').hidden === false && byId('journal-select-btn').hidden === true, 'select mode shows the strip');
    check(!container.classList.contains('journal-rows-plain'), 'the rows take the tick column');
    check(rows()[0].children[0].classList.contains('journal-row-tick') && !rows()[0].querySelector('input[data-entity-pick]'),
      'your own entity keeps the column with no box');
    check(rows()[1].querySelector('input[data-entity-pick]') !== null, 'another entity has a box');
    check(byId('journal-pick-idle').hidden === false && byId('journal-pick-all-label').textContent === C.selectAllPage, 'more than one page: select all on this page');
    rows()[0].fire('click');
    check(h.log.navigated.at(-1) === 'me', 'your own entity still opens its page');
    rows()[1].fire('click');
    check(api.entityPick.items.has('a') && rows()[1].classList.contains('journal-row--picked'), 'pressing a row picks it');
    check(byId('journal-pick-some').hidden === false && byId('journal-pick-count').textContent === C.selected.replace('{n}', 1), 'the strip counts what is picked');
    rows()[2].fire('keydown', { key: ' ' });
    check(api.entityPick.items.has('b'), 'Space on a row picks it');
    rows()[2].fire('keydown', { key: 'Enter' });
    check(!api.entityPick.items.has('b') && h.log.navigated.length === 1, 'Enter toggles it back instead of opening it');
    const box = rows()[3].querySelector('input[data-entity-pick]');
    box.checked = true;
    box.fire('change');
    rows()[3].fire('click', { target: box });
    check(api.entityPick.items.has('c'), 'a press on the box itself picks once');

    // The selection holds across pages and searches inside a folder.
    api.setSummary(summary([ent('x'), ent('y')], 120));
    api.renderJournalEntities();
    check(byId('journal-pick-count').textContent === C.selected.replace('{n}', 2), 'a new page keeps what was picked');
    byId('journal-pick-all').onchange();
    check(api.entityPick.items.size === 4 && h.doc.activeElement === byId('journal-pick-delete'), 'select all adds this page and moves focus to delete');

    // A single page says select all.
    api.setSummary(summary(page1, 4));
    api.renderJournalEntities();
    api.selectJournalFolder('person');
    check(api.entityPick.on === true && api.entityPick.items.size === 0, 'changing the folder clears the selection');
    check(byId('journal-pick-all-label').textContent === C.selectAll, 'one page: select all');
    byId('journal-pick-all').onchange();
    check(!api.entityPick.items.has('me') && api.entityPick.items.size === 3, 'select all leaves your own entity out');
    api.setFolder('company');
    api.renderJournalEntities();
    check(api.entityPick.on === true && api.entityPick.items.size === 0, 'a folder changed by the browser clears the selection too');
    api.selectJournalFolder('ids');
    check(api.entityPick.on === false && byId('journal-pick-bar').hidden === true && byId('journal-select-btn').hidden === true,
      'a folder with its own ticks ends select mode');
    api.setFolder('');
    api.renderJournalEntities();
    byId('journal-select-btn').onclick();
    rows()[1].fire('click');
    byId('journal-pick-done-some').onclick();
    check(api.entityPick.on === false && api.entityPick.items.size === 0 && h.doc.activeElement === byId('journal-select-btn'), 'done clears and leaves select mode');
  }

  // A set: one confirm; what partly went keeps only the refused, each with its note.
  {
    const h = setup();
    const api = h.context.api;
    const C = api.ENTITY_DELETE_COPY;
    const byId = (id) => h.doc.getElementById(id);
    const items = ['a', 'b', 'c', 'd', 'e', 'f', 'g'].map((id) => ent(id));
    api.setSummary(summary(items, 7));
    api.renderJournalEntities();
    byId('journal-select-btn').onclick();
    // One picked on another page earlier.
    api.entityPick.items.set('far', { id: 'far', name: 'Far', type: 'person' });
    byId('journal-pick-all').onchange();
    byId('journal-pick-delete').onclick();
    const body = byId('entityDeleteModalBody');
    check(byId('entityDeleteModalTitle').textContent === C.move.titleMany.replace('{n}', 8), 'the set title counts them');
    const list = body.children[0];
    check(list.children.length === 6 && list.children[5].textContent === C.more.replace('{n}', 3), 'up to five, then how many more');
    check(JSON.stringify(bodyKeys(body).slice(1)) === JSON.stringify([C.move.reachMany, C.move.ownMany, C.move.staysMany, C.againMany]), 'the set has no counts');
    byId('entityDeleteModalErase').fire('click');
    check(JSON.stringify(bodyKeys(body).slice(1, 3)) === JSON.stringify([C.erase.reachMany, C.erase.ownMany]), 'the set erase step');
    byId('entityDeleteModalBack').fire('click');
    h.answers['POST /app/entities/api/journal/delete'] = { body: {
      done: ['a', 'b', 'c', 'd'].map((id) => ({ id, trash: { id: `T-${id}`, erase_at: '2099-11-17T00:00:00Z' } })),
      failed: [{ id: 'e', reason: 'busy' }, { id: 'f', reason: 'record_damaged' }, { id: 'g', reason: 'gone' }, { id: 'far', reason: 'weird' }],
    } };
    api.setNext(summary([ent('e'), ent('f')], 2));
    byId('entityDeleteModalConfirm').fire('click');
    await flush();
    const post = h.log.fetches.find((f) => f.url === '/app/entities/api/journal/delete');
    check(post.body.ids.length === 8 && post.body.erase === undefined, 'a set move posts its ids');
    check(api.entityPick.on === true && JSON.stringify([...api.entityPick.items.keys()].sort()) === JSON.stringify(['e', 'f', 'far']),
      'only the refused still in the journal stay picked; the gone one is dropped');
    const rows = byId('journal-entity-cards-content').children;
    check(rows[0].querySelector('.journal-row-note').textContent === C.notes.busy && rows[1].querySelector('.journal-row-note').textContent === C.notes.record_damaged,
      'each refused row carries its own note');
    check(api.entityPick.notes.get('far') && byId('journal-pick-count').textContent === C.selected.replace('{n}', 3), 'one picked elsewhere stays picked, counted');
    const receipt = h.log.receipts.at(-1);
    check(receipt.title === C.set.partMoved.title.replace('{done}', 4).replace('{n}', 8), 'the receipt says how many of how many moved');
    check(receipt.message === C.set.markedMany.replace('{k}', 3) + ' ' + C.set.goneMoreOne.replace('{g}', 1), 'it counts the marked and the gone');
    check(receipt.buttons[0].label === C.buttons.recover && receipt.autoDismiss === undefined, 'a partly done set offers recover and stays');
  }

  // The set outcome on its own: the gone, the unanswered and the erased.
  {
    const api = setup().context.api;
    const C = api.ENTITY_DELETE_COPY;
    const ids = ['a', 'b'];
    const there = () => true;
    const all = api.entitySetOutcome(false, ids, { done: ids.map((id) => ({ id, trash: { id: `T-${id}`, erase_at: '2099-11-17T00:00:00Z' } })), failed: [] }, there);
    check(all.receipt === C.set.moved && all.settled === true && all.keep.length === 0 && all.moved.length === 2, 'all moved: settled, select mode ends');
    const gone = api.entitySetOutcome(false, ids, { done: [], failed: [{ id: 'a', reason: 'gone' }, { id: 'b', reason: 'gone' }] }, there);
    check(gone.receipt === C.set.noneMoved && gone.message === C.set.noneGoneMany && gone.keep.length === 0, 'refused because gone: nothing kept, no note');
    const unanswered = api.entitySetOutcome(false, ids, null, (id) => id === 'a');
    check(unanswered.receipt === C.set.noAnswerMove && JSON.stringify(unanswered.keep) === JSON.stringify([['a', null]]), 'no answer: what is still listed stays picked without a note');
    const vanished = api.entitySetOutcome(false, ids, { done: [{ id: 'a', trash: { id: 'T-a' } }], failed: [{ id: 'b', reason: 'busy' }] }, (id) => id !== 'b');
    check(vanished.keep.length === 0 && vanished.message === C.set.othersGoneOne, 'a refused one the reloaded list no longer shows is not counted and gets no note');
    const noList = api.entitySetOutcome(false, ids, { done: [], failed: [{ id: 'a', reason: 'busy' }, { id: 'b', reason: 'busy' }] }, null);
    check(noList.message === '' && noList.keep.length === 0, 'without a reloaded list nothing is said that needs it');
    const erased = api.entitySetOutcome(true, ids, { done: ids.map((id) => ({ id })), failed: [] }, there);
    check(erased.receipt === C.set.erased && erased.settled === true, 'an erased set');
    const partErase = api.entitySetOutcome(true, ids, { done: [{ id: 'a', erased: true }], failed: [{ id: 'b', reason: 'busy' }] }, there);
    check(partErase.receipt === C.set.partErased && partErase.keep[0][1] === 'erase:busy', 'a refused erase takes the erase note');
  }
}


// Hiding a name's detections: the row's hide, its receipt, the hidden names
// and their unhide. Asserts which copy key reaches which element and which
// state renders; the expected words are read from the copy file, never typed.
async function detectedHideCases(workspace, copy) {
  const start = workspace.indexOf('// ========== DETECTED HIDE ==========');
  const end = workspace.indexOf('// ========== END DETECTED HIDE ==========');
  check(start !== -1 && end > start, 'the detected hide block is marked');
  const fill = (key, values = {}) => Object.entries(values).reduce((text, [name, value]) => text.split(`{${name}}`).join(String(value)), copy[key]);
  Object.keys(copy).filter((key) => key.startsWith('ENT_DETECTED_') || key === 'ENT_HIDE_PANE_HELP')
    .forEach((key) => check(typeof copy[key] === 'string' && copy[key].length > 0, `${key} has words`));
  const paneHelp = /<strong>hide<\/strong>\s*<span id="journal-detail-hide-help"><\/span>/.test(workspace)
    && workspace.includes("hideHelp.textContent = ENT_COPY.ENT_HIDE_PANE_HELP");
  check(paneHelp, 'the pane helper takes its hide sentence from its copy key');

  const flush = async () => { for (let i = 0; i < 6; i += 1) await new Promise((resolve) => setImmediate(resolve)); };
  const setup = (data) => {
    const h = pickHarness();
    const make = (id, tag = 'div') => { const node = new PickNode(h.doc, tag); node.id = id; h.doc.registry.set(id, node); return node; };
    ['all-entities-tbody', 'no-detected-entities', 'no-detected-matches', 'detected-hidden-foot', 'detected-hidden-list',
      'detected-hidden-count', 'detected-hidden-heading', 'detected-hidden-lede', 'detected-hidden-tbody', 'detected-action-error']
      .forEach((id) => make(id));
    make('detected-hidden-toggle', 'button');
    make('entity-search', 'input');
    h.log.errors = [];
    h.log.apis = [];
    h.context.window.ConveyIcons = { svg: (name) => `<svg data-icon="${name}"></svg>` };
    h.context.window.logError = () => {};
    h.context.window.apiJson = async (url, options = {}) => {
      h.log.apis.push({ url, method: options.method || 'GET', body: options.body ? JSON.parse(options.body) : undefined });
      const answer = h.answers[`${options.method || 'GET'} ${url}`];
      if (answer instanceof Error) throw answer;
      return typeof answer === 'function' ? answer() : answer;
    };
    h.context.window.JournalFormat = { day: (key) => `DAY:${key}` };
    h.context.apiJson = (...args) => h.context.window.apiJson(...args);
    vm.runInContext(`
      let ENT_COPY = ${JSON.stringify(copy)};
      let entitiesData = ${JSON.stringify(data)};
      let currentFacet = 'work';
      let detectedPage = 1;
      function facetTitle(name) { return 'Title of ' + name; }
      function groupedCount(n) { return String(n); }
      function renderDetectedPagination() {}
      function attachDetectedEntity() {}
      function renderEntities(term) { renderDetectedTable(term); }
      function showInlineError(id, text) { __log.errors.push(text); }
      function hideInlineError() {}
      function selectEntityScope(facet) { __log.scope = facet; }
      function navigateToEntity(id) { __log.navigated.push(id); }
      ${noticesSource(workspace)}
      ${workspace.slice(start, end)}
      function recoverEntities() {}
      function reloadEntityList() {}
      ${SHOWING_HOOKS}
      this.api = { renderDetectedTable, detectedNameKey, data: () => entitiesData, setFacet(f) { currentFacet = f; } };
    `.replace(/__log/g, 'this.__log'), h.context);
    h.context.__log = h.log;
    return h;
  };
  const el = (h, id) => h.doc.getElementById(id);
  const row = (name, extra = {}) => ({ type: 'Person', name, description: 'seen', count: 2, last_seen: '20261101', ...extra });
  const hideButtons = (h) => el(h, 'all-entities-tbody').all('ent-hide-btn');
  const unhideButtons = (h) => el(h, 'detected-hidden-tbody').all('ent-unhide-btn');

  // The row action: hide, keyed by name and facet title, never a delete.
  {
    const h = setup({ attached: [], detected: [row('Ada'), row('Bob')], hidden: [] });
    h.context.api.renderDetectedTable('');
    const [ada] = hideButtons(h);
    check(hideButtons(h).length === 2 && el(h, 'all-entities-tbody').all('entity-delete-btn').length === 2, 'each row has its hide in the place of the delete');
    check(ada.title === fill('ENT_DETECTED_HIDE_ACTION', { name: 'Ada', facet: 'Title of work' }) && ada.getAttribute('aria-label') === ada.title,
      'the hide is titled and labelled from its copy key with the name and facet title');
    check(ada.innerHTML.includes('eye-off') && !ada.innerHTML.includes('trash'), 'the hide carries the eye-off icon');
    check(el(h, 'detected-hidden-foot').hidden === true, 'nothing hidden: no foot line');
    check(el(h, 'no-detected-entities').style.display === 'none', 'rows: no empty line');
  }

  // A hide: the row leaves, the hidden count rises, focus moves on, the receipt shows.
  {
    const h = setup({ attached: [], detected: [row('Ada'), row('Bob'), row('Cy')], hidden: [] });
    h.context.api.renderDetectedTable('');
    h.answers['POST /app/entities/api/work/detected/hide'] = { success: true, changed: true, search_queued: true };
    hideButtons(h)[1].fire('click');
    await flush();
    const call = h.log.apis.find((c) => c.method === 'POST');
    check(call.url === '/app/entities/api/work/detected/hide' && call.body.name === 'Bob', 'a hide posts the name, with no confirm first');
    const names = el(h, 'all-entities-tbody').children.map((r) => r.children[2].textContent);
    check(names.length === 2 && !names.some((n) => n.startsWith('Bob')), 'the row leaves the list');
    check(el(h, 'detected-hidden-foot').hidden === false
      && el(h, 'detected-hidden-count').textContent === fill('ENT_DETECTED_HIDDEN_COUNT_ONE', { facet: 'Title of work' }), 'the foot line counts one hidden name');
    check(h.doc.activeElement === hideButtons(h)[1], "focus moves to the next row's hide");
    const receipt = h.log.receipts.at(-1);
    check(receipt.icon === 'eye-off' && receipt.app === 'entities' && receipt.autoDismiss === 8000, 'the receipt is the shared card, dismissing itself');
    check(receipt.title === fill('ENT_DETECTED_HIDE_DONE', { name: 'Bob', facet: 'Title of work' }) && receipt.message === copy.ENT_DETECTED_HIDE_DONE_DETAIL,
      'the receipt says the name is hidden here and stays hidden');
    check(receipt.buttons.length === 1 && receipt.buttons[0].label === copy.ENT_DETECTED_UNHIDE, 'its one button unhides');
    h.answers['POST /app/entities/api/work/detected/unhide'] = { success: true, changed: true, search_queued: true };
    h.answers['GET /app/entities/api/work'] = { attached: [], detected: [row('Ada'), row('Bob'), row('Cy')], hidden: [], hidden_unreadable: false };
    await receipt.buttons[0].onClick();
    await flush();
    check(h.log.apis.some((c) => c.url === '/app/entities/api/work/detected/unhide' && c.body.name === 'Bob'), 'the receipt unhides');
    check(h.log.receipts.length === 1, 'an unhide from the receipt makes no second card');
    check(el(h, 'all-entities-tbody').children.length === 3 && el(h, 'detected-hidden-foot').hidden === true, 'the name is back, read again');
  }

  // The last row hidden: focus goes to the search; the all-hidden line shows.
  {
    const h = setup({ attached: [], detected: [row('Ada')], hidden: [] });
    h.context.api.renderDetectedTable('');
    h.answers['POST /app/entities/api/work/detected/hide'] = { success: true, changed: true, search_queued: true };
    hideButtons(h)[0].fire('click');
    await flush();
    check(h.doc.activeElement === el(h, 'entity-search'), 'no rows left: focus goes to the search');
    check(el(h, 'no-detected-entities').textContent === copy.ENT_DETECTED_ALL_HIDDEN && el(h, 'no-detected-entities').style.display === 'block',
      'everything recent hidden: its own empty line, not the nothing-yet one');
    check(el(h, 'detected-hidden-foot').hidden === false, 'the foot line still shows');
  }

  // Search wasn't queued: the second card, which stays, with try again.
  {
    const h = setup({ attached: [], detected: [row('Ada')], hidden: [] });
    h.context.api.renderDetectedTable('');
    h.answers['POST /app/entities/api/work/detected/hide'] = { success: true, changed: true, search_queued: false };
    hideButtons(h)[0].fire('click');
    await flush();
    const receipt = h.log.receipts.at(-1);
    check(receipt.message === copy.ENT_DETECTED_HIDE_NO_SEARCH && receipt.autoDismiss === undefined, 'the no-search card stays until dismissed');
    check(receipt.buttons.map((b) => b.label).join() === [copy.ENT_DETECTED_HIDE_TRY_AGAIN, copy.ENT_DETECTED_UNHIDE].join(), 'try again, then unhide');
    h.answers['POST /app/entities/api/work/detected/hide'] = { success: true, changed: false, search_queued: true };
    await receipt.buttons[0].onClick();
    await flush();
    check(h.log.apis.filter((c) => c.url === '/app/entities/api/work/detected/hide').length === 2, 'try again hides again');
    check(h.log.receipts.at(-1).message === copy.ENT_DETECTED_HIDE_DONE_DETAIL, 'once queued, the first receipt');
  }

  // A refusal or no answer: the failure line, the row stays.
  {
    const h = setup({ attached: [], detected: [row('Ada')], hidden: [] });
    h.context.api.renderDetectedTable('');
    h.answers['POST /app/entities/api/work/detected/hide'] = new Error('refused');
    hideButtons(h)[0].fire('click');
    await flush();
    check(h.log.errors.at(-1) === fill('ENT_DETECTED_HIDE_FAILED', { name: 'Ada' }), 'a failed hide says nothing changed');
    check(el(h, 'all-entities-tbody').children.length === 1 && h.log.receipts.length === 0, 'the row stays and no receipt shows');
  }

  // The hidden list: the toggle, its rows, the search, unhide and its focus.
  {
    const hidden = [
      { name: 'Dr. Ellis', type: 'Person', count: 3, last_seen: '20261109' },
      { name: 'the clinic', type: 'Other', count: null, last_seen: null },
    ];
    const h = setup({ attached: [], detected: [row('Ada')], hidden });
    h.context.api.renderDetectedTable('');
    const toggle = el(h, 'detected-hidden-toggle');
    check(el(h, 'detected-hidden-count').textContent === fill('ENT_DETECTED_HIDDEN_COUNT', { n: 2, facet: 'Title of work' }), 'the foot line counts the names');
    check(toggle.textContent === copy.ENT_DETECTED_HIDDEN_SHOW && toggle.getAttribute('aria-expanded') === 'false' && el(h, 'detected-hidden-list').hidden === true,
      'the list starts closed');
    toggle.onclick();
    check(toggle.textContent === copy.ENT_DETECTED_HIDDEN_STOP_SHOW && toggle.getAttribute('aria-expanded') === 'true' && el(h, 'detected-hidden-list').hidden === false,
      'pressed, the list shows and the toggle says so');
    check(el(h, 'detected-hidden-heading').textContent === fill('ENT_DETECTED_HIDDEN_HEADING', { facet: 'Title of work' })
      && el(h, 'detected-hidden-lede').textContent === copy.ENT_DETECTED_HIDDEN_LEDE, 'the heading and its explainer');
    const rows = el(h, 'detected-hidden-tbody').children;
    check(rows.length === 2 && rows[0].all('journal-row-badge')[0].textContent === copy.ENT_DETECTED_HIDDEN_BADGE, 'each hidden name carries the hidden badge');
    check(rows[0].all('mention-count-badge').length === 1 && rows[1].all('mention-count-badge').length === 0, 'the count shows only when there is one');
    check(rows[0].all('ent-hidden-last')[0].textContent === fill('ENT_DETECTED_HIDDEN_LAST', { day: 'day:20261109' })
      && rows[1].all('ent-hidden-last').length === 0, 'last detected, lowercased, only with a day');
    h.doc.getElementById('entity-search').value = 'clinic';
    h.context.api.renderDetectedTable('clinic');
    check(el(h, 'detected-hidden-tbody').children.length === 1, 'the search filters the hidden rows');
    check(el(h, 'detected-hidden-list').hidden === false, 'the list stays shown while the owner is on the page');
    h.doc.getElementById('entity-search').value = '';
    h.context.api.renderDetectedTable('');

    h.answers['POST /app/entities/api/work/detected/unhide'] = { success: true, changed: true, search_queued: true };
    h.answers['GET /app/entities/api/work'] = { attached: [], detected: [row('Ada'), row('Dr. Ellis')], hidden: [hidden[1]], hidden_unreadable: false };
    const first = unhideButtons(h)[0];
    first.fire('click');
    await flush();
    check(h.log.apis.some((c) => c.url === '/app/entities/api/work/detected/unhide' && c.body.name === 'Dr. Ellis'), 'unhide posts the name');
    check(el(h, 'all-entities-tbody').children.length === 2 && unhideButtons(h).length === 1, 'the name is back in the list, read again');
    check(h.doc.activeElement === unhideButtons(h)[0], 'focus goes to the next unhide');
    check(h.log.receipts.length === 0, 'an unhide needs no receipt');
    h.answers['GET /app/entities/api/work'] = { attached: [], detected: [row('Ada'), row('Dr. Ellis'), row('the clinic')], hidden: [], hidden_unreadable: false };
    unhideButtons(h)[0].fire('click');
    await flush();
    check(el(h, 'detected-hidden-foot').hidden === true, 'none left: the foot line goes');
    const focused = h.doc.activeElement;
    check(focused && focused !== el(h, 'entity-search') && String(focused.className).includes('ent-hide-btn')
      && focused.dataset.nameKey === h.context.api.detectedNameKey('the clinic'),
      'none left: focus goes to the restored row\'s hide');

    h.context.api.setFacet('home');
    h.context.api.data().hidden = hidden;
    h.context.api.renderDetectedTable('');
    check(el(h, 'detected-hidden-list').hidden === true, 'a facet change closes the list');
  }

  // Unhide while a hidden entity still matches: the card names it and opens it.
  {
    const h = setup({ attached: [], detected: [], hidden: [{ name: 'Bob Smith', type: 'Person', count: 1, last_seen: '20261101' }] });
    h.context.api.renderDetectedTable('');
    el(h, 'detected-hidden-toggle').onclick();
    h.answers['POST /app/entities/api/work/detected/unhide'] = { success: true, changed: true, search_queued: true, still_hidden_by: { id: 'bob', name: 'Robert Smith' } };
    h.answers['GET /app/entities/api/work'] = { attached: [], detected: [], hidden: [], hidden_unreadable: false };
    unhideButtons(h)[0].fire('click');
    await flush();
    const card = h.log.receipts.at(-1);
    check(card && card.title === fill('ENT_DETECTED_STILL_HIDDEN', { name: 'Bob Smith', entity: 'Robert Smith' }) && card.autoDismiss === undefined,
      'the card names the hidden entity and stays');
    check(card.buttons.length === 1 && card.buttons[0].label === fill('ENT_DETECTED_STILL_HIDDEN_OPEN', { entity: 'Robert Smith' }), 'its button opens the entity');
    card.buttons[0].onClick();
    check(h.log.scope === null && h.log.navigated.at(-1) === 'bob', 'it goes to the entity in the whole journal');
  }

  // A failed unhide: the failure line; the name stays hidden.
  {
    const h = setup({ attached: [], detected: [], hidden: [{ name: 'Bob', type: 'Person', count: 1, last_seen: '20261101' }] });
    h.context.api.renderDetectedTable('');
    el(h, 'detected-hidden-toggle').onclick();
    h.answers['POST /app/entities/api/work/detected/unhide'] = new Error('refused');
    unhideButtons(h)[0].fire('click');
    await flush();
    check(h.log.errors.at(-1) === fill('ENT_DETECTED_UNHIDE_FAILED', { name: 'Bob' }) && unhideButtons(h).length === 1, 'a failed unhide says nothing changed');
  }

  // The hidden names can't be read: its own line, no foot line, no rows.
  {
    const h = setup({ attached: [], detected: [row('Ada')], hidden: [{ name: 'Bob' }], hiddenUnreadable: true });
    h.context.api.renderDetectedTable('');
    check(el(h, 'no-detected-entities').textContent === copy.ENT_DETECTED_UNREADABLE && el(h, 'no-detected-entities').style.display === 'block',
      'the unreadable line shows');
    check(el(h, 'all-entities-tbody').children.length === 0 && el(h, 'detected-hidden-foot').hidden === true, 'no detections and no foot line');
  }

  // Nothing detected and nothing hidden: the nothing-yet line, unchanged.
  {
    const h = setup({ attached: [], detected: [], hidden: [] });
    h.context.api.renderDetectedTable('');
    check(el(h, 'no-detected-entities').textContent === copy.ENT_DETECTED_EMPTY, 'nothing yet');
  }
}

async function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) throw new Error('manifest directory required');
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/entities/workspace.html'), 'utf8');
  check(!workspace.includes('window.selectedFacet'), 'Entities does not read the shared selected facet');
  check(!workspace.includes('window.facetsData'), 'Entities does not read the shared facet roster');
  check(!workspace.includes('facet.switch'), 'Entities does not register the retired facet event');

  const scriptStart = workspace.lastIndexOf('<script>') + '<script>'.length;
  const scriptEnd = workspace.lastIndexOf('</script>');
  new vm.Script(workspace.slice(scriptStart, scriptEnd), {filename: 'entities-workspace.js'});
  check(scriptStart >= '<script>'.length && scriptEnd > scriptStart, 'the complete Entities workspace script parses');

  // Every entities refusal answers `error: "Entity request refused"`; the owner's reason is in `detail`.
  const errorStart = workspace.indexOf('function entityTrustFormat(');
  const errorEnd = workspace.indexOf('function entityTrustIdentitySummary', errorStart);
  check(errorStart !== -1 && errorEnd !== -1, 'workspace exposes the entities error-message helper');
  const errorContext = {
    window: { createViewReady: () => () => {},SurfaceState: {serverMessageFrom: (err) => err?.serverMessage || ''}},
    ENT_COPY: {},
  };
  vm.runInNewContext(
    `${workspace.slice(errorStart, errorEnd)}\nwindow.entityErrorMessage = entityErrorMessage;`,
    errorContext,
    {filename: 'entities-error-message.js'},
  );
  const entityErrorMessage = errorContext.window.entityErrorMessage;
  const refused = {status: 400, serverMessage: 'Entity request refused', rawDetail: "can't merge a person into something that isn't a person."};
  check(entityErrorMessage(refused, 'fallback') === refused.rawDetail, 'a refusal shows its detail, not the generic error');
  check(entityErrorMessage({serverMessage: 'Not Found'}, 'fallback') === 'Not Found', 'an error without detail shows its server message');
  check(entityErrorMessage(new TypeError('Failed to fetch'), 'fallback') === 'fallback', 'an error with neither shows the fallback');
  check(
    entityErrorMessage({status: 500, serverMessage: 'Entity request refused', rawDetail: 'No such file or directory (os error 2)'}, 'fallback') === 'fallback',
    'a server failure shows the fallback, not its engineering detail',
  );
  // Failed is also returned after a successful rollback, so its partial-state
  // envelope is not a reason to disable the recovery-aware retry path.
  const mergeFailure = {
    status: 500,
    serverMessage: 'Entity request refused',
    rawDetail: 'entity merge failed during audit: injected failure',
    payload: {
      error: 'Entity request refused', reason_code: 'entity_operation_failed',
      detail: 'entity merge failed during audit: injected failure',
      operation_state: 'partially_applied', mutation_applied: true,
      failed_phase: 'audit', source_id: 'source', target_id: 'target',
      source_state: {id: 'source'}, target_state: {id: 'target'},
      safe_remediation: 'Contact an operator to repair this merge.',
    },
  };
  const mergeStart = workspace.indexOf('async function commitJournalEntityMerge(');
  const mergeEnd = workspace.indexOf('function showJournalMergeDone(', mergeStart);
  let calls = 0;
  let shownError;
  const mergeContext = {
    ...errorContext,
    currentDetailEntity: {name: 'source'},
    ENT_COPY: {ENT_TRUST_ACTION_FAILED: 'failure fallback'},
    window: { createViewReady: () => () => {},
      ...errorContext.window,
      confirm: () => true,
      apiJson: async () => { calls += 1; throw mergeFailure; },
    },
    // The view's reads and writes go through its mount's `apiJson`.
    apiJson: (...args) => mergeContext.window.apiJson(...args),
    showInlineError: (_id, message) => { shownError = message; },
  };
  vm.runInNewContext(workspace.slice(errorStart, errorEnd) + workspace.slice(mergeStart, mergeEnd), mergeContext);
  const mergeButton = {disabled: false};
  await mergeContext.commitJournalEntityMerge('source', 'target', 'target', mergeButton);
  check(!mergeButton.disabled, 'a failed entity merge leaves its confirm button retryable');
  check(shownError === mergeContext.ENT_COPY.ENT_TRUST_ACTION_FAILED, 'a failed merge shows the fallback rather than diagnostics or operator advice');
  await mergeContext.commitJournalEntityMerge('source', 'target', 'target', mergeButton);
  check(calls === 2 && !mergeButton.disabled, 'a repeat confirmation reaches the recovery-aware merge route');

  const curation = fs.readFileSync(path.join(manifestDir, '../solstone-core-facets-web/assets/curation/workspace.html'), 'utf8');
  const curationStart = curation.indexOf('  function repairRequired(');
  const curationEnd = curation.indexOf('  function postJson(', curationStart);
  const curationContext = {
    window: { createViewReady: () => () => {},SurfaceState: {serverMessageFrom: (err) => err.serverMessage}},
    CUR_COPY: {CUR_REPAIR_REQUIRED: '{detail} {remediation}'},
    copyFormat: (text, values) => text.replace(/\{(\w+)\}/g, (_, key) => values[key]),
    setBusy: (control, busy) => { control.disabled = busy; },
    showError: (_item, message) => { shownError = message; },
  };
  vm.runInNewContext(curation.slice(curationStart, curationEnd), curationContext);
  const curationButton = {disabled: true};
  const siblingControl = {disabled: false};
  const item = {querySelectorAll: () => [curationButton, siblingControl]};
  // Curation returns an ordinary per-item error for EntityMergeError::Failed.
  curationContext.showMutationError(item, curationButton, {
    serverMessage: mergeFailure.rawDetail,
    payload: {status: 'error', kind: 'entity_merge', error: mergeFailure.rawDetail},
  }, 'fallback');
  check(!curationButton.disabled && !siblingControl.disabled, 'curation also permits a repeat entity merge');
  curationContext.showMutationError(item, curationButton, {
    payload: {operation_state: 'repair_required', safe_remediation: 'speaker remedy'},
  }, 'fallback');
  check(curationButton.disabled && siblingControl.disabled, 'speaker repair still disables curation mutations');

  const helperBody = workspace.slice(errorStart, errorEnd);
  const outsideHelper = workspace.replace(helperBody, '');
  check(!outsideHelper.includes('SurfaceState.serverMessageFrom('), 'every entities error surface reads through entityErrorMessage');
  check(!/err(or)?\?\.serverMessage/.test(outsideHelper), 'no entities error surface reads the generic server message directly');
  check(!/new Error\((data|body)\.error/.test(workspace), 'raw-fetch failures prefer the refusal detail');

  const start = workspace.indexOf('let ENT_COPY = {};');
  const scopeEnd = workspace.indexOf('function showFacetDetailView', start);
  const metadataStart = workspace.indexOf('function adoptJournalSummary', start);
  const metadataEnd = workspace.indexOf('// Edit form submission', metadataStart);
  check(start !== -1 && scopeEnd !== -1 && metadataStart !== -1 && metadataEnd !== -1, 'workspace exposes scope implementation boundaries');

  const documentListeners = {};
  const document = {
    activeElement: null,
    body: null,
    createElement() { return new Element(document); },
    getElementById(id) { return findById(document.body, id); },
    querySelector(selector) { return document.body.querySelector(selector); },
    addEventListener(name, listener) { (documentListeners[name] ||= []).push(listener); },
    removeEventListener(name, listener) {
      documentListeners[name] = (documentListeners[name] || []).filter((candidate) => candidate !== listener);
    },
  };
  document.body = new Element(document, 'body');
  const appendRoot = (id) => {
    const node = new Element(document, id);
    document.body.appendChild(node);
    return node;
  };
  appendRoot('entities-scope-anchor');
  appendRoot('entities-list-region');
  appendRoot('journal-entities-view');
  appendRoot('journal-entity-detail-view');
  appendRoot('entities-list-view');
  appendRoot('entity-detail-view');
  const emptyView = appendRoot('facet-entities-empty-view');
  emptyView.append(
    new Element(document, 'facet-entities-empty-title'),
    new Element(document, 'facet-entities-empty-body'),
    new Element(document, 'facet-entities-empty-action'),
  );

  const windowListeners = {};
  const window = { createViewReady: () => () => {},
    location: {pathname: '/app/entities/', search: '', hash: ''},
    history: {
      pushed: [],
      replaced: [],
      pushState(_state, _title, value) {
        this.pushed.push(value);
        setLocation(window, value);
      },
      replaceState(_state, _title, value) {
        this.replaced.push(value);
        setLocation(window, value);
      },
    },
    addEventListener(name, listener) { (windowListeners[name] ||= []).push(listener); },
    removeEventListener(name, listener) {
      windowListeners[name] = (windowListeners[name] || []).filter((candidate) => candidate !== listener);
    },
    emit(name) { (windowListeners[name] || []).forEach((listener) => listener()); },
    beginViewReady: () => () => {},
    logError() {},
    SurfaceState: {replaceLoading() {}, error() {}},
    CONVEY_COPY: {RELOAD_HINT: ''},
  };
  window.window = window;
  // What the mount gives the slices: its context and its own-entry helpers.
  const context = {
    window,
    document,
    URLSearchParams,
    history: window.history,
    location: window.location,
    console,
    ENTITIES_PATH: '/app/entities/',
    ctx: {
      disposed: false,
      on(target, type, fn, options) {
        target.addEventListener(type, fn, options);
        return () => target.removeEventListener(type, fn, options);
      },
    },
    entitiesHere: () => /^\/app\/entities(\/|$)/.test(window.location.pathname),
    entitiesEntryId: () => null,
    entityScrolls: new Map(),
    ownEntry(kind, url) {
      if (kind === 'push') window.history.pushState(null, '', url);
      else window.history.replaceState(null, '', url);
      return true;
    },
    holdFocus: (_own, done) => Promise.resolve(done),
    restoredFocusTarget: () => null,
  };
  const scopeSource = `${workspace.slice(start, scopeEnd)}\nfunction loadEntities() { window.__scopeLoads = (window.__scopeLoads || 0) + 1; showListView(); }`;
  vm.runInNewContext(scopeSource, context, {filename: 'entities-scope.js'});
  // The list-state key builder lives in the paged-list region, outside both slices; the scope
  // harness only needs adoptJournalSummary to record a key, not to compute one.
  context.journalListStateKey = () => 'harness';
  vm.runInNewContext(workspace.slice(metadataStart, metadataEnd), context, {filename: 'entities-metadata.js'});
  vm.runInNewContext(`
    window.__entities = {
      setRoster(value) { facetRoster = value; },
      setJournal(summary) { adoptJournalSummary(summary); },
      restore: restoreEntityScopeFromLocation,
      render: renderScopeControl,
      showList: showListView,
      currentFacet: () => currentFacet,
      count: facetCount,
    };
  `, context, {filename: 'entities-exports.js'});
  const entities = window.__entities;
  const copy = {
    ENT_SCOPE_SHOWING: 'showing',
    ENT_SCOPE_WHOLE_JOURNAL: 'your whole journal',
    ENT_SCOPE_EMPTY_TITLE: 'nothing in {facet} yet.',
    ENT_SCOPE_EMPTY_BODY: 'entities show up here as your journal holds people, places and projects in this facet.',
    ENT_SCOPE_EMPTY_ACTION: 'show your whole journal',
    ENT_SCOPE_FACET_MISSING: "that facet isn't in your journal any more, so this is your whole journal.",
  };
  vm.runInNewContext(`ENT_COPY = ${JSON.stringify(copy)};`, context, {filename: 'entities-copy.js'});

  entities.setRoster([
    {name: 'work', title: 'Work', color: '#2f6fdd'},
    {name: 'personal', title: 'Personal', color: '#a23f7b'},
  ]);
  // Counts come from the paged summary read; the route excludes blocked and
  // detached memberships server-side (see solstone-core-entities router tests).
  entities.setJournal({facet_counts: {work: 1}});
  entities.restore();
  const control = document.getElementById('entities-scope-control');
  check(control !== null, 'scope control is present when the roster is non-empty');
  const options = control.querySelectorAll('[role="option"]');
  check(options.length === 3, 'scope menu includes whole journal and every roster facet');
  check(
    JSON.stringify(options.map((option) => [option.children[0].textContent, option.children[1]?.textContent || '']))
      === JSON.stringify([['your whole journal', ''], ['Work', '1'], ['Personal', '0']]),
    'scope control shows counts from the journal summary and includes zero-entity facets',
  );

  setLocation(window, '/app/entities/?facet=work');
  entities.restore();
  entities.showList();
  check(entities.currentFacet() === 'work' && document.getElementById('entities-list-view').style.display === 'block', 'facet query selects the facet-scoped list');
  check(document.getElementById('entities-list-region').style.getPropertyValue('--entities-facet-color') === '#2f6fdd', 'facet tint stays on the local list region');

  setLocation(window, '/app/entities/');
  entities.restore();
  entities.showList();
  check(entities.currentFacet() === null && document.getElementById('journal-entities-view').style.display === 'block', 'plain Entities URL selects the journal-wide list');

  setLocation(window, '/app/entities/?facet=personal');
  entities.restore();
  entities.showList();
  check(document.getElementById('facet-entities-empty-view').style.display === 'block', 'zero-entity facets use the page-level empty state');
  check(
    document.getElementById('facet-entities-empty-title').textContent === 'nothing in Personal yet.'
      && document.getElementById('facet-entities-empty-body').textContent === copy.ENT_SCOPE_EMPTY_BODY
      && document.getElementById('facet-entities-empty-action').textContent === copy.ENT_SCOPE_EMPTY_ACTION,
    'page-level empty state uses its own scoped copy',
  );

  setLocation(window, '/app/entities/?facet=work');
  entities.restore();
  const journalOption = document.getElementById('entities-scope-control').querySelectorAll('[role="option"]')[0];
  journalOption.emit('click');
  check(window.history.pushed.at(-1) === '/app/entities/' && window.location.search === '', 'scope selection pushes the journal-wide query state');
  setLocation(window, '/app/entities/?facet=work');
  window.emit('popstate');
  check(entities.currentFacet() === 'work' && document.getElementById('entities-list-view').style.display === 'block', 'popstate restores the prior scoped list');
  check((window.__scopeLoads || 0) >= 2, 'scope selections and popstate rerender through the page loader');

  setLocation(window, '/app/entities/?facet=missing');
  entities.restore();
  check(window.location.search === '' && entities.currentFacet() === null, 'unknown facets canonicalize to the journal-wide URL');
  check(
    document.getElementById('entities-scope-notice').textContent === copy.ENT_SCOPE_FACET_MISSING,
    'unknown facets show the stale-facet notice',
  );

  entities.setRoster([]);
  entities.setJournal({facet_counts: {}});
  entities.render();
  check(document.getElementById('entities-scope-control') === null, 'scope control and popover are removed when the roster is empty');

  const parseStart = workspace.indexOf('function parseYYYYMMDD(');
  const parseEnd = workspace.indexOf('function formatDateShort(', parseStart);
  const connMapStart = workspace.indexOf('function entityConnCopyMap(');
  const connMapEnd = workspace.indexOf('async function fetchEntityConnJson(', connMapStart);
  const netStart = workspace.indexOf('function renderConnectionsNetwork(');
  const netEnd = workspace.indexOf('async function toggleConnectionEvidence(', netStart);
  check(parseStart !== -1 && parseEnd !== -1 && connMapStart !== -1 && connMapEnd !== -1 && netStart !== -1 && netEnd !== -1, 'connections workspace slices exist');

  const connDoc = {
    createElement() { return new Element(connDoc); },
  };
  const connContext = {
    document: connDoc,
    console,
    ENT_COPY: {
      ENT_CONN_SUMMARY: 'ENT_CONN_SUMMARY:{s}:{a}',
      ENT_CONN_SUMMARY_CALENDAR: 'ENT_CONN_SUMMARY_CALENDAR:{c}',
      ENT_CONN_ROW_META_SCHEDULED: 'ENT_CONN_ROW_META_SCHEDULED:{day}',
      ENT_CONN_CALENDAR_ONLY: 'ENT_CONN_CALENDAR_ONLY',
      ENT_CONN_EVENTS_ONLY: 'ENT_CONN_EVENTS_ONLY',
      ENT_CONN_ROW_META_ATTENDANCE: 'ENT_CONN_ROW_META_ATTENDANCE:{n}:{day}',
      ENT_CONN_ROW_META_ONE_ATTENDANCE: 'ENT_CONN_ROW_META_ONE_ATTENDANCE:{day}',
      ENT_CONN_ROW_META: 'ENT_CONN_ROW_META:{n}:{day}',
      ENT_CONN_ROW_META_ONE: 'ENT_CONN_ROW_META_ONE:{day}',
      ENT_CONN_WITH_YOU_SUMMARY: 'ENT_CONN_WITH_YOU_SUMMARY:{n}:{kind}:{day}',
      ENT_CONN_WITH_YOU_SUMMARY_ONE: 'ENT_CONN_WITH_YOU_SUMMARY_ONE:{kind}:{day}',
      ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED: 'ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:{day}',
      ENT_CONN_WITH_YOU_TOGGLE: 'ENT_CONN_WITH_YOU_TOGGLE',
    },
    ATTENDANCE_KINDS: new Set(['attended-with', 'co-present', 'scheduled-with']),
  };
  const connSource = [
    workspace.slice(parseStart, parseEnd),
    workspace.slice(connMapStart, connMapEnd),
    workspace.slice(netStart, netEnd),
  ].join('\n');
  vm.runInNewContext(connSource, connContext, { filename: 'entities-connections.js' });

  function allDescendants(node) {
    const list = [node];
    for (const child of node.children) {
      list.push(...allDescendants(child));
    }
    return list;
  }

  // The panel drops the extraction artifacts home's connections card drops.
  // Cases mirror solstone-core-home/src/connections.rs's tests, so the two
  // rules are checked against the same examples.
  const mentions = count => ({mentioned: {count}});
  [
    ['make', {mentioned: {count: 1329}, 'co-present': {count: 4}}],
    ['just', mentions(3249)],
    ['Own', mentions(991)],
    ['Élan', mentions(991)],
    ['jean-luc', mentions(991)],
  ].forEach(([name, kinds]) => {
    check(connContext.isEntityConnMentionOnlyWord({name, kinds}), `${name} is a word the journal only ever mentioned`);
  });
  [
    ['Ada', {'spoke-with': {count: 12}, mentioned: {count: 400}}],
    ['ada', {'attended-with': {count: 3}, mentioned: {count: 40}}],
    ['Ada Lovelace', mentions(900)],
    ['studio 54', mentions(900)],
    ['Kai', {mentioned: {count: 98}, 'spoke-with': {count: 2}}],
    ['make', undefined],
    ['make', {}],
    ['   ', mentions(5)],
  ].forEach(([name, kinds]) => {
    check(!connContext.isEntityConnMentionOnlyWord({name, kinds}), `${JSON.stringify(name)} stays a connection`);
  });
  ['the', ' The ', 'and', 'A', 'x', '1979', '—', 'of'].forEach(label => {
    check(connContext.isEntityConnResidueEvidence({name: 'Gallery At Reunion (The)', evidence: [{label}], kinds: mentions(40)}),
      `${JSON.stringify(label)} is a token, not a connection`);
  });
  [
    {name: 'Gallery At Reunion (The)', evidence: [{label: 'Reunion'}], kinds: mentions(40)},
    {name: 'Gallery At Reunion (The)', evidence: [{label: 'the gallery'}], kinds: mentions(40)},
    {name: 'Ada Lovelace', evidence: [{label: 'the'}], kinds: {mentioned: {count: 40}, 'spoke-with': {count: 9}}},
    {name: 'Ada Lovelace', kinds: mentions(40)},
    {name: 'Ada Lovelace', evidence: [{label: ''}], kinds: mentions(40)},
    {name: 'He', evidence: [{label: 'He'}], kinds: mentions(40)},
    {name: 'An', evidence: [{label: 'an'}], kinds: mentions(40)},
    {name: '何', evidence: [{label: '何'}], kinds: mentions(40)},
  ].forEach(row => {
    check(!connContext.isEntityConnResidueEvidence(row), `${row.name} / ${JSON.stringify(row.evidence)} stays a connection`);
  });
  const fetched = [
    {entity_id: 'just', name: 'just', kinds: mentions(3680), evidence: [{label: 'just'}]},
    {entity_id: 'gallery', name: 'Gallery At Reunion (The)', kinds: mentions(5578), evidence: [{label: 'The'}]},
    ...Array.from({length: 20}, (_, i) => ({entity_id: `p${i}`, name: `Person ${i}`, kinds: {'spoke-with': {count: 5}}})),
  ];
  const shown = connContext.shownConnectionNeighbors(fetched);
  check(shown.length === 15, 'the panel stays full after dropping artifacts');
  check(shown[0].entity_id === 'p0' && !shown.some(row => row.entity_id === 'just' || row.entity_id === 'gallery'),
    'artifacts are dropped and the next real neighbours move up');

  const connContainer = new Element(connDoc, 'connections-container');
  const testNeighbors = [
    {entity_id:'sem', name:'Ada', evidence_class:'semantic', count:2, last_seen:'20260501', kinds:{'works-with':{weighted:1}}},
    {entity_id:'att', name:'Bea', evidence_class:'attendance', count:3, last_seen:'20260502', kinds:{'attended-with':{weighted:1}}},
    {entity_id:'sch', name:'Cal', evidence_class:'scheduled', count:4, last_seen:'20260503', kinds:{'scheduled-with':{weighted:1}}},
  ];
  connContext.renderConnectionsNetwork(connContainer, 'self', testNeighbors, '20260530', true);

  const connRows = allDescendants(connContainer).filter(node => node.className === 'entity-conn-row');
  const calRow = connRows.find(r => allDescendants(r).some(n => n.className === 'entity-conn-name' && n.textContent === 'Cal'));
  check(calRow !== undefined, 'found scheduled row for Cal');
  const calDescendants = allDescendants(calRow);
  const calText = calDescendants.map(n => n.textContent).join(' ');

  check(
    calDescendants.some(n => n.textContent.includes('ENT_CONN_ROW_META_SCHEDULED')),
    'scheduled row contains ENT_CONN_ROW_META_SCHEDULED',
  );
  check(
    calDescendants.some(n => n.textContent === 'ENT_CONN_CALENDAR_ONLY' && n.className === 'entity-conn-chip entity-conn-chip-event-only'),
    'scheduled row has calendar-only chip with expected classes',
  );
  check(
    !calText.includes('ENT_CONN_EVENTS_ONLY')
      && !calText.includes('ENT_CONN_ROW_META_ATTENDANCE')
      && !calText.includes('ENT_CONN_ROW_META_ONE_ATTENDANCE'),
    'scheduled row does not contain attendance copy keys',
  );

  const summary = allDescendants(connContainer).find(n => n.className === 'entity-conn-summary');
  check(
    summary && summary.textContent === 'ENT_CONN_SUMMARY:1:1 · ENT_CONN_SUMMARY_CALENDAR:1',
    'summary includes direct, attendance, and scheduled counts',
  );

  const connContainer2 = new Element(connDoc, 'connections-container-2');
  connContext.renderConnectionsNetwork(connContainer2, 'self', [
    {entity_id:'sem', name:'Ada', evidence_class:'semantic', count:2, last_seen:'20260501', kinds:{'works-with':{weighted:1}}},
    {entity_id:'att', name:'Bea', evidence_class:'attendance', count:3, last_seen:'20260502', kinds:{'attended-with':{weighted:1}}},
  ], '20260530', true);
  const summary2 = allDescendants(connContainer2).find(n => n.className === 'entity-conn-summary');
  check(
    summary2 && summary2.textContent === 'ENT_CONN_SUMMARY:1:1' && !summary2.textContent.includes('ENT_CONN_SUMMARY_CALENDAR'),
    'summary without scheduled neighbours omits calendar clause',
  );

  function checkWithYou(history, label) {
    const container = new Element(connDoc);
    const rendered = connContext.renderWithYouBlock(container, 'alice', history, '20260530');
    check(rendered, `${label} rendered with-you block`);
    const summaryNode = allDescendants(container).find(n => n.className === 'entity-conn-with-you-summary');
    const text = summaryNode ? summaryNode.textContent : '';
    const toggleNode = allDescendants(container).find(n => n.className === 'entity-conn-with-you-toggle');
    const paneNode = allDescendants(container).find(n => n.className === 'entity-conn-evidence');
    return { container, text, toggleNode, paneNode };
  }

  const resScheduled1 = checkWithYou(
    { evidence_class: 'scheduled', total: 1, evidence: [{ kind: 'scheduled-with', day: '20260501' }] },
    'scheduled single',
  );
  check(
    resScheduled1.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:')
      && !resScheduled1.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
      && !resScheduled1.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE'),
    'scheduled total 1 uses scheduled copy and neither one- nor many-moments copy',
  );

  const resScheduledMany = checkWithYou(
    {
      evidence_class: 'scheduled',
      total: 3,
      evidence: [
        { kind: 'scheduled-with', day: '20260501' },
        { kind: 'scheduled-with', day: '20260501' },
      ],
    },
    'scheduled multiple',
  );
  check(
    resScheduledMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:')
      && !resScheduledMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
      && !resScheduledMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE'),
    'scheduled total > 1 uses scheduled copy and neither one- nor many-moments copy',
  );

  const otherClasses = ['semantic', 'mixed', 'attendance', undefined];
  for (const cls of otherClasses) {
    const historyOne = { total: 1, evidence: [{ kind: 'works-with', day: '20260501' }] };
    if (cls !== undefined) historyOne.evidence_class = cls;
    const resOne = checkWithYou(historyOne, `${cls ?? 'undefined'} total 1`);
    check(
      resOne.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE')
        && !resOne.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
        && !resOne.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:'),
      `${cls ?? 'undefined'} total 1 uses one-moment copy and neither many nor scheduled copy`,
    );

    const historyMany = {
      total: 4,
      evidence: [
        { kind: 'works-with', day: '20260501' },
        { kind: 'works-with', day: '20260501' },
      ],
    };
    if (cls !== undefined) historyMany.evidence_class = cls;
    const resMany = checkWithYou(historyMany, `${cls ?? 'undefined'} total > 1`);
    check(
      resMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
        && !resMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE')
        && !resMany.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:'),
      `${cls ?? 'undefined'} total > 1 uses many-moments copy and neither one nor scheduled copy`,
    );
  }

  const resNull = checkWithYou(
    {
      evidence_class: null,
      total: 2,
      evidence: [
        { kind: 'works-with', day: '20260501' },
        { kind: 'works-with', day: '20260501' },
      ],
    },
    'null class multiple',
  );
  check(
    resNull.text.includes('ENT_CONN_WITH_YOU_SUMMARY:')
      && !resNull.text.includes('ENT_CONN_WITH_YOU_SUMMARY_ONE')
      && !resNull.text.includes('ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED:'),
    'null evidence_class total > 1 uses many-moments copy and not scheduled copy',
  );

  check(
    resScheduled1.toggleNode
      && resScheduled1.toggleNode.textContent === 'ENT_CONN_WITH_YOU_TOGGLE'
      && resScheduled1.toggleNode.getAttribute('aria-expanded') === 'false',
    'scheduled body toggle has expected text and aria-expanded false',
  );
  check(
    resScheduled1.paneNode
      && resScheduled1.paneNode.className === 'entity-conn-evidence'
      && resScheduled1.paneNode.hidden === true,
    'scheduled body pane has entity-conn-evidence class and is hidden',
  );

  const resSemantic = checkWithYou(
    { evidence_class: 'semantic', total: 1, evidence: [{ kind: 'mentioned', day: '20260501' }] },
    'semantic toggle check',
  );
  check(
    resSemantic.toggleNode
      && resSemantic.toggleNode.textContent === 'ENT_CONN_WITH_YOU_TOGGLE'
      && resSemantic.toggleNode.getAttribute('aria-expanded') === 'false',
    'semantic body toggle has expected text and aria-expanded false',
  );
  check(
    resSemantic.paneNode
      && resSemantic.paneNode.className === 'entity-conn-evidence'
      && resSemantic.paneNode.hidden === true,
    'semantic body pane has entity-conn-evidence class and is hidden',
  );

  // Tidy-up: bulk hide batches and resume, undo numbers, merge planning.
  const tidyStart = workspace.indexOf('// Tidy-up:');
  const tidyEnd = workspace.indexOf('// ENTITIES MOUNT', tidyStart);
  check(tidyStart !== -1 && tidyEnd !== -1, 'workspace exposes the tidy-up block');
  function tidyHarness(api, {stored = null} = {}) {
    const storage = new Map(stored ? [['entities.tidyOperation', stored]] : []);
    const log = {requests: [], sleeps: [], storedAtFirstRequest: null};
    const context = {
      console,
      Promise,
      Map,
      Set,
      Uint8Array,
      journalFolder: '',
      facetRoster: [{name: 'work', title: 'Work'}],
      groupedCount: (n) => String(n),
      renderJournalEntities: () => {},
      loadJournalEntities: async () => {},
      // The mount's own apiJson, as the view calls it.
      apiJson: (...args) => context.window.apiJson(...args),
      window: { createViewReady: () => () => {},
        logError: () => {},
        crypto: {getRandomValues: (bytes) => bytes.fill(7)},
        sessionStorage: {
          getItem: (key) => (storage.has(key) ? storage.get(key) : null),
          setItem: (key, value) => storage.set(key, value),
          removeItem: (key) => storage.delete(key),
        },
        apiJson: async (url, opts) => {
          const body = opts && opts.body ? JSON.parse(opts.body) : null;
          if (log.storedAtFirstRequest === null && body) log.storedAtFirstRequest = storage.get('entities.tidyOperation') || '';
          log.requests.push({url, body});
          return api(url, body, log.requests.length);
        },
      },
    };
    vm.runInNewContext(`${workspace.slice(tidyStart, tidyEnd)}\nthis.tidyClock = tidyClock; this.tidyState = tidyState;`, context, {filename: 'entities-tidy.js'});
    context.tidyClock.sleep = async (ms) => { log.sleeps.push(ms); };
    context.renderTidyToast = () => {};
    context.renderIdsPanel = () => {};
    context.renderDupsPanel = () => {};
    return {context, log, storage};
  }
  const noLoss = (url) => {
    if (url.includes('tidy-up')) return {id_shaped: {count: 0, members: [], items: []}, duplicates: {count: 0, members: [], items: []}, damage: {count: 0, paths: []}};
    return null;
  };

  {
    const members = Array.from({length: 118}, (_, i) => `id${i}`);
    const {context, log} = tidyHarness((url, body) => noLoss(url) || {hidden: body.ids, already_hidden: [], missing: [], unreadable: []});
    context.tidyState.captured = members;
    context.tidyState.unticked = new Set(['id3', 'id110']);
    context.startBulkHide();
    context.startBulkHide();
    await new Promise(resolve => setImmediate(resolve));
    for (let i = 0; i < 20 && context.tidyState.running; i += 1) await new Promise(resolve => setImmediate(resolve));
    const hides = log.requests.filter(r => r.url.endsWith('/journal/block'));
    check(JSON.stringify(hides.map(r => r.body.ids.length)) === '[50,50,16]', 'bulk hide sends the 116 ticked ids in batches of 50, 50, 16');
    check(new Set(hides.map(r => r.body.operation)).size === 1, 'every batch carries one operation id');
    check(!hides.some(r => r.body.ids.includes('id3') || r.body.ids.includes('id110')), 'unticked ids are never sent');
    check(log.sleeps.filter(ms => ms >= 100).length >= 2, 'batches after the first wait at least 100 ms');
    check(JSON.parse(log.storedAtFirstRequest).phase === 'hiding', 'the operation is stored before the first request');
    check(!log.requests.slice(0, 3).some(r => r.url.includes('tidy-up')), 'no groups read runs between the press and the last batch');
    check(context.tidyState.op.view === 'hide-done' && context.tidyState.op.hidden.size === 116, 'a finished hide counts the hidden ids');
  }

  const settle = async (context) => {
    for (let i = 0; i < 40 && (context.tidyState.running || context.tidyState.dupBusy); i += 1) await new Promise(resolve => setImmediate(resolve));
    await new Promise(resolve => setImmediate(resolve));
  };
  const refusal = {status: 409, reasonCode: 'entity_busy_refusal', serverMessage: 'no'};

  {
    let calls = 0;
    const {context, log} = tidyHarness((url) => {
      const read = noLoss(url); if (read) return read;
      calls += 1;
      if (calls === 1) throw new TypeError('Failed to fetch');
      throw refusal;
    });
    context.tidyState.captured = ['a', 'b'];
    context.startBulkHide();
    await settle(context);
    const ops = new Set(log.requests.filter(r => r.body).map(r => r.body.operation));
    check(context.tidyState.op.view === 'hide-stopped' && ops.size === 1, 'a lost first attempt then a refusal is stopped partway under the same operation');
  }

  {
    const {context} = tidyHarness((url) => { const read = noLoss(url); if (read) return read; throw refusal; });
    context.tidyState.captured = ['a'];
    context.startBulkHide();
    await settle(context);
    check(context.tidyState.op.view === 'hide-failed', 'a refusal on a first batch with no earlier attempt means nothing was hidden');
  }

  {
    let calls = 0;
    const {context} = tidyHarness((url, body) => {
      const read = noLoss(url); if (read) return read;
      calls += 1;
      if (calls === 1) throw new TypeError('Failed to fetch');
      return {hidden: body.ids.filter(id => id !== 'x' && id !== 'y' && id !== 'z'), already_hidden: ['x', 'y'], missing: ['z'], unreadable: []};
    });
    context.tidyState.captured = ['a', 'b', 'x', 'y', 'z'];
    context.startBulkHide();
    await settle(context);
    const content = context.tidyToastContent(context.tidyState.op);
    check(context.tidyState.op.hidden.size === 2 && context.tidyState.op.other.size === 3, 'hide numbers come from the responses, not the ticked count');
    check(content.text.includes('2') && content.text.includes('3'), 'the done toast states m and k');
  }

  {
    const {context} = tidyHarness(() => null, {stored: JSON.stringify({id: 'op12345678', selection: ['a', 'b'], phase: 'hiding', hidden: [], other: []})});
    context.restoreTidyOp();
    check(context.tidyState.op.view === 'hide-stopped' && context.tidyState.op.unanswered, 'a reload while hiding restores the stopped-partway state as unanswered');
  }

  {
    const responses = [
      {restored: [{id: 'a', facets_reattached: []}], skipped: ['facets/w/entities/q/entity.json'], failed: [{id: 'x', reason: 'r'}], remaining: 2, unreadable: []},
      {restored: [{id: 'b', facets_reattached: []}], skipped: ['facets/w/entities/q/entity.json'], failed: [{id: 'x', reason: 'r'}], remaining: 1, unreadable: []},
      {restored: [{id: 'c', facets_reattached: []}], skipped: [], failed: [{id: 'x', reason: 'r'}], remaining: 0, unreadable: []},
    ];
    let n = 0;
    const {context, log} = tidyHarness((url) => noLoss(url) || responses[n++]);
    context.tidyState.op = {id: 'op12345678', selection: ['a', 'b', 'c', 'x'], phase: 'done', hidden: new Set(['a', 'b', 'c', 'x']), other: new Set(), view: 'hide-done', undo: null};
    await context.runTidyUndo();
    const op = context.tidyState.op;
    const content = context.tidyToastContent(op);
    check(op.undo.total === 4 && op.undo.restored.size === 3 && op.undo.failed.size === 1, 'undo denominator is the first response total and failures dedupe');
    check(content.text.startsWith('put back 3 of 4.'), 'undo with a failure shows the put-back-of form');
    check(content.small && content.small.split('facets/w/entities/q/entity.json').length === 2, 'the facets line names a skipped path once');
    check(log.sleeps.filter(ms => ms >= 100).length >= 2, 'undo requests wait at least 100 ms');
  }

  {
    const {context} = tidyHarness((url) => noLoss(url) || {restored: [], skipped: [], failed: [{id: 'x', reason: 'r'}], remaining: 3, unreadable: []});
    context.tidyState.op = {id: 'op12345678', selection: ['x'], phase: 'done', hidden: new Set(['x']), other: new Set(), view: 'hide-done', undo: null};
    await context.runTidyUndo();
    check(context.tidyState.op.view === 'undo-stopped', 'undo with no progress stops instead of polling');
  }

  {
    const {context} = tidyHarness(() => null);
    const pair = (s, t, facets = ['work']) => ({pair: [s, t], names: [s, t], rows: facets.map(facet => ({facet, source_slug: s, target_slug: t}))});
    const members = [pair('A', 'B'), pair('C', 'B'), pair('A', 'C'), pair('D', 'E', ['work', 'home']), pair('F', 'A'), pair('B', 'G'), pair('G', 'H')];
    const selected = new Set(['A\u0000B', 'C\u0000B', 'A\u0000C', 'D\u0000E']);
    const plan = context.planDuplicateMerge(members, selected);
    check(JSON.stringify(plan.kept.map(m => m.pair.join(''))) === '["AB","CB","DE"]', 'the merge confirm keeps pairs that name no kept source');
    check(JSON.stringify(plan.leaving.map(m => m.pair.join(''))) === '["AC","FA"]', 'listed pairs naming a kept source leave the list, ticked or not');
  }

  {
    const pair = (i) => ({pair: [`s${i}`, `t${i}`], names: [`s${i}`, `t${i}`], rows: [{facet: 'work', source_slug: `s${i}`, target_slug: `t${i}`}]});
    const members = Array.from({length: 60}, (_, i) => pair(i));
    let batch = 0;
    const {context, log} = tidyHarness((url, body) => {
      if (url.includes('tidy-up')) return {id_shaped: {count: 0, members: []}, duplicates: {count: 59, members: members.slice(1), items: []}, damage: {count: 0, paths: []}};
      batch += 1;
      if (batch === 2) throw new TypeError('Failed to fetch');
      return {results: body.items.map(() => ({status: 'accepted'}))};
    });
    context.tidyState.data = {duplicates: {members}};
    await context.runDuplicateMerge({kept: members, leaving: []});
    const accepts = log.requests.filter(r => r.url.endsWith('accept-batch'));
    check(JSON.stringify(accepts.map(r => r.body.items.length)) === '[1,1]', 'merge sends one item per request and stops after a request with no response');
    check(context.tidyState.dupStatus === 'merged 1 of 60 pairs. 59 couldn\'t be merged.', 'pairs with no answer are decided by the re-read');
  }

  {
    let release;
    const gate = new Promise(resolve => { release = resolve; });
    const {context} = tidyHarness(async () => {
      await gate;
      return {id_shaped: {count: 1, actionable: true, members: ['code-1'], items: []}, duplicates: {count: 0, members: [], items: []}, damage: {count: 0, paths: []}};
    });
    const doc = {createElement: () => {
      const el = Object.assign(new Element(doc), {dataset: {}});
      const append = el.append.bind(el);
      el.append = (...nodes) => append(...nodes.filter(node => typeof node !== 'string'));
      return el;
    }};
    const host = new Element(doc, 'journal-detail-indicators');
    doc.getElementById = (id) => (id === 'journal-detail-indicators' ? host : null);
    context.document = doc;
    context.currentDetailEntity = null;
    context.loadTidyGroups();
    const shown = context.renderJournalDetailIdNote({id: 'code-1', entity_id: 'code-1'});
    release();
    await shown;
    check(host.children.some(node => node.className === 'journal-detail-id-note'), 'a detail opened while the groups read is in flight shows the ID line once the read lands');
  }

  {
    // The state read and the scope reads start together; the scope is drawn
    // with the state's copy, so it waits for that read.
    const initStart = workspace.indexOf('async function loadEntitiesState()');
    const initEnd = workspace.indexOf('async function loadEntities()', initStart);
    check(initStart !== -1 && initEnd !== -1, 'workspace exposes the entities start-up');
    const asked = [];
    const answers = new Map();
    const restoredWith = [];
    const noop = () => {};
    const initContext = {
      window: {
        apiJson(url) {
          asked.push(url);
          let resolve;
          const promise = new Promise((done) => { resolve = done; });
          answers.set(url, resolve);
          return promise;
        },
        logError: noop,
        SurfaceState: {replaceLoading: noop, error: () => ''},
        CONVEY_COPY: {RELOAD_HINT: ''},
      },
      document: {getElementById: () => null, querySelector: () => null, addEventListener: noop},
      ctx: {on: noop},
      apiJson: (...args) => initContext.window.apiJson(...args),
      URLSearchParams,
      location: {search: '', hash: ''},
      Set, String, Promise,
      resetEntitiesLoading: noop, readJournalListState: noop, writeJournalListState: noop,
      journalListStateKey: () => 'key', hideJournalListError: noop, entityErrorMessage: () => '',
      reportEntitiesReady: noop, loadEntityTypes: noop, loadEntities: noop, loadTidyGroups: noop,
    };
    vm.runInNewContext(`
      var ENT_COPY = {}, ATTENDANCE_KINDS = new Set(), journalSummary = null, journalSummaryKey = '',
        facetEntityCounts = {}, facetRoster = [], scopeMetadataDirty = true, entitiesInitialized = false,
        journalEntityPage = 0, journalEntityType = '', journalEntitySearch = '', journalFolder = '';
      const JOURNAL_ENTITY_PAGE_SIZE = 50;
      function restoreEntityScopeFromLocation() { window.__restoredWith.push(ENT_COPY.mark || null); }
    `, initContext);
    initContext.window.__restoredWith = restoredWith;
    vm.runInNewContext(workspace.slice(initStart, initEnd), initContext, {filename: 'entities-init.js'});
    const started = initContext.initEntities();
    await new Promise((resolve) => setImmediate(resolve));
    check(asked.includes('/app/entities/api/state'), 'the state read goes out at once');
    check(asked.includes('/app/settings/api/facets'), 'the facet read goes out with it');
    check(asked.includes('/app/entities/api/journal/summary?offset=0&limit=50&hidden=exclude'),
      'the summary read goes out with it, at the address the page head starts');
    answers.get('/app/settings/api/facets')({facets: []});
    answers.get('/app/entities/api/journal/summary?offset=0&limit=50&hidden=exclude')({facet_counts: {}});
    await new Promise((resolve) => setImmediate(resolve));
    check(restoredWith.length === 0, 'the scope waits for the state read');
    answers.get('/app/entities/api/state')({entities_copy: {mark: 'state copy'}, attendance_kinds: []});
    await started;
    check(restoredWith.length === 1 && restoredWith[0] === 'state copy', 'the scope is drawn with the state copy');
  }

  await entityDeleteCases(workspace);
  const entitiesCopy = JSON.parse(fs.readFileSync(path.join(manifestDir, '../solstone-core-entities/src/data/entities_copy.json'), 'utf8'));
  await detectedHideCases(workspace, entitiesCopy);
  // Entities on the view contract: the real fragment mounted in a page.
  cases += await require('./entities_contract_dom.js')(manifestDir);

  console.log(`DOM CASES: ${cases} passed`);
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
