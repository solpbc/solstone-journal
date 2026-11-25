// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The trash page's states and acts. Asserts which copy key reaches which element
// and which state renders; no assertion pins the words.
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { loadWindowCatalog, journalContext } = require('../../solstone-core-convey-shell/tests/journal_catalog_dom.js');

const crateDir = process.argv[2];
const html = fs.readFileSync(path.join(crateDir, 'assets/workspace.html'), 'utf8');
const start = html.indexOf('// ========== TRASH ==========');
const end = html.indexOf('// ========== END TRASH ==========');
assert.ok(start > 0 && end > start, 'the trash block is marked');
const history = html.slice(html.indexOf('// Removal history:'), html.indexOf('function renderRemovalHistory'));
const sourceNames = html.slice(html.indexOf('// ========== SOURCE NAMES =========='), html.indexOf('// ========== END SOURCE NAMES =========='));
assert.ok(sourceNames.includes('REMOVAL_SOURCE'), 'the source names block is marked');

class El {
  constructor(tag) { this.tag = tag; this.children = []; this.dataset = {}; this.style = {}; this.hidden = false; this.listeners = {}; this.className = ''; this._text = ''; this.classes = new Set(); this.classList = { toggle: (name, on) => { if (on) this.classes.add(name); else this.classes.delete(name); }, contains: (name) => this.classes.has(name) }; }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return this._text + this.children.map((child) => child.textContent).join(''); }
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren(...nodes) { this.children = nodes; this._text = ''; }
  addEventListener(name, fn) { this.listeners[name] = fn; }
  fire(name, event = {}) { return this.listeners[name]({ currentTarget: this, target: this, ...event }); }
  focus() { document.activeElement = this; }
  setAttribute(name, value) { (this.attributes ||= {})[name] = String(value); }
  getAttribute(name) { return this.attributes && name in this.attributes ? this.attributes[name] : null; }
  removeAttribute(name) { if (this.attributes) delete this.attributes[name]; }
  hasAttribute(name) { return Boolean(this.attributes && name in this.attributes); }
  get tagName() { return this.tag.toUpperCase(); }
  contains(node) { return node === this || this.children.some((child) => child.contains && child.contains(node)); }
  all(cls) { const out = []; for (const child of this.children) { if (child.className === cls) out.push(child); if (child.all) out.push(...child.all(cls)); } return out; }
}
const ids = ['section-trash', 'trashBar', 'trashBarIdle', 'trashBarSome', 'trashCount', 'trashSelectAll', 'trashSelectAllLabel', 'trashEmptyBtn', 'trashRecoverBtn', 'trashEraseBtn', 'trashResult', 'trashList', 'trashEraseAfterDays', 'trashHeading', 'trashConfirmHeading', 'trashConfirmBody', 'trashConfirmCancel', 'trashConfirmErase', 'trashConfirmModal', 'trashConfirmChoice', 'trashBackupsField', 'trashBackupsLine', 'trashBackupsSetting', 'trashBackupsNote', 'trashBackupsSettingLabel', 'trashBackupsMoreSummary', 'trashBackupsMoreList'];
const TRASH_OPENER_IDS = ['trashRecoverBtn', 'trashEraseBtn', 'trashEmptyBtn'];
const byId = Object.fromEntries(ids.map((id) => { const el = new El('div'); el.id = id; return [id, el]; }));
const modal = byId.trashConfirmModal;
modal._openAccessible = (trigger) => { modal.open = true; modal._triggerEl = trigger; };
modal._closeAccessible = () => { modal.open = false; };
byId.trashEraseAfterDays.disabled = true;
const document = {
  activeElement: null,
  getElementById: (id) => byId[id],
  createElement: (tag) => new El(tag),
  createTextNode: (text) => { const node = new El('#text'); node.textContent = text; return node; },
  querySelectorAll: () => byId.trashList.children.map((row) => row.children && row.children[0]).filter((box) => box && box.tag === 'input'),
};

let answers = {};
const calls = [];
const statuses = [];
const saves = [];
const window = {
  // The shell's focus helpers, as the page has them.
  SolFocus: require(path.join(crateDir, '..', 'solstone-core-convey-shell', 'assets', 'static', 'focus.js')),
  JournalClock: { dayKey: (instant) => instant.toISOString().slice(0, 10).replace(/-/g, ''), placeNote: () => {} },
  JournalFormat: { day: (key) => key, stream: (stream) => `label:${stream}`, },
  apiJson: async (url, options) => {
    calls.push({ url, options });
    const answer = answers[url];
    if (answer instanceof Error) throw answer;
    return typeof answer === 'function' ? answer() : answer;
  },
};
const sandbox = {
  window, document, Intl, Date, Set, Number, JSON, Object, Array, console,
  removalEl: (tag, cls, text) => { const el = new El(tag); el.className = cls || ''; if (text != null) el.textContent = text; return el; },
  removalDay: (key) => key,
  removalSegmentTime: (key) => (key ? `time:${key}` : null),
  loadRemovalHistory: () => {},
  showFieldStatus: (el, type, message) => statuses.push({ type, message }),
  saveField: (el, section, key, value) => saves.push({ section, key, value }),
  // The mount's read and write, and its state, as the trash block sees them.
  settingsApi: (url, options) => window.apiJson(url, options),
  ctx: { disposed: false },
  settingsPage: null,
};
sandbox.settingsPage = document;
vm.createContext(sandbox);
loadWindowCatalog(sandbox);
// The landings go through settings' one landing, as the page has it.
const land = html.slice(html.indexOf('function settingsLand('), html.indexOf('function settingsHeld('));
assert.ok(land.includes('SolFocus.refocus'), 'the settings landing is sliced');
vm.runInContext(`${sourceNames}\n${land}\n${html.slice(start, end)};this.api={trashLandings,TRASH_COPY,renderTrash,loadTrash,trashEraseDate,trashResultLine,trashConfirmCopy,setupTrash,saveTrashDays,renderTrashBackups,trashSelected,trashKindMessage,trashRowNote,REMOVAL_SOURCE,removalImportedLabel,TRASH_KIND_LABEL};`, sandbox);
const { api } = sandbox;
const C = api.TRASH_COPY;
const counted = (id, name, n) => window.JournalI18n.renderEnglish(
  'journal.window.settings.' + id, [['quantity', n], [name, String(n)]]
);
api.setupTrash();

const LATER = '2099-11-15T00:00:00.000Z';
const seg = (id, extra = {}) => ({ id, kind: 'segment', day: '20261101', stream: 'field', segment: '070000_300', erase_at: LATER, bytes: 2048, replaces: false, ...extra });
const mem = (id) => ({ id, kind: 'memory', day: '20261101', stream: 'agent-memory-x', segment: '080000_1', erase_at: LATER, bytes: 10, replaces: false });
const imp = (id, extra = {}) => ({ id, kind: 'import', source: 'chatgpt', import_id: '20261102_161200', label: 'conversations.zip', imported_day: '20261102', segments: 12, days: ['20261005', '20261009'], erase_at: LATER, bytes: 3 * 1024 * 1024, replaces: false, ...extra });
const flush = () => new Promise((resolve) => setImmediate(resolve));

