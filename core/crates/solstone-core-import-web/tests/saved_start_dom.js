// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

module.exports = async function savedStartCases(crateDir) {
  let cases = 0;
  const source = fs.readFileSync(path.join(crateDir, 'assets/import_detail.js'), 'utf8');
  const saved = () => ({
    timestamp: '20261029_120000', status: 'pending', saved_start: { state: 'eligible', active: false },
    import_json: { import_id: '20261029_120000', file_path: '/journal/imports/20261029_120000/note.txt',
      source_hash: 'source-digest', file_size: 6, is_local_path: false,
      source_timestamp: '20250101_090000', setting: 'planning' }, imported_json: null,
  });
  const ready = { preview_token: 'current-token', size: { recordings: 1, days: 1, other_units: null } };
  function harness(api) {
    const calls = [];
    const document = { activeElement: null, addEventListener() {} };
    const window = { JournalFormat: { day: String }, Drawer: { render: () => '<div></div>' },
      apiJson: async (url, options) => { calls.push({ url, body: JSON.parse(options.body) }); return api(url, options); } };
    const context = vm.createContext({ window, document });
    vm.runInContext(source, context);
    const root = { isConnected: true, slot: null,
      querySelector() { return this.slot; },
      set innerHTML(html) {
        if (this.slot) {
          if (this.slot.contains(document.activeElement)) document.activeElement = null;
          this.slot.isConnected = false;
        }
        this.slot = html.includes('data-saved-start-slot') ? slot() : null;
      } };
    function slot() {
      let html = '';
      const node = { isConnected: true, button: null, status: null, error: null, actions: null,
        contains(value) { return value === this.button && value !== null; },
        querySelector(selector) {
          return selector.includes('status]') ? this.status : selector.includes('error]') ? this.error
            : selector.includes('actions]') ? this.actions : this.button;
        },
        get innerHTML() { return html; },
        set innerHTML(value) {
          html = value;
          this.status = { textContent: '' }; this.error = { textContent: '' };
          this.actions = { set innerHTML(_) {
            if (node.button === document.activeElement) document.activeElement = null;
            node.button = { disabled: false, attributes: {}, textContent: '',
              setAttribute(key, value) { this.attributes[key] = value; },
              focus() { if (!this.disabled) document.activeElement = this; }, onclick: null };
          } };
          if (!value) this.button = null;
        } };
      return node;
    }
    let refresh = async () => {};
    function render(data) {
      const focus = Boolean(root.slot?.contains(document.activeElement));
      const html = window.ImportDetail.renderDetail(data);
      root.innerHTML = html;
      window.ImportDetail.mountSavedStart(root, data, () => refresh(), focus);
      return html;
    }
    return { calls, root, document, render, deriveStatus: data => window.ImportDetail.deriveStatus(data), setRefresh(fn) { refresh = fn; },
      button: () => root.slot?.button, html: () => root.slot?.innerHTML || '' };
  }

  {
    let finish;
    const pending = new Promise(resolve => { finish = resolve; });
    const h = harness(async url => url.endsWith('/preview') ? pending : { import_id: saved().timestamp });
    h.render(saved());
    const first = h.button(); first.focus();
    const preview = first.onclick();
    await first.onclick();
    h.render(saved());
    assert.strictEqual(h.calls.length, 1);
    assert.strictEqual(h.button().attributes['aria-disabled'], 'true');
    finish(ready); await preview;
    assert.ok(h.html().includes('role="status"'));
    assert.strictEqual(h.button().attributes['aria-disabled'], 'false');
    assert.strictEqual(h.document.activeElement, h.button(), 'Preview preserves keyboard focus through pending refresh');
    await first.onclick();
    assert.strictEqual(h.calls.length, 1, 'a detached Preview click cannot become Start');
    h.setRefresh(async () => h.render({ ...saved(), status: 'running', saved_start: { state: 'consumed', active: true } }));
    await h.button().onclick();
    assert.strictEqual(h.calls.length, 2);
    const [p, s] = h.calls;
    assert.ok(p.url.endsWith('/preview') && s.url.endsWith('/start'));
    for (const call of h.calls) {
      assert.strictEqual(call.body.import_id, saved().timestamp);
      assert.strictEqual(call.body.timestamp, saved().import_json.source_timestamp);
      assert.strictEqual(call.body.setting, 'planning');
      assert.strictEqual(call.body.path, saved().import_json.file_path);
      assert.ok(!call.url.endsWith('/save'));
    }
    assert.strictEqual(s.body.preview_token, ready.preview_token);
    assert.strictEqual(s.body.first_start_only, true);
    assert.ok(!h.button());
    cases += 1;
  }
  {
    const h = harness(async () => ready);
    h.render(saved()); await h.button().onclick();
    const reloaded = harness(async () => ready);
    reloaded.render(saved()); await reloaded.button().onclick();
    assert.ok(reloaded.calls[0].url.endsWith('/preview'), 'reload never uses invisible old Preview');
    for (const state of ['unknown', 'consumed']) {
      const legacy = harness(async () => { throw new Error('no request'); });
      legacy.render({ ...saved(), saved_start: { state, active: false } });
      assert.ok(!legacy.button()); assert.strictEqual(legacy.calls.length, 0);
    }
    cases += 1;
  }
  {
    const h = harness(async () => ({ preview_token: 'empty', size: { recordings: 0, days: 0, other_units: null } }));
    h.render(saved()); await h.button().onclick();
    assert.ok(h.html().includes('role="status"'));
    await h.button().onclick();
    assert.ok(h.calls.every(call => call.url.endsWith('/preview')));
    cases += 1;
  }
  {
    const h = harness(async url => { if (url.endsWith('/start')) throw new Error('reply lost'); return ready; });
    h.render(saved()); await h.button().onclick(); await h.button().onclick();
    let reads = 0;
    h.setRefresh(async () => { reads += 1; h.render(saved()); });
    await h.button().onclick(); await h.button().onclick();
    assert.strictEqual(reads, 2);
    assert.strictEqual(h.calls.filter(call => call.url.endsWith('/start')).length, 1);
    assert.strictEqual(h.calls.length, 2, 'stale unstarted reads cannot restore mutating controls');
    cases += 1;
  }
  {
    let finish;
    const h = harness(async () => new Promise(resolve => { finish = resolve; }));
    h.render(saved()); const old = h.button(); const pending = old.onclick();
    const changed = saved(); changed.import_json.setting = 'new setting';
    h.render(changed);
    await h.button().onclick();
    assert.strictEqual(h.calls.length, 1, 'a changed detail binding cannot overlap Preview requests');
    finish(ready); await pending;
    await old.onclick();
    assert.strictEqual(h.calls.length, 1);
    const retry = h.button().onclick(); finish(ready); await retry;
    assert.ok(h.calls[1].url.endsWith('/preview'));
    assert.strictEqual(h.calls[1].body.setting, 'new setting');
    cases += 1;
  }
  {
    const h = harness(async () => ready);
    h.render(saved()); await h.button().onclick();
    h.render({ ...saved(), saved_start: { state: 'consumed', active: false } });
    assert.ok(!h.button());
    h.render(saved());
    assert.ok(h.button());
    await h.button().onclick();
    assert.strictEqual(h.calls.length, 1, 'a consumed observation permanently invalidates the old token');
    cases += 1;
  }
  {
    const h = harness(async url => { if (url.endsWith('/start')) throw new Error('reply lost'); return ready; });
    h.render(saved()); await h.button().onclick(); await h.button().onclick();
    h.setRefresh(async () => { throw new Error('detail read failed'); });
    await h.button().onclick();
    assert.ok(h.button());
    assert.ok(h.root.slot.error.textContent);
    assert.strictEqual(h.calls.filter(call => call.url.endsWith('/start')).length, 1);
    cases += 1;
  }
  {
    const h = harness(async () => { throw new Error('no mutation'); });
    const file = saved();
    const pointer = saved(); pointer.import_json.is_local_path = true;
    h.render(pointer);
    assert.ok(!h.button(), 'local pointers do not gain ordinary file continuation');
    assert.deepStrictEqual(h.deriveStatus(pointer), h.deriveStatus(file), 'authoritative unstarted status is independent of supported controls');
    assert.strictEqual(h.calls.length, 0);
    cases += 1;
  }
  // A copied folder is registered with no single file size and its copied tree's snapshot.
  const folder = () => {
    const data = saved();
    Object.assign(data.import_json, { file_path: '/journal/imports/20261029_120000/tree',
      file_size: null, folder_snapshot: 'a'.repeat(64), source_hint: 'obsidian', setting: null });
    return data;
  };
  {
    const h = harness(async url => url.endsWith('/preview') ? ready : { import_id: folder().timestamp });
    h.render(folder());
    assert.ok(h.button(), 'an eligible copied folder offers Preview');
    await h.button().onclick();
    h.setRefresh(async () => h.render({ ...folder(), status: 'running', saved_start: { state: 'consumed', active: true } }));
    await h.button().onclick();
    const [p, s] = h.calls;
    assert.ok(h.calls.length === 2 && p.url.endsWith('/preview') && s.url.endsWith('/start'));
    for (const call of h.calls) {
      assert.strictEqual(call.body.path, folder().import_json.file_path);
      assert.strictEqual(call.body.setting, '');
    }
    assert.strictEqual(s.body.preview_token, ready.preview_token);
    assert.strictEqual(s.body.first_start_only, true);
    assert.ok(!h.button());
    cases += 1;
  }
  {
    const h = harness(async () => ready);
    h.render(folder()); await h.button().onclick();
    const changed = folder(); changed.import_json.folder_snapshot = 'b'.repeat(64);
    h.render(changed); await h.button().onclick();
    assert.ok(h.calls.every(call => call.url.endsWith('/preview')), 'a changed folder snapshot needs a new Preview');
    const refused = harness(async () => { throw new Error('no request'); });
    const sizeless = folder(); delete sizeless.import_json.folder_snapshot;
    const undigested = folder(); undigested.import_json.folder_snapshot = 'snapshot';
    const sizelessFile = folder(); sizelessFile.import_json.file_path = saved().import_json.file_path;
    const sized = folder(); sized.import_json.file_size = 0;
    const archive = folder(); archive.import_json.source_hint = 'audio_archive';
    const pointer = folder(); pointer.import_json.is_local_path = true;
    for (const data of [sizeless, undigested, sizelessFile, sized, archive, pointer,
      { ...folder(), saved_start: { state: 'unknown', active: false } },
      { ...folder(), saved_start: { state: 'consumed', active: false } }]) {
      refused.render(data);
      assert.ok(!refused.button());
    }
    assert.strictEqual(refused.calls.length, 0);
    cases += 1;
  }
  return cases;
};
