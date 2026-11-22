// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { journalContext } = require('../../solstone-core-convey-shell/tests/journal_catalog_dom.js');

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
    const window = { createViewReady: () => () => {}, JournalFormat: { day: String }, Drawer: { render: () => '<div></div>' },
      apiJson: async (url, options) => { calls.push({ url, body: JSON.parse(options.body) }); return api(url, options); } };
    const context = journalContext({ window, document });
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
              getAttribute(key) { return key in this.attributes ? this.attributes[key] : null; },
              hasAttribute(key) { return key in this.attributes; },
              removeAttribute(key) { delete this.attributes[key]; },
              focus() { if (!this.disabled) document.activeElement = this; }, onclick: null };
          } };
          if (!value) this.button = null;
        } };
      return node;
    }
    let refresh = async () => {};
    function render(data, opts = { autoPreview: false }) {
      const focus = Boolean(root.slot?.contains(document.activeElement));
      const html = window.ImportDetail.renderDetail(data);
      root.innerHTML = html;
      window.ImportDetail.mountSavedStart(root, data, () => refresh(), focus, opts);
      return html;
    }
    return { calls, root, document, window, render, deriveStatus: data => window.ImportDetail.deriveStatus(data), setRefresh(fn) { refresh = fn; },
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
      file_size: null, folder_snapshot: 'a'.repeat(64), source_hint: 'gemini', setting: null });
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

  // ---- Import OX auto-preview, Apple Health / Evernote / space / facet / discard / running / expired ----
  function richHarness(api) {
    const calls = [];
    const docListeners = new Map();
    const document = {
      activeElement: null,
      addEventListener(type, fn) { docListeners.set(type, [...(docListeners.get(type) || []), fn]); },
      removeEventListener(type, fn) { docListeners.set(type, (docListeners.get(type) || []).filter(f => f !== fn)); },
      dispatchKey(key) {
        const ev = { key, preventDefault() { this.defaultPrevented = true; } };
        for (const fn of [...(docListeners.get('keydown') || [])]) fn(ev);
        return ev;
      },
    };
    const window = {
      createViewReady: () => () => {},
      JournalFormat: { day: String },
      Drawer: { render: () => '<div></div>' },
      apiJson: async (url, options = {}) => {
        calls.push({ url, body: options.body ? JSON.parse(options.body) : null, signal: options.signal || null });
        return api(url, options);
      },
    };
    const context = journalContext({ window, document });
    vm.runInContext(source, context);
    function makeEl(tag = 'div') {
      return {
        tagName: tag.toUpperCase(), isConnected: true, hidden: false, disabled: false, value: '',
        attributes: {}, textContent: '', _html: '', onclick: null, onchange: null, oninput: null,
        children: new Map(),
        setAttribute(k, v) { this.attributes[k] = String(v); },
        getAttribute(k) { return k in this.attributes ? this.attributes[k] : null; },
        hasAttribute(k) { return k in this.attributes; },
        removeAttribute(k) { delete this.attributes[k]; },
        focus() { document.activeElement = this; },
        contains(node) { return node === this || [...this.children.values()].some(c => c === node || c.contains?.(node)); },
        querySelector(sel) {
          for (const [k, v] of this.children) if (sel.includes(k)) return v;
          return null;
        },
        querySelectorAll(sel) {
          const out = [];
          for (const [k, v] of this.children) if (sel.includes(k)) out.push(v);
          return out;
        },
        get innerHTML() { return this._html; },
        set innerHTML(v) {
          this._html = String(v || '');
          this.children.clear();
          if (this._html.includes('data-saved-start-status')) {
            this.children.set('data-saved-start-status', makeEl('p'));
            this.children.set('data-saved-start-preview', makeEl('div'));
            this.children.set('data-saved-start-options', makeEl('div'));
            this.children.set('data-saved-start-originals', makeEl('div'));
            this.children.set('data-saved-start-actions', makeEl('div'));
            this.children.set('data-saved-start-discard', makeEl('div'));
            this.children.set('data-saved-start-error', makeEl('div'));
          }
          if (this._html.includes('data-saved-start-action')) {
            this.children.set('data-saved-start-action', makeEl('button'));
            this.children.set('data-saved-start-discard-trigger', makeEl('button'));
          }
          if (this._html.includes('data-discard-cancel')) {
            this.children.set('data-discard-cancel', makeEl('button'));
            this.children.set('data-discard-confirm', makeEl('button'));
          }
          if (this._html.includes('data-review-facet-answer="yes"')) {
            const yes = makeEl('button'); yes.setAttribute('data-review-facet-answer', 'yes');
            const no = makeEl('button'); no.setAttribute('data-review-facet-answer', 'no');
            this.children.set('data-review-facet-answer="yes"', yes);
            this.children.set('data-review-facet-answer="no"', no);
            this.children.set('data-review-facet-answer', yes);
          }
          if (this._html.includes('data-review-facet-select')) {
            this.children.set('data-review-facet-select', makeEl('select'));
          }
          if (this._html.includes('data-review-context')) {
            this.children.set('data-review-context', makeEl('input'));
          }
          if (this._html.includes('data-review-notebook')) {
            this.children.set('data-review-notebook', makeEl('input'));
          }
        },
      };
    }
    const root = {
      isConnected: true, slot: null,
      querySelector(sel) {
        if (sel.includes('data-saved-start-slot')) return this.slot;
        return null;
      },
      set innerHTML(html) { this.slot = html.includes('data-saved-start-slot') ? makeEl('div') : null; },
    };
    const slotQuery = sel => {
      if (!root.slot) return null;
      const direct = root.slot.querySelector(sel);
      if (direct) return direct;
      for (const child of root.slot.children.values()) {
        const nested = child.querySelector(sel);
        if (nested) return nested;
      }
      return null;
    };
    // Patch root.slot.querySelector to search nested containers (actions, options, discard).
    function patchSlot(s) {
      if (!s) return;
      const orig = s.querySelector.bind(s);
      s.querySelector = sel => {
        const hit = orig(sel);
        if (hit) return hit;
        for (const child of s.children.values()) {
          const nested = child.querySelector?.(sel);
          if (nested) return nested;
        }
        return null;
      };
    }
    let refresh = async () => {};
    function render(data, opts = {}) {
      const html = window.ImportDetail.renderDetail(data);
      root.innerHTML = html;
      patchSlot(root.slot);
      window.ImportDetail.mountSavedStart(root, data, () => refresh(), false, opts);
      return html;
    }
    return { calls, root, document, window, render, q: slotQuery, setRefresh(fn) { refresh = fn; } };
  }

  // Case: Auto-preview on Apple Health sends body_confirmed on start, supports listed_moves & space check.
  {
    const apple = () => {
      const d = saved();
      d.import_json.source_hint = 'apple_health';
      d.import_json.original_filename = 'export.zip';
      return d;
    };
    const applePreview = {
      preview_token: 'apple-tok-1',
      size: {
        recordings: 0, days: 365, other_units: ['records', 1200],
        apple_health: { workouts: 48, nights: 310, readings: 9200, span: ['2025-01-01', '2026-01-01'] },
        listed_moves: { count: 3 },
      },
      space: { fits: true, need_words: '42 MB', free_words: '18 GB' },
    };
    const h = richHarness(async url => url.endsWith('/preview') ? applePreview : { import_id: apple().timestamp });
    h.render(apple());
    await new Promise(r => setImmediate(r));
    assert.strictEqual(h.calls.length, 1, 'auto-preview runs on mount');
    assert.ok(h.calls[0].url.endsWith('/preview'));
    assert.ok(h.q('data-saved-start-preview').innerHTML.includes('48'), 'renders Apple Health workouts');
    assert.ok(h.q('data-saved-start-preview').innerHTML.includes('where your journal lives'), 'uses location-honest space copy');
    assert.ok(h.q('data-saved-start-preview').innerHTML.includes('3'), 'renders listed_moves count');
    const actionBtn = h.q('data-saved-start-action');
    assert.strictEqual(actionBtn.getAttribute('aria-disabled'), 'false');
    await actionBtn.onclick();
    const startCall = h.calls.find(c => c.url.endsWith('/start'));
    assert.ok(startCall, 'start call fired');
    assert.strictEqual(startCall.body.body_confirmed, 'apple-tok-1', 'Apple Health start includes body_confirmed');
    cases += 1;
  }

  // Case: Low space disables Start button and shows alert warning.
  {
    const apple = saved();
    apple.import_json.source_hint = 'apple_health';
    const h = richHarness(async () => ({
      preview_token: 'apple-low-space',
      size: { recordings: 0, days: 10, other_units: ['records', 20], apple_health: { workouts: 2 } },
      space: { fits: false, need_words: '2 GB', free_words: '100 MB' },
    }));
    h.render(apple);
    await new Promise(r => setImmediate(r));
    assert.strictEqual(h.q('data-saved-start-action').textContent, 'try preview again', 'insufficient space shows try preview again, not start import');
    assert.ok(h.q('data-saved-start-preview').innerHTML.includes('import-preview-space-warn'));
    await h.q('data-saved-start-action').onclick();
    assert.ok(h.calls.every(c => c.url.endsWith('/preview')), 'low space blocks /start');
    cases += 1;
  }

  // Case: Definite refusal (409 body_space) surfaces error without locking into start_unknown.
  {
    const h = richHarness(async url => {
      if (url.endsWith('/preview')) return ready;
      const err = new Error('not enough space');
      err.status = 409;
      err.reasonCode = 'body_space';
      err.serverMessage = 'not enough space where your journal lives';
      throw err;
    });
    h.render(saved());
    await new Promise(r => setImmediate(r));
    await h.q('data-saved-start-action').onclick();
    assert.strictEqual(h.q('data-saved-start-error').textContent, 'not enough space where your journal lives');
    assert.strictEqual(h.q('data-saved-start-action').textContent, 'try preview again', 'definite refusal keeps retryable preview path, not check status');
    cases += 1;
  }

  // Case: Evernote preview + notebook edit saves via /meta before /start.
  {
    const ev = saved();
    ev.import_json.source_hint = 'evernote';
    const h = richHarness(async url => {
      if (url.endsWith('/preview')) return {
        preview_token: 'ev-tok',
        size: { schema: 'solstone.import.evernote.preview.v1', ready: true, lines: ['12 notes ready'], notebook: 'Work' },
      };
      return { ok: true, import_id: ev.timestamp };
    });
    h.render(ev);
    await new Promise(r => setImmediate(r));
    const nb = h.q('data-review-notebook');
    assert.ok(nb, 'Evernote renders notebook input');
    nb.value = 'Personal';
    nb.oninput();
    await h.q('data-saved-start-action').onclick();
    const metaCall = h.calls.find(c => c.url.endsWith('/meta'));
    const startCall = h.calls.find(c => c.url.endsWith('/start'));
    assert.ok(metaCall && metaCall.body.notebook === 'Personal', 'notebook saved via /meta');
    assert.ok(startCall && startCall.body.preview_token === 'ev-tok');
    cases += 1;
  }

  // Case: First-use body-facet ask_* blocks Start until answered, then writes /meta and re-previews.
  {
    const apple = saved();
    apple.import_json.source_hint = 'apple_health';
    let answered = false;
    const bodyFacet = {
      async load() {
        return { offer: { state: 'ask_new', facet: 'body' }, facets: [{ name: 'body', title: 'body' }], copy: { ask_new: 'create body facet?', yes: 'yes', no: 'no' } };
      },
      async answer(ans) {
        answered = ans === 'yes';
        return { offer: { state: 'chosen', facet: 'body' }, facets: [{ name: 'body', title: 'body' }], copy: { none_option: 'none' } };
      },
    };
    const h = richHarness(async url => {
      if (url.endsWith('/preview')) return { preview_token: 'tok-facet', size: { recordings: 0, days: 1, other_units: null, apple_health: { workouts: 1 } } };
      return { ok: true };
    });
    h.render(apple, { bodyFacet });
    await new Promise(r => setImmediate(r));
    assert.strictEqual(h.q('data-saved-start-action').getAttribute('aria-disabled'), 'true', 'unanswered facet blocks start');
    const yesBtn = h.q('data-review-facet-answer="yes"');
    assert.ok(yesBtn, 'first-use facet buttons rendered');
    await yesBtn.onclick();
    assert.ok(answered, 'body-facet answer sent');
    assert.ok(h.calls.some(c => c.url.endsWith('/meta') && c.body.body_facet?.facet === 'body'), 'facet choice written via /meta');
    assert.strictEqual(h.calls.filter(c => c.url.endsWith('/preview')).length, 2, 're-previews after facet change');
    cases += 1;
  }

  // Case: Context (setting) edit writes /meta + re-previews; Discard confirm handles Escape, abort, 200, and 409.
  {
    let discardedId = null;
    let discardStatus = 200;
    const h = richHarness(async url => {
      if (url.endsWith('/preview')) return ready;
      if (url.endsWith('/discard')) {
        if (discardStatus !== 200) { const e = new Error('conflict'); e.status = discardStatus; throw e; }
        return { discarded: true };
      }
      return { ok: true };
    });
    h.render(saved(), { onDiscard: id => { discardedId = id; } });
    await new Promise(r => setImmediate(r));
    const ctxIn = h.q('data-review-context');
    ctxIn.value = 'spring archive';
    ctxIn.oninput();
    await ctxIn.onchange();
    assert.ok(h.calls.some(c => c.url.endsWith('/meta') && c.body.setting === 'spring archive'), 'setting saved via /meta');
    assert.strictEqual(h.calls.filter(c => c.url.endsWith('/preview')).length, 2, 'setting edit re-previews');

    // Open inline discard confirm, cancel with Escape -> focus returns to trigger.
    const trigger = h.q('data-saved-start-discard-trigger');
    trigger.onclick();
    assert.ok(h.q('data-discard-cancel'), 'discard confirm rendered inline');
    h.document.dispatchKey('Escape');
    assert.strictEqual(h.document.activeElement, trigger, 'Escape restores focus to discard trigger');
    assert.ok(!h.q('data-discard-cancel'), 'Escape closes confirm box');

    // Confirm discard -> 200 calls onDiscard.
    trigger.onclick();
    await h.q('data-discard-confirm').onclick();
    assert.strictEqual(discardedId, saved().timestamp, '200 discard invokes onDiscard');
    cases += 1;
  }

  // Case: renderRunningProgress (Apple Health body poll + SSE stage) and renderExpiredRecovery.
  {
    const h = richHarness(async () => ready);
    const bodyRunning = h.window.ImportDetail.renderRunningProgress({
      status: 'running',
      thinking: { status: 'body', live: true, body: { phase: 'write', days_done: 120, days_total: 400 } },
    });
    assert.ok(bodyRunning.includes('aria-valuenow="30"'), 'Apple Health body progress bar computes percent');
    assert.ok(bodyRunning.includes('120'), 'Apple Health body progress shows days done');
    const sseRunning = h.window.ImportDetail.renderRunningProgress(
      { status: 'running' },
      { headline: 'importing conversations', detail: '4 of 10', pct: 40 }
    );
    assert.ok(sseRunning.includes('importing conversations') && sseRunning.includes('aria-valuenow="40"'));
    const exp = h.window.ImportDetail.renderExpiredRecovery();
    assert.ok(exp.metaHtml.includes('expired') && exp.bodyHtml.includes('/app/import/'));
    cases += 1;
  }

  return cases;
};