(async () => {
  // Error is never the empty line.
  answers['/app/settings/api/trash'] = new Error('down');
  assert.strictEqual(await api.loadTrash(), false);
  assert.strictEqual(byId.trashList.children[0].textContent, C.error);
  assert.strictEqual(byId.trashBar.hidden, true);
  assert.strictEqual(api.renderTrash({ not: 'a list' }), false);

  // Empty.
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [] };
  await api.loadTrash();
  assert.strictEqual(byId.trashList.children[0].textContent, C.empty);
  assert.strictEqual(byId.trashBar.hidden, true);
  assert.strictEqual(byId.trashEraseAfterDays.value, '7');
  assert.strictEqual(byId.trashEraseAfterDays.__lastKnownValue, '7');
  assert.strictEqual(byId.trashEraseAfterDays.disabled, false);

  // Rows: a segment names its stream and size, a memory does neither.
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [seg('a', { replaces: true }), mem('b')] };
  await api.loadTrash();
  assert.strictEqual(byId.trashBar.hidden, false);
  assert.strictEqual(byId.trashBarIdle.hidden, false);
  assert.strictEqual(byId.trashBarSome.hidden, true);
  const [segRow, memRow] = byId.trashList.children;
  assert.ok(segRow.textContent.includes('label:field'));
  assert.ok(segRow.textContent.includes('2 KB'));
  assert.ok(segRow.textContent.includes(C.returned));
  assert.ok(!memRow.textContent.includes('label:'));
  assert.ok(!memRow.textContent.includes(C.returned));

  // An import names its source, its own file name and the day it came in, then
  // its segments and size; its returned line is its own.
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [imp('i', { replaces: true }), imp('j', { label: null, source: 'not-a-source', segments: 1 })] };
  await api.loadTrash();
  const [impRow, bareRow] = byId.trashList.children;
  const quals = (row) => row.all('removal-qual').map((node) => node.textContent);
  assert.strictEqual(impRow.all('removal-what')[0].textContent, api.TRASH_KIND_LABEL.import);
  assert.deepStrictEqual(quals(impRow), [api.REMOVAL_SOURCE.chatgpt, 'conversations.zip', api.removalImportedLabel('20261102')]);
  assert.ok(impRow.textContent.includes('12 segments'));
  assert.ok(impRow.textContent.includes('3.0 MB'));
  assert.ok(impRow.textContent.includes(C.returnedImport));
  assert.ok(!impRow.textContent.includes(C.returned));
  assert.deepStrictEqual(quals(bareRow), [api.removalImportedLabel('20261102')], 'no file name and an unknown source leave no empty qualifier');
  assert.ok(bareRow.textContent.includes('1 segment'));
  answers['/app/settings/api/trash'] = {
    erase_after_days: 7,
    items: [imp('g1', { source: 'garmin', label: 'ignored-file', days: ['20261101', '20261105'], imported_day: '20261106' })],
  };
  await api.loadTrash();
  const [garminRow] = byId.trashList.children;
  assert.deepStrictEqual(
    quals(garminRow),
    [api.REMOVAL_SOURCE.garmin, api.TRASH_COPY.bodySpan({ first: '20261101', last: '20261105' }), api.removalImportedLabel('20261106')],
    'a Garmin body sync run names Garmin and its day span instead of a filename',
  );
  assert.strictEqual(api.trashKindMessage('recover', [imp('i')], 1), 'recovered 1 import.');
  assert.strictEqual(api.trashKindMessage('recover', [imp('i'), imp('j')], 2), 'recovered 2 imports.');
  assert.strictEqual(api.trashKindMessage('recover', [imp('i'), seg('a')], 2), 'recovered 2 things.');
  assert.strictEqual(api.trashKindMessage('recover', [{ kind: 'workout' }], 1), 'recovered 1 workout.');
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [seg('a', { replaces: true }), mem('b')] };
  await api.loadTrash();

  // The date: never in the past.
  const now = new Date('2026-11-08T00:00:00Z');
  assert.deepStrictEqual({ ...api.trashEraseDate('2026-11-07T00:00:00Z', now) }, { text: C.erasingSoon, soon: true });
  assert.strictEqual(api.trashEraseDate(LATER, now).soon, false);
  assert.ok(api.trashEraseDate(LATER, now).text.includes('20991115'));

  // Selection swaps the strip; select all means every row.
  byId.trashSelectAll.fire('change');
  assert.strictEqual(api.trashSelected.size, 2);
  assert.strictEqual(byId.trashBarSome.hidden, false);
  assert.strictEqual(byId.trashBarIdle.hidden, true);
  assert.ok(byId.trashCount.textContent.includes('2'));
  assert.strictEqual(document.activeElement, byId.trashRecoverBtn, 'select all moves focus to the reversible act');

  // Recover has no confirm, and sends the selected ids.
  answers['/app/settings/api/trash/recover'] = { done: ['a', 'b'], failed: [] };
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [] };
  byId.trashRecoverBtn.fire('click');
  await flush();
  const recover = calls.find((call) => call.url === '/app/settings/api/trash/recover');
  assert.deepStrictEqual(JSON.parse(recover.options.body).ids, ['a', 'b']);
  assert.strictEqual(modal.open, undefined);
  assert.strictEqual(byId.trashResult.hidden, false);

  // Erase now always confirms, and only the confirm acts.
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [seg('a')] };
  await api.loadTrash();
  byId.trashSelectAll.fire('change');
  calls.length = 0;
  byId.trashEraseBtn.fire('click');
  assert.strictEqual(modal.open, true);
  assert.strictEqual(calls.length, 0);
  byId.trashConfirmCancel.fire('click');
  assert.strictEqual(modal.open, false);
  assert.strictEqual(calls.length, 0);
  byId.trashEraseBtn.fire('click');
  answers['/app/settings/api/trash/erase'] = { done: ['a'], failed: [] };
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [] };
  byId.trashConfirmErase.fire('click');
  await flush();
  assert.ok(calls.some((call) => call.url === '/app/settings/api/trash/erase'));

  // Empty confirms too, and posts no selection.
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [seg('a')] };
  await api.loadTrash();
  calls.length = 0;
  byId.trashEmptyBtn.fire('click');
  assert.strictEqual(modal.open, true);
  answers['/app/settings/api/trash/empty'] = { done: ['a'], failed: [] };
  byId.trashConfirmErase.fire('click');
  await flush();
  assert.deepStrictEqual(JSON.parse(calls.find((call) => call.url === '/app/settings/api/trash/empty').options.body), { backups: false }, 'empty sends only the backups choice');

  // One act at a time: a held erase keeps its opener busy, never disabled,
  // the other acts are off without being busy, and no press starts another.
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [seg('a'), seg('c')] };
  await api.loadTrash();
  byId.trashSelectAll.fire('change');
  calls.length = 0;
  let releaseErase = null;
  answers['/app/settings/api/trash/erase'] = () => new Promise((resolve) => { releaseErase = () => resolve({ done: ['a', 'c'], failed: [] }); });
  byId.trashEraseBtn.fire('click');
  await flush();
  byId.trashConfirmErase.fire('click');
  await flush();
  const attr = (el, name) => el.getAttribute(name);
  assert.ok(attr(byId.trashEraseBtn, 'aria-busy') === 'true' && attr(byId.trashEraseBtn, 'aria-disabled') === 'true', 'the pressed opener is busy');
  assert.ok(!byId.trashEraseBtn.disabled, 'and not disabled');
  assert.ok(attr(byId.trashRecoverBtn, 'aria-disabled') === 'true' && attr(byId.trashRecoverBtn, 'aria-busy') === null, 'recover is off, not busy');
  assert.ok(attr(byId.trashEmptyBtn, 'aria-disabled') === 'true' && attr(byId.trashEmptyBtn, 'aria-busy') === null, 'empty is off, not busy');
  byId.trashEraseBtn.fire('click');
  byId.trashEmptyBtn.fire('click');
  byId.trashRecoverBtn.fire('click');
  await flush();
  assert.strictEqual(modal.open, false, 'no second confirm opens while the erase is out');
  assert.ok(calls.filter((call) => call.url.startsWith('/app/settings/api/trash/')).length === 1, 'one act sent');
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [] };
  releaseErase();
  await flush(); await flush();
  assert.ok(TRASH_OPENER_IDS.every((id) => attr(byId[id], 'aria-disabled') === null && attr(byId[id], 'aria-busy') === null), 'the answer clears them all');

  // Where an act lands: the row after the first one it took, else the one
  // before, else the heading; nothing taken, the result line.
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [seg('a'), seg('c')] };
  await api.loadTrash();
  const landing = (act, chosen, order) => api.trashLandings(act, chosen.map((id) => ({ id })), order, true).map((el) => (el ? (el.dataset.trashId || el === byId.trashHeading && 'heading' || el === byId.trashResult && 'result' || el === byId.trashEmptyBtn && 'empty') : null)).join(' ');
  assert.strictEqual(landing('erase', ['b'], ['a', 'b', 'c']), 'c a heading', 'b gone: c next, a before');
  assert.strictEqual(landing('erase', ['x', 'b'], ['a', 'x', 'b', 'c']), 'c a heading', 'the first one taken decides, not the first chosen');
  assert.strictEqual(landing('recover', ['c', 'd'], ['a', 'c', 'd']), ' c heading', 'the last row taken: the one before it, a refused one included');
  assert.strictEqual(landing('erase', ['a'], ['a', 'c']), 'result heading', 'nothing taken: the result line');
  assert.strictEqual(landing('empty', [], ['a', 'c']), 'empty result heading', 'an empty that left rows: its button');
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [] };
  await api.loadTrash();
  assert.strictEqual(landing('empty', [], ['a', 'c']), 'heading', 'an empty that took everything: the heading');
  assert.strictEqual(api.trashLandings('erase', [{ id: 'a' }], ['a'], false).map((el) => el.id).join(' '), 'trashResult trashHeading', 'no list drawn: the result line');

  // Confirm wording follows count and kind.
  const K = (act, items) => { const copy = api.trashConfirmCopy(act, items); return { title: copy.title, paragraphs: Array.from(copy.paragraphs) }; };
  // A segment's erase changes nothing more of what was made from it (that was
  // made again at its delete); a memory's line is its own.
  assert.deepStrictEqual(K('erase', [seg('a')]).paragraphs, [C.bodyOne, C.madeOne]);
  assert.deepStrictEqual(K('erase', [seg('a'), seg('c')]).paragraphs, [C.bodyMany, C.madeMany]);
  assert.deepStrictEqual(K('erase', [mem('b')]).paragraphs, [C.bodyOne, C.staysOne, C.agentOne]);
  assert.strictEqual(K('empty', [seg('a')]).title, C.confirmEmpty);
  // An import or a workout in the selection: summaries and entities.
  assert.deepStrictEqual(K('erase', [imp('i')]).paragraphs, [C.bodyOne, C.madeOne]);
  assert.deepStrictEqual(K('erase', [imp('i'), seg('a')]).paragraphs, [C.bodyMany, C.madeMany]);
  assert.deepStrictEqual(K('erase', [{ kind: 'workout' }, seg('a')]).paragraphs, [C.bodyMany, C.madeMany]);

  // Entities, facets, the voices wipe and a merge's files: each row tells
  // itself apart from its own record, and says what recovering does.
  const ent = (id, extra = {}) => ({ id, kind: 'entity', label: 'Dana Ruiz', entity_type: 'Person', facets: 3, same_name: false, trashed_at: '2026-11-09T10:00:00Z', erase_at: LATER, bytes: 10, ...extra });
  const fac = (id, extra = {}) => ({ id, kind: 'facet', label: 'work 2024', name: 'work-2024', replaces: false, trashed_at: '2026-11-09T10:00:00Z', erase_at: LATER, bytes: 10, ...extra });
  const wipe = (id) => ({ id, kind: 'speakers', files: 12, trashed_at: '2026-11-05T10:00:00Z', erase_at: LATER, bytes: 5 * 1024 * 1024 });
  const merge = (id, state, title) => ({ id, kind: 'facet_file', dest: 'work', dest_state: state, dest_title: title, files: 4, trashed_at: '2026-11-05T10:00:00Z', erase_at: LATER, bytes: 99 });
  const rowOf = (item) => { answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [item] }; return api.loadTrash().then(() => byId.trashList.children[0]); };
  const lines = (row, cls) => row.all(cls).map((node) => node.textContent);
  {
    const row = await rowOf(ent('e1'));
    assert.strictEqual(row.all('removal-what')[0].textContent, api.TRASH_KIND_LABEL.entity);
    assert.deepStrictEqual(quals(row), ['Dana Ruiz', 'person'], 'an entity shows its name and its type, lowercased');
    assert.ok(row.textContent.includes('in 3 facets'));
    assert.deepStrictEqual(lines(row, 'trash-returned'), [], 'no other of its name: no line');
    assert.ok(!(await rowOf(ent('e0', { facets: 0 }))).textContent.includes('in 1 facet'), 'no facets: no count');
    assert.ok((await rowOf(ent('e2', { facets: ['a'] }))).textContent.includes('in 1 facet'));
    assert.deepStrictEqual(lines(await rowOf(ent('e3', { same_name: true })), 'trash-returned'), [C.entitySameName], 'another of its name: both will be there');
    const hidden = await rowOf(ent('e4', { hidden: true }));
    assert.deepStrictEqual(quals(hidden), ['Dana Ruiz', 'person', C.entityHidden], 'a hidden entity says so after its type');
    assert.deepStrictEqual(lines(hidden, 'trash-returned'), [C.entityHiddenBack], 'a hidden one comes back hidden');
    assert.deepStrictEqual(lines(await rowOf(ent('e5', { hidden: true, same_name: true })), 'trash-returned'), [C.entitySameName, C.entityHiddenBack], 'both lines, the same-name one first');
    assert.ok(!quals(row).includes(C.entityHidden), 'a visible one has no hidden mark');
  }
  {
    const row = await rowOf(fac('f1'));
    assert.strictEqual(row.all('removal-what')[0].textContent, api.TRASH_KIND_LABEL.facet);
    assert.deepStrictEqual(quals(row), ['work 2024'], 'a facet shows its title');
    assert.deepStrictEqual(lines(await rowOf(fac('f2', { replaces: true })), 'trash-returned'), [C.facetReplaces]);
  }
  {
    const row = await rowOf(wipe('w1'));
    assert.strictEqual(row.all('removal-what')[0].textContent, api.TRASH_KIND_LABEL.speakers);
    assert.deepStrictEqual(quals(row), [C.cleared({ day: '20261105' })], 'the wipe shows the day it was cleared');
    assert.ok(row.textContent.includes('5.0 MB'), 'and its size');
    assert.deepStrictEqual(lines(row, 'trash-returned'), [C.speakersReplaces, C.speakersKept], 'what recovering replaces, every time');
  }
  {
    const live = await rowOf(merge('m1', 'live', 'work'));
    assert.strictEqual(live.all('removal-what')[0].textContent, api.TRASH_KIND_LABEL.facet_file);
    assert.deepStrictEqual(quals(live), [C.mergedInto({ title: 'work' })]);
    assert.deepStrictEqual(lines(live, 'trash-returned'), [C.mergeLive({ title: 'work' })], 'a live destination: what recovering erases');
    const trashed = await rowOf(merge('m2', 'trashed', 'clients'));
    assert.deepStrictEqual(quals(trashed), [C.mergedInto({ title: 'clients' })]);
    assert.deepStrictEqual(lines(trashed, 'trash-returned'), [C.mergeTrashed({ title: 'clients' })], 'a destination in the trash: recover that first');
    const gone = await rowOf(merge('m3', 'gone', null));
    assert.deepStrictEqual(quals(gone), [], 'a gone destination has no qualifier');
    assert.deepStrictEqual(lines(gone, 'trash-returned'), [C.mergeGone]);
    const none = await rowOf(merge('m4', 'none', null));
    assert.strictEqual(none.all('removal-what')[0].textContent, api.TRASH_KIND_LABEL.facet_file_unfinished, 'no manifest: its own kind words');
    assert.deepStrictEqual(quals(none), []);
    assert.deepStrictEqual(lines(none, 'trash-returned'), [C.mergeNoManifest]);
    const tricky = await rowOf(merge('m5', 'live', "a$&b"));
    assert.ok(lines(tricky, 'trash-returned')[0].includes('a$&b'), 'a title is filled in as it is');
  }
  assert.strictEqual(api.trashKindMessage('recover', [ent('a')], 1), 'recovered 1 entity.');
  assert.strictEqual(api.trashKindMessage('recover', [ent('a'), ent('b')], 2), 'recovered 2 entities.');
  assert.strictEqual(api.trashKindMessage('recover', [fac('a'), fac('b')], 2), 'recovered 2 facets.');
  assert.strictEqual(api.trashKindMessage('recover', [wipe('a')], 1), 'recovered 1 thing.');
  assert.strictEqual(api.trashKindMessage('recover', [ent('a'), fac('b')], 2), 'recovered 2 things.');

  // Rows that stopped part way lead the list, with no box, and nothing counts them.
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [seg('a')], stopped: [{ ...ent('s1'), state: 'detaching' }, { ...fac('s2'), state: 'attaching' }, { ...seg('s3'), state: 'erasing' }] };
  await api.loadTrash();
  {
    const [first, second, third] = byId.trashList.children;
    assert.strictEqual(byId.trashList.children.length, 3, 'a stopped erase is not listed');
    assert.strictEqual(first.className, 'trash-row trash-row--stopped');
    assert.strictEqual(first.children[0].className, 'trash-row-gap', 'a spacer holds the column');
    assert.strictEqual(lines(first, 'trash-stopped')[0], C.stopped.detaching);
    assert.strictEqual(lines(second, 'trash-stopped')[0], C.stopped.attaching);
    assert.ok(!first.textContent.includes(C.erasesOn({ date: '' })), 'the sentence stands in place of the date');
    assert.strictEqual(third.children[0].tag, 'input');
    byId.trashSelectAll.fire('change');
    assert.deepStrictEqual([...api.trashSelected], ['a'], 'select all leaves stopped rows out');
  }
  answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [], stopped: [{ ...ent('s1'), state: 'attaching' }] };
  await api.loadTrash();
  assert.ok(!byId.trashList.children.some((row) => row.className === 'log-empty'), 'only stopped rows: the trash is not empty');
  assert.strictEqual(byId.trashBar.hidden, true, 'and there is nothing to act on');

  // A refused recover: the row says why, from the code and what it names.
  {
    const items = [ent('p1'), ent('p2'), wipe('w1'), fac('f1'), ent('p3'), merge('m1', 'gone', null), fac('f9')];
    answers['/app/settings/api/trash'] = { erase_after_days: 7, items };
    await api.loadTrash();
    byId.trashSelectAll.fire('change');
    answers['/app/settings/api/trash/recover'] = { done: [], failed: [
      { id: 'p1', reason: 'waits_facets', detail: { code: 'waits_facets', title: 'clients', others: 0 } },
      { id: 'p2', reason: 'waits_facets', detail: { code: 'waits_facets', title: 'clients', others: 2 } },
      { id: 'w1', reason: 'waits_wipe', detail: { code: 'waits_wipe', segments: 3, imports: 0, people: 2, name: 'Ada' } },
      { id: 'f1', reason: 'name_redirected', detail: { code: 'name_redirected' } },
      { id: 'p3', reason: 'taken', detail: { code: 'taken' } },
      { id: 'm1', reason: 'merge_dest_missing', detail: { code: 'merge_dest_missing' } },
      { id: 'f9', reason: 'something the page has no words for' },
    ] };
    byId.trashRecoverBtn.fire('click');
    await flush();
    const N = C.notes;
    const note = (id) => lines(byId.trashList.children[items.findIndex((item) => item.id === id)], 'trash-row-note');
    assert.deepStrictEqual(note('p1'), [N.waitsFacetNone({ title: 'clients' })]);
    assert.deepStrictEqual(note('p2'), [N.waitsFacetMany({ title: 'clients', n: 2 })], 'the count is the rest');
    assert.deepStrictEqual(note('w1'), [`${counted('trash.refusal.segments.count', 'segments', 3)} ${N.waitsPersonOne({ name: 'Ada' })}`]);
    assert.deepStrictEqual(note('f1'), [N.redirected]);
    assert.deepStrictEqual(note('p3'), [N.taken]);
    assert.deepStrictEqual(note('m1'), [], "a merge's row already says why");
    assert.deepStrictEqual(note('f9'), [], 'an unknown code adds no note');
    assert.ok(!JSON.stringify(byId.trashList).includes('has no words'), 'no server reason shows');
    assert.strictEqual(byId.trashResult.textContent, C.recoverFailedMany, 'something here could change: try again');
    answers['/app/settings/api/trash/recover'] = { done: [], failed: [{ id: 'p3', reason: 'taken' }, { id: 'f1', reason: 'name_redirected' }] };
    byId.trashSelectAll.fire('change');
    byId.trashRecoverBtn.fire('click');
    await flush();
    assert.strictEqual(byId.trashResult.textContent, C.recoverStuckMany, 'nothing here can change: no try again');
    assert.deepStrictEqual(note('p1'), [], 'notes are from the last act only');
  }
  {
    const N = C.notes;
    const wipeNote = (detail) => api.trashRowNote({ reason: 'waits_wipe', detail });
    assert.strictEqual(wipeNote({ segments: 1, imports: 0, people: 0 }), counted('trash.refusal.segments.count', 'segments', 1));
    assert.strictEqual(wipeNote({ segments: 0, imports: 1, people: 0 }), counted('trash.refusal.imports.count', 'imports', 1));
    assert.strictEqual(wipeNote({ segments: 0, imports: 2, people: 0 }), counted('trash.refusal.imports.count', 'imports', 2));
    assert.strictEqual(wipeNote({ segments: 2, imports: 1, people: 0 }), N.waitsBoth);
    assert.strictEqual(wipeNote({ segments: 0, imports: 0, people: 1, name: 'Bo' }), N.waitsPersonNone({ name: 'Bo' }));
    assert.strictEqual(wipeNote({ segments: 0, imports: 0, people: 4, name: 'Bo' }), N.waitsPersonMany({ name: 'Bo', n: 3 }));
    assert.strictEqual(api.trashRowNote({ reason: 'busy' }), N.busy);
    assert.strictEqual(api.trashRowNote({ reason: 'waits_dest', detail: { title: 'work' } }), null);
    assert.strictEqual(api.trashRowNote({ reason: 'waits_facets', detail: {} }), null, 'nothing to name: no note');
  }

  // Originals released from a segment: named by what was released, read as
  // their segment's row, and say what recovering does by where it stands.
  {
    const orig = (id, extra = {}) => ({ id, kind: 'originals', day: '20261110', stream: 'desk', segment: '070000_300', modalities: ['audio', 'screen'], segment_state: 'live', trashed_at: '2026-11-10T10:00:00Z', erase_at: LATER, bytes: 5 * 1024 * 1024, surface: 'home', ...extra });
    const L = api.TRASH_KIND_LABEL;
    const both = await rowOf(orig('o1'));
    assert.strictEqual(both.all('removal-what')[0].textContent, L.originals);
    assert.deepStrictEqual(quals(both), ['20261110', 'time:070000_300', 'label:desk'], 'its day, time range and stream, as a segment row');
    assert.ok(both.textContent.includes('5.0 MB'), 'and its size');
    assert.deepStrictEqual(lines(both, 'trash-returned'), [C.originalsLive], 'a live segment: what recovering does');
    assert.strictEqual((await rowOf(orig('o2', { modalities: ['audio'] }))).all('removal-what')[0].textContent, L.originals_audio);
    assert.strictEqual((await rowOf(orig('o3', { modalities: ['screen'] }))).all('removal-what')[0].textContent, L.originals_screen);
    assert.strictEqual((await rowOf(orig('o4', { modalities: undefined }))).all('removal-what')[0].textContent, L.originals, 'not given: both');
    assert.deepStrictEqual(lines(await rowOf(orig('o5', { segment_state: 'trashed' })), 'trash-returned'), [C.originalsSegmentTrashed]);
    assert.deepStrictEqual(lines(await rowOf(orig('o6', { segment_state: 'gone' })), 'trash-returned'), [C.originalsSegmentGone]);
    assert.deepStrictEqual(lines(await rowOf(orig('o7', { segment_state: undefined })), 'trash-returned'), [], 'no state: no line');
    assert.strictEqual(api.trashKindMessage('recover', [orig('a')], 1), 'recovered 1 thing.');
    assert.strictEqual(api.trashKindMessage('recover', [orig('a'), orig('b')], 2), 'recovered 2 things.');

    // A stopped one reads as part 2a's stopped rows.
    answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [], stopped: [{ ...orig('s1', { modalities: ['screen'] }), state: 'detaching' }] };
    await api.loadTrash();
    const stoppedRow = byId.trashList.children[0];
    assert.strictEqual(stoppedRow.className, 'trash-row trash-row--stopped');
    assert.strictEqual(stoppedRow.all('removal-what')[0].textContent, L.originals_screen);
    assert.deepStrictEqual(lines(stoppedRow, 'trash-stopped'), [C.stopped.detaching]);
    assert.deepStrictEqual(lines(stoppedRow, 'trash-returned'), []);

    // A recover its segment refuses: the row that already says why takes no note,
    // one whose view didn't carry the state does, and trying again won't help.
    const items = [orig('r1', { segment_state: 'trashed' }), orig('r2', { segment_state: undefined }), orig('r3', { segment_state: 'gone' })];
    answers['/app/settings/api/trash'] = { erase_after_days: 7, items };
    await api.loadTrash();
    byId.trashSelectAll.fire('change');
    answers['/app/settings/api/trash/recover'] = { done: [], failed: [
      { id: 'r1', reason: 'recording_in_trash', detail: { code: 'recording_in_trash' } },
      { id: 'r2', reason: 'recording_in_trash', detail: { code: 'recording_in_trash' } },
      { id: 'r3', reason: 'segment_gone', detail: { code: 'segment_gone' } },
    ] };
    byId.trashRecoverBtn.fire('click');
    await flush();
    const note = (id) => lines(byId.trashList.children[items.findIndex((item) => item.id === id)], 'trash-row-note');
    assert.deepStrictEqual(note('r1'), [], 'its line already says its segment is in the trash');
    assert.deepStrictEqual(note('r2'), [C.notes.recordingInTrash]);
    assert.deepStrictEqual(note('r3'), [], 'a gone segment is said by the row');
    assert.strictEqual(byId.trashResult.textContent, C.recoverStuckMany, 'no try again');

    // The erase confirm adds what stays when an originals item is picked.
    assert.deepStrictEqual(K('erase', [orig('a')]).paragraphs, [C.bodyOne, C.originalsStaysOne]);
    assert.deepStrictEqual(K('erase', [orig('a'), orig('b')]).paragraphs, [C.bodyMany, C.originalsStaysMany]);
    assert.deepStrictEqual(K('erase', [orig('a'), seg('b')]).paragraphs, [C.bodyMany, C.madeMany, C.originalsStaysMany], 'after the line the other kinds take');
    assert.deepStrictEqual(K('erase', [orig('a'), ent('b')]).paragraphs, [C.bodyMany, C.mentionsMany, C.originalsStaysMany]);
  }

  // A segment its import's file still holds: the row says where that import is,
  // a refused recover's note is only for a row that doesn't, and erasing the
  // import says its waiting segments go too.
  {
    const own = (id, state, extra = {}) => seg(id, { import: { id: '20261110_090000', label: 'standup.m4a', state }, ...extra });
    assert.deepStrictEqual(lines(await rowOf(own('i1', 'trashed')), 'trash-returned'), [C.importTrashedLabel({ label: 'standup.m4a' })]);
    assert.deepStrictEqual(lines(await rowOf(own('i2', 'trashed', { import: { id: 'x', label: null, state: 'trashed' } })), 'trash-returned'), [C.importTrashed]);
    assert.deepStrictEqual(lines(await rowOf(own('i3', 'gone')), 'trash-returned'), [C.importGone]);
    assert.deepStrictEqual(lines(await rowOf(own('i4', 'live')), 'trash-returned'), [], 'its import is in the journal: nothing new');
    const items = [own('r1', 'trashed'), seg('r2'), own('r3', 'gone')];
    answers['/app/settings/api/trash'] = { erase_after_days: 7, items };
    await api.loadTrash();
    byId.trashSelectAll.fire('change');
    answers['/app/settings/api/trash/recover'] = { done: [], failed: [
      { id: 'r1', reason: 'import_in_trash', detail: { code: 'import_in_trash' } },
      { id: 'r2', reason: 'import_in_trash', detail: { code: 'import_in_trash' } },
      { id: 'r3', reason: 'import_gone', detail: { code: 'import_gone' } },
    ] };
    byId.trashRecoverBtn.fire('click');
    await flush();
    const note = (id) => lines(byId.trashList.children[items.findIndex((item) => item.id === id)], 'trash-row-note');
    assert.deepStrictEqual(note('r1'), [], 'its line already says its import is in the trash');
    assert.deepStrictEqual(note('r2'), [C.notes.importInTrash]);
    assert.deepStrictEqual(note('r3'), [], 'a gone import is said by the row');
    assert.strictEqual(byId.trashResult.textContent, C.recoverStuckMany, 'no try again');
    const upload = { id: 'u1', kind: 'import', source: 'audio', import_id: '20261110_090000', label: 'standup.m4a', imported_day: '20261110', segments: 1, days: ['20261110', '20261110'], trashed_at: '2026-11-10T10:00:00Z', erase_at: LATER, bytes: 1024, surface: 'transcripts', takes: 2 };
    answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [upload, own('w1', 'trashed'), own('w2', 'trashed')] };
    await api.loadTrash();
    assert.ok(K('erase', [upload]).paragraphs.includes(C.waitingOwnOneMany({ w: 2 })));
    assert.ok(!K('erase', [upload, own('w1', 'trashed')]).paragraphs.includes(C.waitingOwnOneMany({ w: 2 })), 'a picked segment is listed already');
    assert.ok(K('erase', [upload, own('w1', 'trashed')]).paragraphs.includes(C.waitingOwnOneOne()));
  }

  // A facet recovered over another of its name confirms first; nothing else does.
  {
    answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [fac('f1', { replaces: true }), ent('e1'), fac('f2')] };
    await api.loadTrash();
    calls.length = 0;
    byId.trashList.children[0].children[0].checked = true;
    byId.trashList.children[0].children[0].fire('change');
    byId.trashRecoverBtn.fire('click');
    assert.strictEqual(modal.open, true, 'the replace confirm opens');
    assert.strictEqual(calls.length, 0, 'nothing is recovered before the confirm');
    assert.strictEqual(byId.trashConfirmHeading.textContent, C.replaceTitle);
    assert.deepStrictEqual(byId.trashConfirmBody.children.map((p) => p.textContent), [C.replaceOne]);
    assert.strictEqual(byId.trashConfirmErase.textContent, C.replaceConfirm);
    byId.trashConfirmCancel.fire('click');
    assert.strictEqual(calls.length, 0);
    const K2 = (items) => Array.from(api.trashConfirmCopy('replace', items).paragraphs);
    assert.deepStrictEqual(K2([fac('a', { replaces: true }), ent('b')]), [C.replaceSomeOne]);
    assert.deepStrictEqual(K2([fac('a', { replaces: true }), fac('b', { replaces: true })]), [C.replaceSomeMany({ k: 2 })]);
    byId.trashSelectAll.fire('change');
    byId.trashRecoverBtn.fire('click');
    answers['/app/settings/api/trash/recover'] = { done: ['f1', 'e1', 'f2'], failed: [] };
    answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [] };
    byId.trashConfirmErase.fire('click');
    await flush();
    const recover = calls.find((call) => call.url === '/app/settings/api/trash/recover');
    assert.deepStrictEqual(JSON.parse(recover.options.body).ids, ['f1', 'e1', 'f2'], 'the confirm recovers');
    answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [ent('e1'), ent('e2', { same_name: true })] };
    await api.loadTrash();
    byId.trashSelectAll.fire('change');
    modal.open = false;
    answers['/app/settings/api/trash/recover'] = { done: ['e1', 'e2'], failed: [] };
    byId.trashRecoverBtn.fire('click');
    assert.strictEqual(modal.open, false, 'people keep both, so no confirm');
    await flush();
    answers['/app/settings/api/trash'] = { erase_after_days: 7, items: [seg('a')] };
    await api.loadTrash();
    byId.trashSelectAll.fire('change');
    byId.trashEraseBtn.fire('click');
    assert.strictEqual(byId.trashConfirmErase.textContent, C.eraseNow, 'the erase confirm keeps its own button');
    byId.trashConfirmCancel.fire('click');
  }

  // The erase confirm's what-stays lines follow what's picked.
  assert.deepStrictEqual(K('erase', [ent('a')]).paragraphs, [C.bodyOne, C.mentionsOne]);
  assert.deepStrictEqual(K('erase', [fac('a')]).paragraphs, [C.bodyOne, C.mentionsOne, C.sortedOne]);
  assert.deepStrictEqual(K('erase', [fac('a'), seg('b')]).paragraphs, [C.bodyMany, C.mentionsMany, C.sortedMany]);
  assert.deepStrictEqual(K('erase', [ent('a'), imp('b')]).paragraphs, [C.bodyMany, C.mentionsMany], 'an entity or facet picked takes the mentions line');
  assert.deepStrictEqual(K('erase', [wipe('a')]).paragraphs, [C.bodyOne], 'the wipe alone adds none');
  assert.deepStrictEqual(K('erase', [wipe('a'), merge('b', 'live', 'w')]).paragraphs, [C.bodyMany], "nor with a merge's files");
  assert.deepStrictEqual(K('erase', [wipe('a'), seg('b')]).paragraphs, [C.bodyMany, C.madeMany]);
  assert.deepStrictEqual(K('empty', [ent('a'), seg('b')]).paragraphs.slice(1), [C.mentionsMany]);

  // Results: what the answer says, checked against the reloaded list.
  const items = [seg('a'), seg('c')];
  const line = (act, answer, listed) => api.trashResultLine(act, items, answer, listed);
  const failA = { id: 'a', reason: 'busy' };
  assert.ok(line('recover', { done: ['a', 'c'], failed: [] }, new Set()).length > 0);
  assert.strictEqual(line('recover', { done: ['c'], failed: [failA] }, new Set(['a'])), C.partialRecoverOne({ done: 1, n: 2 }));
  assert.strictEqual(line('erase', { done: [], failed: [failA] }, new Set(['a'])), C.eraseFailedOne);
  assert.strictEqual(line('recover', { done: [], failed: [failA, { id: 'c' }] }, new Set(['a', 'c'])), C.recoverFailedMany);
  assert.strictEqual(line('empty', { done: [], failed: [failA] }, new Set(['a'])), C.emptyFailed);
  // A failed id no longer listed isn't "still in the trash", and no server reason shows.
  assert.strictEqual(line('erase', { done: [], failed: [failA] }, new Set()), C.goneOne());
  assert.strictEqual(line('erase', { done: [], failed: [failA, { id: 'c' }] }, new Set()), C.goneMany());
  assert.strictEqual(line('recover', { done: ['c'], failed: [failA] }, new Set()), `recovered 1 segment. ${C.goneMoreOne()}`);
  // No answer and no list: nothing is claimed.
  assert.strictEqual(line('erase', null, null), '');
  assert.strictEqual(line('erase', { done: [], failed: [failA] }, null), '');

  // The setting: out of range shows the error and saves nothing.
  const input = byId.trashEraseAfterDays;
  for (const bad of ['0', '31', '1.5', 'x', '']) {
    input.value = bad;
    api.saveTrashDays(input);
  }
  assert.strictEqual(saves.length, 0);
  assert.strictEqual(statuses.length, 5);
  assert.ok(statuses.every((status) => status.type === 'error' && status.message === C.range));
  input.value = '30';
  api.saveTrashDays(input);
  assert.deepStrictEqual(saves[0], { section: 'trash', key: 'erase_after_days', value: 30 });
  // The same number saves again: a failed save can be retried.
  api.saveTrashDays(input);
  assert.strictEqual(saves.length, 2);

  // History rows read by event; a timer erase reads as its own line.
  const rows = journalContext({
    document,
    window: { JournalFormat: { day: (key) => `d${key}` } },
  });
  vm.runInContext(`${history}
    this.removalHow = removalHow; this.removalWhat = removalWhat;
    this.removalGroupWhat = removalGroupWhat; this.REMOVAL_SOURCE = REMOVAL_SOURCE;`, rows);
  const web = { surface: 'web', page: 'settings' };
  assert.strictEqual(rows.removalHow({ ...web, event: 'trashed' }), 'moved to the trash in settings');
  assert.strictEqual(rows.removalHow({ surface: 'cli', event: 'recovered' }), 'recovered from the command line');
  assert.strictEqual(rows.removalHow({ surface: 'policy', policy: 'trash', event: 'erased' }), 'erased on its date');
  assert.strictEqual(rows.removalHow({ ...web, event: 'removed', action: 'segment_delete' }), 'deleted in settings');
  assert.strictEqual(rows.removalHow({ ...web, event: 'recovered' }, { erasing: true, except: true }),
    'recovered in settings, erasing what was in the way, except notes in facets no longer in your journal');

  // An import's line keeps its source and arrival day without its file name.
  const historyLine = (unit) => {
    const node = rows.removalWhat(unit);
    return { what: node.all('removal-what').map((n) => n.textContent), quals: node.all('removal-qual').map((n) => n.textContent) };
  };
  const importLine = historyLine({ kind: 'import', source: 'chatgpt', id: '20261102_161200', file: 'secret.zip' });
  assert.deepStrictEqual(importLine.what, ['import']);
  assert.strictEqual(importLine.quals.length, 2);
  assert.strictEqual(importLine.quals[0], rows.REMOVAL_SOURCE.chatgpt);
  assert.ok(importLine.quals[1].includes('d20261102'));
  assert.ok(!JSON.stringify(importLine).includes('secret'), 'the history keeps nothing that was in it');
  assert.ok(historyLine({ kind: 'import', source: '', id: '20261102_161200' }).quals.length === 1, 'a source-less import shows no empty qualifier');
  assert.ok(historyLine({ kind: 'import', source: 'toString', id: 'bad' }).quals.length === 0);
  const workoutLine = historyLine({ kind: 'workout', source: 'strava', id: 'w1' });
  assert.deepStrictEqual(workoutLine.what, ['workout']);
  assert.deepStrictEqual(workoutLine.quals, [rows.REMOVAL_SOURCE.strava], 'without its day a workout reads as its kind and source');
  assert.deepStrictEqual(historyLine({ kind: 'workout', source: 'strava', day: '20261108' }).quals, ['d20261108', rows.REMOVAL_SOURCE.strava]);
  const group = rows.removalGroupWhat({ units: [{ unit: { kind: 'import', source: 'chatgpt', id: '20261102_161200' } }, { unit: { kind: 'import', source: 'document', id: '20261101_094000' } }] });
  assert.ok(group.all('removal-what')[0].textContent.endsWith('imports'), 'a group of imports keeps the plural kind');

  // Originals released from a segment read as original media with the day and
  // time range, and a group counts segments, not files. Older rows keep theirs.
  {
    const unit = { kind: 'originals', day: '20261110', stream: 'desk', segment: '070000_300', files: ['070000_300.flac', 'screen.webm'] };
    const one = historyLine(unit);
    assert.deepStrictEqual(one.what, ['original media']);
    assert.strictEqual(one.quals.length, 2);
    assert.strictEqual(one.quals[0], 'd20261110');
    assert.ok(!JSON.stringify(one).includes('.flac') && !JSON.stringify(one).includes('desk'), 'no file name and no stream');
    const group = rows.removalGroupWhat({ units: [{ unit }, { unit: { ...unit, segment: '080000_300' } }, { unit: { ...unit, segment: '090000_300' } }] });
    assert.deepStrictEqual(group.all('removal-what').map((n) => n.textContent), ['original media from 3 segments']);
    assert.deepStrictEqual(group.all('removal-qual').map((n) => n.textContent), ['d20261110'], 'the day they share');
    const old = historyLine({ kind: 'original', file: '120000_300.m4a', day: '20261105', segment: '120000_300' });
    assert.deepStrictEqual(old.what, ['original media']);
    assert.strictEqual(old.quals[0], '120000_300.m4a', 'an older row keeps its file name');
    const oldGroup = rows.removalGroupWhat({ units: [{ unit: { kind: 'original', day: '20261105' } }, { unit: { kind: 'original', day: '20261105' } }] });
    assert.ok(oldGroup.all('removal-what')[0].textContent.endsWith('originals'), 'an older group keeps its count and plural');
  }

  // A merge's files have a label of their own and no name to keep.
  const mergeLine = historyLine({ kind: 'facet_file' });
  assert.deepStrictEqual(mergeLine.what, ['files from a facet merge']);
  assert.deepStrictEqual(mergeLine.quals, [], "it isn't one of the nameless kinds");
  assert.deepStrictEqual(historyLine({ kind: 'entity' }).quals.length, 1, 'an entity keeps its name-not-kept qualifier');

  // What a recover erased in its way folds into the recover's how-line and
  // leaves the group's count.
  const fold = journalContext({});
  vm.runInContext(`
    function removalHow(entry, fold) {
      const detail = fold?.erasing && fold.except ? '+BOTH' : fold?.erasing ? '+ERASING' : fold?.except ? '+EXCEPT' : '';
      return 'HOW:' + entry.event + detail;
    }
    ${html.slice(html.indexOf('function removalGroups(entries)'), html.indexOf('function removalGroupWhat'))}
    this.removalGroups = removalGroups; this.removalGroupHow = removalGroupHow;
  `, fold);
  const at = (event, kind, operation) => ({ event, operation, unit: { kind } });
  const groups = (entries) => Array.from(fold.removalGroups(entries));
  {
    const [one] = groups([at('erased', 'facet', 'op1'), at('recovered', 'facet', 'op1')]);
    assert.strictEqual(one.units.length, 1, 'one facet recovered over another counts one');
    assert.strictEqual(one.units[0].event, 'recovered');
    assert.strictEqual(fold.removalGroupHow(one), 'HOW:recovered+ERASING');
    const [person] = groups([at('recovered', 'entity', 'op2'), at('erased', 'entity', 'op2')]);
    assert.strictEqual(fold.removalGroupHow(person), 'HOW:recovered+EXCEPT');
    const [both] = groups([at('recovered', 'facet', 'op3'), at('erased', 'facet', 'op3'), at('recovered', 'entity', 'op3'), at('erased', 'entity', 'op3')]);
    assert.strictEqual(both.units.length, 2);
    assert.strictEqual(fold.removalGroupHow(both), 'HOW:recovered+BOTH');
    const [wipeBack] = groups([at('recovered', 'speakers', 'op4'), at('erased', 'speakers', 'op4')]);
    assert.strictEqual(fold.removalGroupHow(wipeBack), 'HOW:recovered+ERASING');
    const [plain] = groups([at('erased', 'segment', 'op5'), at('erased', 'segment', 'op5')]);
    assert.strictEqual(plain.units.length, 2, 'an erase on its own is not folded');
    assert.strictEqual(fold.removalGroupHow(plain), 'HOW:erased');
    const [loose] = groups([at('recovered', 'facet', undefined)]);
    assert.strictEqual(fold.removalGroupHow(loose), 'HOW:recovered');
    // A set moved and erased at once records both under its one operation:
    // its erases and its moves are each a row, so neither counts twice.
    const set = groups([at('erased', 'entity', 'op6'), at('trashed', 'entity', 'op6'), at('erased', 'entity', 'op6'), at('trashed', 'entity', 'op6')]);
    assert.deepStrictEqual(set.map((group) => [fold.removalGroupHow(group), group.units.length]), [['HOW:erased', 2], ['HOW:trashed', 2]]);
    const partly = groups([at('erased', 'entity', 'op7'), at('trashed', 'entity', 'op7'), at('trashed', 'entity', 'op7')]);
    assert.deepStrictEqual(partly.map((group) => group.units.length), [1, 2], 'one whose erase was refused is still counted as moved');
  }

  // The backups line: can't finish warns; where an older backup still holds
  // it, the part words show with a plain "why", never the stays words.
  {
    const B = C.backups;
    const line = byId.trashBackupsLine;
    const backups = (state, reason) => ({ set_up: true, available: true, setting: false, state, reason, pending: 1, staying: 1 });
    api.renderTrashBackups(backups('stays', 'elsewhere'));
    assert.ok(line.classList.contains('trash-backups-warn'), 'a part state warns');
    assert.ok(line.textContent.startsWith(counted('trash.backup.partly.erased.count', 'erased', 1)), 'the part words, not the stays words');
    assert.ok(line.textContent.includes(B.part.elsewhere));
    assert.strictEqual(line.children[1].children[0].textContent, B.whyOnly, 'nothing to do, so the summary is only why');
    api.renderTrashBackups({ ...backups('stays', 'elsewhere'), staying: 3, partly: 1, keeps: 'versions' });
    assert.ok(line.textContent.startsWith(counted('trash.backup.partly.erased.count', 'erased', 1)), 'the part words count only what an older backup holds');
    assert.ok(line.textContent.includes(B.own.versions), 'the rest are said as what the storage keeps');
    api.renderTrashBackups(backups('stays', 'versions'));
    assert.strictEqual(line.children[1].children[0].textContent, B.why);
    api.renderTrashBackups(backups('running', null));
    assert.ok(!line.classList.contains('trash-backups-warn'), 'a running removal does not warn');
    api.renderTrashBackups({ set_up: false });
    assert.ok(line.hidden && byId.trashBackupsField.hidden, 'nothing shows while backup is not set up');
  }

  // A row from the backup half names backups in its action, so it adds no place word.
  {
    for (const action of ['backup_erase', 'backup_erase_shared', 'backup_erase_stays', 'backup_erase_partly']) {
      const record = rows.removalHow({ action, surface: 'web', page: 'backup' });
      assert.ok(record.startsWith('removed from your backups'), action);
      assert.ok(!record.includes(' in backup'), action);
    }
  }
})().catch((error) => { console.error(error); process.exit(1); });
