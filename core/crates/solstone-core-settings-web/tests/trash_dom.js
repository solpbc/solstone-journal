// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The trash page's states and acts. Asserts which copy key reaches which element
// and which state renders; no assertion pins the words.
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
const html = fs.readFileSync(path.join(crateDir, 'assets/workspace.html'), 'utf8');
const start = html.indexOf('// ========== TRASH ==========');
const end = html.indexOf('// ========== END TRASH ==========');
assert.ok(start > 0 && end > start, 'the trash block is marked');
const history = html.slice(html.indexOf('const REMOVAL_EVENT'), html.indexOf('function removalGroups'));
const sourceNames = html.slice(html.indexOf('// ========== SOURCE NAMES =========='), html.indexOf('// ========== END SOURCE NAMES =========='));
assert.ok(sourceNames.includes('REMOVAL_SOURCE'), 'the source names block is marked');

class El {
  constructor(tag) { this.tag = tag; this.children = []; this.dataset = {}; this.style = {}; this.hidden = false; this.listeners = {}; this.className = ''; this._text = ''; }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return this._text + this.children.map((child) => child.textContent).join(''); }
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren(...nodes) { this.children = nodes; this._text = ''; }
  addEventListener(name, fn) { this.listeners[name] = fn; }
  fire(name, event = {}) { return this.listeners[name]({ currentTarget: this, target: this, ...event }); }
  focus() { document.activeElement = this; }
  setAttribute(name, value) { (this.attributes ||= {})[name] = String(value); }
  all(cls) { const out = []; for (const child of this.children) { if (child.className === cls) out.push(child); if (child.all) out.push(...child.all(cls)); } return out; }
}
const ids = ['trashBar', 'trashBarIdle', 'trashBarSome', 'trashCount', 'trashSelectAll', 'trashSelectAllLabel', 'trashEmptyBtn', 'trashRecoverBtn', 'trashEraseBtn', 'trashResult', 'trashList', 'trashEraseAfterDays', 'trashHeading', 'trashConfirmHeading', 'trashConfirmBody', 'trashConfirmCancel', 'trashConfirmErase', 'trashConfirmModal'];
const byId = Object.fromEntries(ids.map((id) => [id, new El('div')]));
const modal = byId.trashConfirmModal;
modal._openAccessible = () => { modal.open = true; };
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
};
vm.createContext(sandbox);
vm.runInContext(`${sourceNames}\n${html.slice(start, end)};this.api={TRASH_COPY,TRASH_WORDS,renderTrash,loadTrash,trashEraseDate,trashResultLine,trashConfirmCopy,setupTrash,saveTrashDays,trashSelected,trashWord,trashRowNote,REMOVAL_SOURCE,REMOVAL_IMPORTED,TRASH_KIND_LABEL};`, sandbox);
const { api } = sandbox;
const C = api.TRASH_COPY;
api.setupTrash();

const LATER = '2099-11-15T00:00:00.000Z';
const seg = (id, extra = {}) => ({ id, kind: 'segment', day: '20261101', stream: 'field', segment: '070000_300', erase_at: LATER, bytes: 2048, replaces: false, ...extra });
const mem = (id) => ({ id, kind: 'memory', day: '20261101', stream: 'agent-memory-x', segment: '080000_1', erase_at: LATER, bytes: 10, replaces: false });
const imp = (id, extra = {}) => ({ id, kind: 'import', source: 'chatgpt', import_id: '20261102_161200', label: 'conversations.zip', imported_day: '20261102', segments: 12, days: ['20261005', '20261009'], erase_at: LATER, bytes: 3 * 1024 * 1024, replaces: false, ...extra });
const flush = () => new Promise((resolve) => setImmediate(resolve));

