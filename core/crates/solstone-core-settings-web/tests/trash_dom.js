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

class El {
  constructor(tag) { this.tag = tag; this.children = []; this.dataset = {}; this.style = {}; this.hidden = false; this.listeners = {}; this.className = ''; this._text = ''; }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return this._text + this.children.map((child) => child.textContent).join(''); }
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren(...nodes) { this.children = nodes; this._text = ''; }
  addEventListener(name, fn) { this.listeners[name] = fn; }
  fire(name, event = {}) { return this.listeners[name]({ currentTarget: this, target: this, ...event }); }
  focus() { document.activeElement = this; }
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
vm.runInContext(`${html.slice(start, end)};this.api={TRASH_COPY,renderTrash,loadTrash,trashEraseDate,trashResultLine,trashConfirmCopy,setupTrash,saveTrashDays,trashSelected};`, sandbox);
const { api } = sandbox;
const C = api.TRASH_COPY;
api.setupTrash();

const LATER = '2099-11-15T00:00:00.000Z';
const seg = (id, extra = {}) => ({ id, kind: 'segment', day: '20261101', stream: 'field', segment: '070000_300', erase_at: LATER, bytes: 2048, replaces: false, ...extra });
const mem = (id) => ({ id, kind: 'memory', day: '20261101', stream: 'agent-memory-x', segment: '080000_1', erase_at: LATER, bytes: 10, replaces: false });
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

  // Results: what the answer says, checked against the reloaded list.
  const items = [seg('a'), seg('c')];
  const line = (act, answer, listed) => api.trashResultLine(act, items, answer, listed);
  const failA = { id: 'a', reason: 'busy' };
  assert.ok(line('recover', { done: ['a', 'c'], failed: [] }, new Set()).length > 0);
  assert.strictEqual(line('recover', { done: ['c'], failed: [failA] }, new Set(['a'])), C.partialRecoverOne.replace('{done}', 1).replace('{n}', 2));
  assert.strictEqual(line('erase', { done: [], failed: [failA] }, new Set(['a'])), C.eraseFailedOne);
  assert.strictEqual(line('recover', { done: [], failed: [failA, { id: 'c' }] }, new Set(['a', 'c'])), C.recoverFailedMany);
  assert.strictEqual(line('empty', { done: [], failed: [failA] }, new Set(['a'])), C.emptyFailed);
  // A failed id no longer listed isn't "still in the trash": its reason shows instead.
  assert.strictEqual(line('erase', { done: [], failed: [failA] }, new Set()), 'busy.');
  // No answer and no list: nothing is claimed.
  assert.strictEqual(line('erase', null, null), '');

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
})().catch((error) => { console.error(error); process.exit(1); });