(async () => {
  // Error is never the empty line.
  answers['api/trash'] = new Error('down');
  assert.strictEqual(await api.loadTrash(), false);
  assert.strictEqual(byId.trashList.children[0].textContent, C.error);
  assert.strictEqual(byId.trashBar.hidden, true);
  assert.strictEqual(api.renderTrash({ not: 'a list' }), false);

  // Empty.
  answers['api/trash'] = { erase_after_days: 7, items: [] };
  await api.loadTrash();
  assert.strictEqual(byId.trashList.children[0].textContent, C.empty);
  assert.strictEqual(byId.trashBar.hidden, true);
  assert.strictEqual(byId.trashEraseAfterDays.value, '7');
  assert.strictEqual(byId.trashEraseAfterDays.__lastKnownValue, '7');
  assert.strictEqual(byId.trashEraseAfterDays.disabled, false);

  // Rows: a segment names its stream and size, a memory does neither.
  answers['api/trash'] = { erase_after_days: 7, items: [seg('a', { replaces: true }), mem('b')] };
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
  answers['api/trash'] = { erase_after_days: 7, items: [imp('i', { replaces: true }), imp('j', { label: null, source: 'not-a-source', segments: 1 })] };
  await api.loadTrash();
  const [impRow, bareRow] = byId.trashList.children;
  const quals = (row) => row.all('removal-qual').map((node) => node.textContent);
  assert.strictEqual(impRow.all('removal-what')[0].textContent, api.TRASH_KIND_LABEL.import);
  assert.deepStrictEqual(quals(impRow), [api.REMOVAL_SOURCE.chatgpt, 'conversations.zip', api.REMOVAL_IMPORTED.replace('{day}', '20261102')]);
  assert.ok(impRow.textContent.includes(C.segmentMany.replace('{n}', 12)));
  assert.ok(impRow.textContent.includes('3.0 MB'));
  assert.ok(impRow.textContent.includes(C.returnedImport));
  assert.ok(!impRow.textContent.includes(C.returned));
  assert.deepStrictEqual(quals(bareRow), [api.REMOVAL_IMPORTED.replace('{day}', '20261102')], 'no file name and an unknown source leave no empty qualifier');
  assert.ok(bareRow.textContent.includes(C.segmentOne.replace('{n}', 1)));
  assert.strictEqual(api.trashWord([imp('i')], 1), api.TRASH_WORDS.import[0]);
  assert.strictEqual(api.trashWord([imp('i'), imp('j')], 2), api.TRASH_WORDS.import[1]);
  assert.ok(!Object.values(api.TRASH_WORDS).flat().includes(api.trashWord([imp('i'), seg('a')], 2)), 'a mixed selection takes neither kind\'s word');
  assert.strictEqual(api.trashWord([{ kind: 'workout' }], 1), api.TRASH_WORDS.workout[0]);
  answers['api/trash'] = { erase_after_days: 7, items: [seg('a', { replaces: true }), mem('b')] };
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
  answers['api/trash/recover'] = { done: ['a', 'b'], failed: [] };
  answers['api/trash'] = { erase_after_days: 7, items: [] };
  byId.trashRecoverBtn.fire('click');
  await flush();
  const recover = calls.find((call) => call.url === 'api/trash/recover');
  assert.deepStrictEqual(JSON.parse(recover.options.body).ids, ['a', 'b']);
  assert.strictEqual(modal.open, undefined);
  assert.strictEqual(byId.trashResult.hidden, false);

  // Erase now always confirms, and only the confirm acts.
  answers['api/trash'] = { erase_after_days: 7, items: [seg('a')] };
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
  answers['api/trash/erase'] = { done: ['a'], failed: [] };
  answers['api/trash'] = { erase_after_days: 7, items: [] };
  byId.trashConfirmErase.fire('click');
  await flush();
  assert.ok(calls.some((call) => call.url === 'api/trash/erase'));

  // Empty confirms too, and posts no selection.
  answers['api/trash'] = { erase_after_days: 7, items: [seg('a')] };
  await api.loadTrash();
  calls.length = 0;
  byId.trashEmptyBtn.fire('click');
  assert.strictEqual(modal.open, true);
  answers['api/trash/empty'] = { done: ['a'], failed: [] };
  byId.trashConfirmErase.fire('click');
  await flush();
  assert.strictEqual(calls.find((call) => call.url === 'api/trash/empty').options.body, undefined);

  // Confirm wording follows count and kind.
  const K = (act, items) => { const copy = api.trashConfirmCopy(act, items); return { title: copy.title, paragraphs: Array.from(copy.paragraphs) }; };
  assert.deepStrictEqual(K('erase', [seg('a')]).paragraphs, [C.bodyOne, C.staysOne]);
  assert.deepStrictEqual(K('erase', [seg('a'), seg('c')]).paragraphs, [C.bodyMany, C.staysMany]);
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
  const rowOf = (item) => { answers['api/trash'] = { erase_after_days: 7, items: [item] }; return api.loadTrash().then(() => byId.trashList.children[0]); };
  const lines = (row, cls) => row.all(cls).map((node) => node.textContent);
  {
    const row = await rowOf(ent('e1'));
    assert.strictEqual(row.all('removal-what')[0].textContent, api.TRASH_KIND_LABEL.entity);
    assert.deepStrictEqual(quals(row), ['Dana Ruiz', 'person'], 'an entity shows its name and its type, lowercased');
    assert.ok(row.textContent.includes(C.facetMany.replace('{n}', 3)));
    assert.deepStrictEqual(lines(row, 'trash-returned'), [], 'no other of its name: no line');
    assert.ok(!(await rowOf(ent('e0', { facets: 0 }))).textContent.includes(C.facetOne), 'no facets: no count');
    assert.ok((await rowOf(ent('e2', { facets: ['a'] }))).textContent.includes(C.facetOne));
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
    assert.deepStrictEqual(quals(row), [C.cleared.replace('{day}', '20261105')], 'the wipe shows the day it was cleared');
    assert.ok(row.textContent.includes('5.0 MB'), 'and its size');
    assert.deepStrictEqual(lines(row, 'trash-returned'), [C.speakersReplaces, C.speakersKept], 'what recovering replaces, every time');
  }
  {
    const live = await rowOf(merge('m1', 'live', 'work'));
    assert.strictEqual(live.all('removal-what')[0].textContent, api.TRASH_KIND_LABEL.facet_file);
    assert.deepStrictEqual(quals(live), [C.mergedInto.replace('{title}', 'work')]);
    assert.deepStrictEqual(lines(live, 'trash-returned'), [C.mergeLive.split('{title}').join('work')], 'a live destination: what recovering erases');
    const trashed = await rowOf(merge('m2', 'trashed', 'clients'));
    assert.deepStrictEqual(quals(trashed), [C.mergedInto.replace('{title}', 'clients')]);
    assert.deepStrictEqual(lines(trashed, 'trash-returned'), [C.mergeTrashed.replace('{title}', 'clients')], 'a destination in the trash: recover that first');
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
  assert.strictEqual(api.trashWord([ent('a')], 1), api.TRASH_WORDS.entity[0]);
  assert.strictEqual(api.trashWord([ent('a'), ent('b')], 2), api.TRASH_WORDS.entity[1]);
  assert.strictEqual(api.trashWord([fac('a'), fac('b')], 2), api.TRASH_WORDS.facet[1]);
  assert.ok(!Object.values(api.TRASH_WORDS).flat().includes(api.trashWord([wipe('a')], 1)), 'the wipe takes thing');
  assert.ok(!Object.values(api.TRASH_WORDS).flat().includes(api.trashWord([ent('a'), fac('b')], 2)), 'a mix takes things');

  // Rows that stopped part way lead the list, with no box, and nothing counts them.
  answers['api/trash'] = { erase_after_days: 7, items: [seg('a')], stopped: [{ ...ent('s1'), state: 'detaching' }, { ...fac('s2'), state: 'attaching' }, { ...seg('s3'), state: 'erasing' }] };
  await api.loadTrash();
  {
    const [first, second, third] = byId.trashList.children;
    assert.strictEqual(byId.trashList.children.length, 3, 'a stopped erase is not listed');
    assert.strictEqual(first.className, 'trash-row trash-row--stopped');
    assert.strictEqual(first.children[0].className, 'trash-row-gap', 'a spacer holds the column');
    assert.strictEqual(lines(first, 'trash-stopped')[0], C.stopped.detaching);
    assert.strictEqual(lines(second, 'trash-stopped')[0], C.stopped.attaching);
    assert.ok(!first.textContent.includes(C.erasesOn.replace('{date}', '')), 'the sentence stands in place of the date');
    assert.strictEqual(third.children[0].tag, 'input');
    byId.trashSelectAll.fire('change');
    assert.deepStrictEqual([...api.trashSelected], ['a'], 'select all leaves stopped rows out');
  }
  answers['api/trash'] = { erase_after_days: 7, items: [], stopped: [{ ...ent('s1'), state: 'attaching' }] };
  await api.loadTrash();
  assert.ok(!byId.trashList.children.some((row) => row.className === 'log-empty'), 'only stopped rows: the trash is not empty');
  assert.strictEqual(byId.trashBar.hidden, true, 'and there is nothing to act on');

  // A refused recover: the row says why, from the code and what it names.
  {
    const items = [ent('p1'), ent('p2'), wipe('w1'), fac('f1'), ent('p3'), merge('m1', 'gone', null), fac('f9')];
    answers['api/trash'] = { erase_after_days: 7, items };
    await api.loadTrash();
    byId.trashSelectAll.fire('change');
    answers['api/trash/recover'] = { done: [], failed: [
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
    assert.deepStrictEqual(note('p1'), [N.waitsFacetNone.replace('{title}', 'clients')]);
    assert.deepStrictEqual(note('p2'), [N.waitsFacetMany.replace('{title}', 'clients').replace('{n}', 2)], 'the count is the rest');
    assert.deepStrictEqual(note('w1'), [`${N.waitsSegmentMany.replace('{n}', 3)} ${N.waitsPersonOne.replace('{name}', 'Ada')}`]);
    assert.deepStrictEqual(note('f1'), [N.redirected]);
    assert.deepStrictEqual(note('p3'), [N.taken]);
    assert.deepStrictEqual(note('m1'), [], "a merge's row already says why");
    assert.deepStrictEqual(note('f9'), [], 'an unknown code adds no note');
    assert.ok(!JSON.stringify(byId.trashList).includes('has no words'), 'no server reason shows');
    assert.strictEqual(byId.trashResult.textContent, C.recoverFailedMany, 'something here could change: try again');
    answers['api/trash/recover'] = { done: [], failed: [{ id: 'p3', reason: 'taken' }, { id: 'f1', reason: 'name_redirected' }] };
    byId.trashSelectAll.fire('change');
    byId.trashRecoverBtn.fire('click');
    await flush();
    assert.strictEqual(byId.trashResult.textContent, C.recoverStuckMany, 'nothing here can change: no try again');
    assert.deepStrictEqual(note('p1'), [], 'notes are from the last act only');
  }
  {
    const N = C.notes;
    const wipeNote = (detail) => api.trashRowNote({ reason: 'waits_wipe', detail });
    assert.strictEqual(wipeNote({ segments: 1, imports: 0, people: 0 }), N.waitsSegmentOne);
    assert.strictEqual(wipeNote({ segments: 0, imports: 1, people: 0 }), N.waitsImportOne);
    assert.strictEqual(wipeNote({ segments: 0, imports: 2, people: 0 }), N.waitsImportMany.replace('{n}', 2));
    assert.strictEqual(wipeNote({ segments: 2, imports: 1, people: 0 }), N.waitsBoth);
    assert.strictEqual(wipeNote({ segments: 0, imports: 0, people: 1, name: 'Bo' }), N.waitsPersonNone.replace('{name}', 'Bo'));
    assert.strictEqual(wipeNote({ segments: 0, imports: 0, people: 4, name: 'Bo' }), N.waitsPersonMany.replace('{name}', 'Bo').replace('{n}', 3));
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
    assert.ok(!Object.values(api.TRASH_WORDS).flat().includes(api.trashWord([orig('a')], 1)), 'originals take thing');
    assert.ok(!Object.values(api.TRASH_WORDS).flat().includes(api.trashWord([orig('a'), orig('b')], 2)), 'and things');

    // A stopped one reads as part 2a's stopped rows.
    answers['api/trash'] = { erase_after_days: 7, items: [], stopped: [{ ...orig('s1', { modalities: ['screen'] }), state: 'detaching' }] };
    await api.loadTrash();
    const stoppedRow = byId.trashList.children[0];
    assert.strictEqual(stoppedRow.className, 'trash-row trash-row--stopped');
    assert.strictEqual(stoppedRow.all('removal-what')[0].textContent, L.originals_screen);
    assert.deepStrictEqual(lines(stoppedRow, 'trash-stopped'), [C.stopped.detaching]);
    assert.deepStrictEqual(lines(stoppedRow, 'trash-returned'), []);

    // A recover its segment refuses: the row that already says why takes no note,
    // one whose view didn't carry the state does, and trying again won't help.
    const items = [orig('r1', { segment_state: 'trashed' }), orig('r2', { segment_state: undefined }), orig('r3', { segment_state: 'gone' })];
    answers['api/trash'] = { erase_after_days: 7, items };
    await api.loadTrash();
    byId.trashSelectAll.fire('change');
    answers['api/trash/recover'] = { done: [], failed: [
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
    assert.deepStrictEqual(K('erase', [orig('a'), seg('b')]).paragraphs, [C.bodyMany, C.staysMany, C.originalsStaysMany], 'after the line the other kinds take');
    assert.deepStrictEqual(K('erase', [orig('a'), ent('b')]).paragraphs, [C.bodyMany, C.mentionsMany, C.originalsStaysMany]);
  }

  // A facet recovered over another of its name confirms first; nothing else does.
  {
    answers['api/trash'] = { erase_after_days: 7, items: [fac('f1', { replaces: true }), ent('e1'), fac('f2')] };
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
    assert.deepStrictEqual(K2([fac('a', { replaces: true }), fac('b', { replaces: true })]), [C.replaceSomeMany.replace('{k}', 2)]);
    byId.trashSelectAll.fire('change');
    byId.trashRecoverBtn.fire('click');
    answers['api/trash/recover'] = { done: ['f1', 'e1', 'f2'], failed: [] };
    answers['api/trash'] = { erase_after_days: 7, items: [] };
    byId.trashConfirmErase.fire('click');
    await flush();
    const recover = calls.find((call) => call.url === 'api/trash/recover');
    assert.deepStrictEqual(JSON.parse(recover.options.body).ids, ['f1', 'e1', 'f2'], 'the confirm recovers');
    answers['api/trash'] = { erase_after_days: 7, items: [ent('e1'), ent('e2', { same_name: true })] };
    await api.loadTrash();
    byId.trashSelectAll.fire('change');
    modal.open = false;
    answers['api/trash/recover'] = { done: ['e1', 'e2'], failed: [] };
    byId.trashRecoverBtn.fire('click');
    assert.strictEqual(modal.open, false, 'people keep both, so no confirm');
    await flush();
    answers['api/trash'] = { erase_after_days: 7, items: [seg('a')] };
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
  assert.deepStrictEqual(K('erase', [wipe('a'), seg('b')]).paragraphs, [C.bodyMany, C.staysMany]);
  assert.deepStrictEqual(K('empty', [ent('a'), seg('b')]).paragraphs.slice(1), [C.mentionsMany]);

  // Results: what the answer says, checked against the reloaded list.
  const items = [seg('a'), seg('c')];
  const line = (act, answer, listed) => api.trashResultLine(act, items, answer, listed);
  const failA = { id: 'a', reason: 'busy' };
  assert.ok(line('recover', { done: ['a', 'c'], failed: [] }, new Set()).length > 0);
  assert.strictEqual(line('recover', { done: ['c'], failed: [failA] }, new Set(['a'])), C.partialRecoverOne.replace('{done}', 1).replace('{n}', 2));
  assert.strictEqual(line('erase', { done: [], failed: [failA] }, new Set(['a'])), C.eraseFailedOne);
  assert.strictEqual(line('recover', { done: [], failed: [failA, { id: 'c' }] }, new Set(['a', 'c'])), C.recoverFailedMany);
  assert.strictEqual(line('empty', { done: [], failed: [failA] }, new Set(['a'])), C.emptyFailed);
  // A failed id no longer listed isn't "still in the trash", and no server reason shows.
  assert.strictEqual(line('erase', { done: [], failed: [failA] }, new Set()), C.goneOne);
  assert.strictEqual(line('erase', { done: [], failed: [failA, { id: 'c' }] }, new Set()), C.goneMany);
  assert.strictEqual(line('recover', { done: ['c'], failed: [failA] }, new Set()), `${C.recovered.replace('{n}', 1).replace('{things}', 'segment')} ${C.goneMoreOne}`);
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
  const hist = {};
  vm.createContext(hist);
  vm.runInContext(`
    const REMOVAL_COPY = { erasedOnItsDate: 'TIMER' };
    const REMOVAL_ACTION = { segment_delete: 'STAGE1' };
    const REMOVAL_ACTION_FALLBACK = 'FALLBACK';
    const REMOVAL_ACTION_NAMES_PLACE = {};
    const REMOVAL_SURFACE = { settings: 'IN-SETTINGS', cli: 'BY-CLI' };
    function removalSurfaceKey(entry) { return entry.surface === 'web' ? entry.page : entry.surface; }
    ${history.slice(0, history.indexOf('\n')).trim()}
    ${html.slice(html.indexOf('function removalHow(entry)'), html.indexOf('function removalGroups'))}
    this.removalHow = removalHow; this.REMOVAL_EVENT = REMOVAL_EVENT;
  `, hist);
  const web = { surface: 'web', page: 'settings' };
  assert.strictEqual(hist.removalHow({ ...web, event: 'trashed' }), `${hist.REMOVAL_EVENT.trashed} IN-SETTINGS`);
  assert.strictEqual(hist.removalHow({ surface: 'cli', event: 'recovered' }), `${hist.REMOVAL_EVENT.recovered} BY-CLI`);
  assert.strictEqual(hist.removalHow({ surface: 'policy', policy: 'trash', event: 'erased' }), 'TIMER');
  assert.strictEqual(hist.removalHow({ ...web, event: 'removed', action: 'segment_delete' }), 'STAGE1 IN-SETTINGS');

  // An import's line: the kind, its source, the day it came in from its id; never its file.
  const rows = {};
  vm.createContext(rows);
  rows.document = document;
  rows.window = { JournalFormat: { day: (key) => `d${key}` } };
  vm.runInContext(`${html.slice(html.indexOf('// Removal history:'), html.indexOf('function renderRemovalHistory'))}
    this.removalWhat = removalWhat; this.removalGroupWhat = removalGroupWhat; this.REMOVAL_KIND = REMOVAL_KIND; this.REMOVAL_SOURCE = REMOVAL_SOURCE; this.REMOVAL_KIND_GROUP = REMOVAL_KIND_GROUP;`, rows);
  const historyLine = (unit) => {
    const node = rows.removalWhat(unit);
    return { what: node.all('removal-what').map((n) => n.textContent), quals: node.all('removal-qual').map((n) => n.textContent) };
  };
  const importLine = historyLine({ kind: 'import', source: 'chatgpt', id: '20261102_161200', file: 'secret.zip' });
  assert.deepStrictEqual(importLine.what, [rows.REMOVAL_KIND.import[0]]);
  assert.strictEqual(importLine.quals.length, 2);
  assert.strictEqual(importLine.quals[0], rows.REMOVAL_SOURCE.chatgpt);
  assert.ok(importLine.quals[1].includes('d20261102'));
  assert.ok(!JSON.stringify(importLine).includes('secret'), 'the history keeps nothing that was in it');
  assert.ok(historyLine({ kind: 'import', source: '', id: '20261102_161200' }).quals.length === 1, 'a source-less import shows no empty qualifier');
  assert.ok(historyLine({ kind: 'import', source: 'toString', id: 'bad' }).quals.length === 0);
  const workoutLine = historyLine({ kind: 'workout', source: 'strava', id: 'w1' });
  assert.deepStrictEqual(workoutLine.what, [rows.REMOVAL_KIND.workout[0]]);
  assert.deepStrictEqual(workoutLine.quals, [rows.REMOVAL_SOURCE.strava], 'without its day a workout reads as its kind and source');
  assert.deepStrictEqual(historyLine({ kind: 'workout', source: 'strava', day: '20261108' }).quals, ['d20261108', rows.REMOVAL_SOURCE.strava]);
  const group = rows.removalGroupWhat({ units: [{ unit: { kind: 'import', source: 'chatgpt', id: '20261102_161200' } }, { unit: { kind: 'import', source: 'document', id: '20261101_094000' } }] });
  assert.ok(group.all('removal-what')[0].textContent.endsWith(rows.REMOVAL_KIND.import[1]), 'a group of imports keeps the plural kind');

  // Originals released from a segment read as original media with the day and
  // time range, and a group counts segments, not files. Older rows keep theirs.
  {
    const unit = { kind: 'originals', day: '20261110', stream: 'desk', segment: '070000_300', files: ['070000_300.flac', 'screen.webm'] };
    const one = historyLine(unit);
    assert.deepStrictEqual(one.what, [rows.REMOVAL_KIND.originals[0]]);
    assert.strictEqual(one.quals.length, 2);
    assert.strictEqual(one.quals[0], 'd20261110');
    assert.ok(!JSON.stringify(one).includes('.flac') && !JSON.stringify(one).includes('desk'), 'no file name and no stream');
    const group = rows.removalGroupWhat({ units: [{ unit }, { unit: { ...unit, segment: '080000_300' } }, { unit: { ...unit, segment: '090000_300' } }] });
    assert.deepStrictEqual(group.all('removal-what').map((n) => n.textContent), [rows.REMOVAL_KIND_GROUP.originals.replace('{n}', 3)]);
    assert.deepStrictEqual(group.all('removal-qual').map((n) => n.textContent), ['d20261110'], 'the day they share');
    const old = historyLine({ kind: 'original', file: '120000_300.m4a', day: '20261105', segment: '120000_300' });
    assert.deepStrictEqual(old.what, [rows.REMOVAL_KIND.original[0]]);
    assert.strictEqual(old.quals[0], '120000_300.m4a', 'an older row keeps its file name');
    const oldGroup = rows.removalGroupWhat({ units: [{ unit: { kind: 'original', day: '20261105' } }, { unit: { kind: 'original', day: '20261105' } }] });
    assert.ok(oldGroup.all('removal-what')[0].textContent.endsWith(rows.REMOVAL_KIND.original[1]), 'an older group keeps its count and plural');
  }

  // A merge's files have a label of their own and no name to keep.
  const mergeLine = historyLine({ kind: 'facet_file' });
  assert.deepStrictEqual(mergeLine.what, [rows.REMOVAL_KIND.facet_file[0]]);
  assert.deepStrictEqual(mergeLine.quals, [], "it isn't one of the nameless kinds");
  assert.deepStrictEqual(historyLine({ kind: 'entity' }).quals.length, 1, 'an entity keeps its name-not-kept qualifier');

  // What a recover erased in its way folds into the recover's how-line and
  // leaves the group's count.
  const fold = {};
  vm.createContext(fold);
  vm.runInContext(`
    const REMOVAL_COPY = { foldErasing: '{how}+ERASING', foldExcept: '{how}+EXCEPT', foldBoth: '{how}+BOTH' };
    function removalHow(entry) { return 'HOW:' + entry.event; }
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
})().catch((error) => { console.error(error); process.exit(1); });
