// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

function functionSource(source, name) {
  const match = new RegExp(`(?:async\\s+)?function\\s+${name}\\(`).exec(source);
  assert.ok(match, `${name} exists in the shipped module`);
  const open = source.indexOf('{', source.indexOf(')', match.index));
  let depth = 0;
  for (let index = open; index < source.length; index++) {
    if (source[index] === '{') depth++;
    if (source[index] === '}' && --depth === 0) return source.slice(match.index, index + 1);
  }
  throw new Error('unterminated source function');
}

function harness(crateDir, source = 'obsidian') {
  const script = fs.readFileSync(path.join(crateDir, 'assets/source_connections.js'), 'utf8');
  const storage = new Map();
  const calls = [];
  const field = () => ({ textContent: '', innerHTML: '', disabled: false, isConnected: true, focus() { this.focused = true; } });
  const fields = Object.fromEntries(['data-source-change-status', 'data-source-status', 'data-source-connection', 'data-source-connection-actions', 'data-source-schedule-state', 'data-source-conflicts'].map(key => [key, field()]));
  const select = { value: '', disabled: true, set innerHTML(value) { this.html = value; this.value = /value="([^"]*)" selected/.exec(value)?.[1] || ''; }, get innerHTML() { return this.html || ''; } };
  const input = { checked: false, disabled: true };
  const buttons = Object.fromEntries(['data-source-sync', 'data-source-refresh', 'data-source-remove'].map(key => [key, field()]));
  const form = { querySelector(selector) { if (selector === 'select') return select; if (selector === 'input') return input; return fields[selector.slice(1, -1)]; } };
  const node = {
    querySelector(selector) {
      if (selector === '[data-source-schedule]') return form;
      return fields[selector.slice(1, -1)] || buttons[selector.slice(1, -1)];
    },
    querySelectorAll(selector) {
      if (selector === 'button') return Object.values(buttons);
      if (selector === '[data-source-change]') return [];
      if (selector.startsWith('[data-source-schedule]')) return [input, select, buttons['data-source-remove']];
      throw new Error(`unhandled selector ${selector}`);
    },
  };
  const context = vm.createContext({
    console, URL,
    clearTimeout() {}, setTimeout() { return 1; },
    document: { visibilityState: 'visible', removeEventListener() {} },
    sessionStorage: { setItem(key, value) { storage.set(key, value); }, getItem(key) { return storage.get(key) ?? null; } },
    crypto: { randomUUID: () => 'connection-reference' },
    window: { location: { href: 'https://journal.invalid/app/import/', assign(value) { calls.push({ navigation: value }); } }, AppServices: { escapeHtml: value => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;').replaceAll("'", '&#39;') }, removeEventListener() {} },
    fetch: async (url, options) => {
      calls.push({ url, options });
      const value = options?.method === 'POST' ? { source, accepted: true, submitted: true, reused: false, request_id: JSON.parse(options.body).request_id, reference: 'accepted-sync' }
        : url.includes('/sync/') ? { source, request_id: 'connection-reference', reference: 'accepted-sync', state: 'running', terminal: false }
        : url.includes('/recurrences/') ? { source, enabled: false, entries: [], blocked_names: [] }
        : { source, connection: null };
      return { ok: true, json: async () => value };
    },
    node,
  });
  vm.runInContext(`const rows=new Map(); let disposed=false; const root={isConnected:true};
    const escape=value => window.AppServices.escapeHtml(String(value));
    const sourceUrl=source => '/api/journal/sources/'+source;
    const scheduleUrl=source => '/api/journal/recurrences/source/'+source;
    const row={source:${JSON.stringify(source)},node,busy:false,readable:true,connection:null,schedule:null,revision:0,syncPending:false}; rows.set(row.source,row);`, context);
  for (const name of ['retained', 'read', 'live', 'status', 'controls', 'showConnection', 'showSchedule', 'refresh', 'validConnection', 'change', 'sync', 'checkSync', 'schedule', 'returned', 'leaving', 'dispose']) {
    vm.runInContext(functionSource(script, name), context);
  }
  const run = code => vm.runInContext(code, context);
  return { context, run, storage, calls, fields, buttons, select, input };
}

module.exports = async function sourceConnectionContracts(crateDir) {
  let cases = 0;
  {
    const h = harness(crateDir);
    await h.run('refresh(row)');
    assert.equal(h.calls.length, 2, 'entry reads current connection and recurrence without writing');
    assert.equal(h.run('row.connection'), null);
    assert.equal(h.buttons['data-source-sync'].disabled, true);
    assert.equal(h.input.checked, false, 'no automatic sync default is installed');
    h.run("row.connection={source_identity:'old',folder_name:'<private>',remembered:true,access_state:'ready'}");
    h.context.fetch = async () => ({ ok: false });
    await h.run('refresh(row)');
    assert.equal(h.run('row.connection.source_identity'), 'old', 'unreadable state never becomes a new connection');
    assert.equal(h.run('row.readable'), false);
    assert.equal(h.buttons['data-source-sync'].disabled, true);
    assert.match(h.fields['data-source-status'].textContent, /could not read/);
    cases++;
  }
  {
    const h = harness(crateDir);
    let args;
    h.context.window.JournalOwnerSources = { change: async (...value) => {
      args = value;
      assert.equal(value[4](), true, 'matching reference is retained before native permission work');
      assert.equal(JSON.parse(h.storage.get('journal-source-operation:obsidian')).change, 'connection-reference');
      return { state: 'updated', permissions: 'unchanged' };
    } };
    await h.run("change(row,'begin',row.node.querySelector('[data-source-refresh]'))");
    assert.deepEqual(args.slice(0, 4), ['obsidian', 'begin', null, 'connection-reference']);
    assert.ok(h.calls.every(call => !call.options?.method), 'folder connection changes do not mutate recurrence or start a sync');
    assert.match(h.fields['data-source-status'].textContent, /connection updated/);
    assert.equal(h.buttons['data-source-refresh'].focused, true);
    cases++;
  }
  {
    const h = harness(crateDir);
    h.run("row.connection={source_identity:'displayed',folder_name:'old',remembered:true,access_state:'ready'}");
    h.context.window.JournalOwnerSources = { change: async (...args) => {
      assert.equal(args[2], 'displayed');
      return { state: 'cancelled' };
    } };
    await h.run("change(row,'replace',row.node.querySelector('[data-source-refresh]'))");
    assert.equal(h.run('row.connection.folder_name'), 'old');
    assert.equal(h.calls.length, 0, 'dismissed picker preserves the prior card');
    assert.equal(h.storage.size, 0, 'dismissal before acknowledgement stores no mutation');
    assert.equal(h.run('row.busy'), false, 'picker remains reopenable');
    h.context.sessionStorage.setItem = () => { throw new Error('blocked'); };
    h.context.window.JournalOwnerSources.change = async (...args) => { assert.equal(args[4](), false); return { state: 'refused' }; };
    await h.run("change(row,'replace',row.node.querySelector('[data-source-refresh]'))");
    assert.match(h.fields['data-source-status'].textContent, /could not confirm/);
    cases++;
  }
  {
    const h = harness(crateDir);
    let put;
    h.context.fetch = async (url, options) => {
      h.calls.push({ url, options });
      if (options?.method === 'PUT') put = JSON.parse(options.body);
      return { ok: true, json: async () => url.includes('/recurrences/')
        ? { source: 'obsidian', enabled: true, entries: [{ name: 'sync:obsidian', active: true, entry: { every: '3m', max_runtime: '9m', extra: 42 } }], blocked_names: ['owner:source:obsidian'] }
        : { source: 'obsidian', connection: null } };
    };
    await h.run('refresh(row)');
    assert.equal(h.select.value, '3m', 'custom maintained cadence is reflected without a new default');
    assert.match(h.fields['data-source-conflicts'].textContent, /owner:source:obsidian/);
    h.select.value = '1h';
    await h.run("schedule(row,'PUT')");
    assert.deepEqual(put, { every: '1h', enabled: true }, 'card leaves runtime, argv and custom fields to the shared mutation authority');
    assert.equal(h.calls.filter(call => call.options?.method === 'PUT').length, 1);
    cases++;
  }
  {
    const h = harness(crateDir);
    h.run("row.schedule={enabled:true,entries:[{name:'sync:obsidian',active:true,entry:{every:'1h'}},{name:'owner:source:obsidian',active:true,entry:{every:'15m'}}],blocked_names:[]};showSchedule(row)");
    assert.equal(h.select.value, '', 'mixed active aliases require an explicit cadence choice');
    await h.run("schedule(row,'PUT')");
    assert.equal(h.calls.length, 0, 'Save cannot silently normalize mixed cadences');
    h.select.value = '1d';
    h.input.checked = true;
    await h.run("schedule(row,'PUT')");
    const writes = h.calls.filter(call => call.options?.method === 'PUT');
    assert.equal(writes.length, 1);
    assert.deepEqual(JSON.parse(writes[0].options.body), { every: '1d', enabled: true });
    cases++;
  }
  {
    const h = harness(crateDir);
    for (const active of [true, false]) {
      h.run(`row.schedule={enabled:${active},entries:[{name:'sync:obsidian',active:${active},entry:{every:'3m'}},{name:'owner:source:obsidian',active:${active},entry:{every:'3m'}}],blocked_names:[]};showSchedule(row)`);
      assert.equal(h.select.value, '3m', 'one unique maintained cadence is reflected');
    }
    h.run("row.schedule={enabled:false,entries:[{name:'sync:obsidian',active:false,entry:{every:'1h'}},{name:'owner:source:obsidian',active:false,entry:{every:'15m'}}],blocked_names:[]};showSchedule(row)");
    assert.equal(h.select.value, '', 'mixed disabled aliases also have no single cadence');
    h.run("row.schedule={enabled:true,entries:[{name:'sync:obsidian',active:true,entry:{every:'3m'}},{name:'owner:source:obsidian',active:false,entry:{every:'15m'}}],blocked_names:[]};showSchedule(row)");
    assert.equal(h.select.value, '3m', 'a disabled duplicate does not override the active cadence');
    cases++;
  }
  {
    const h = harness(crateDir);
    await h.run('sync(row)');
    assert.equal(h.calls.filter(call => call.options?.method === 'POST').length, 1);
    assert.equal(JSON.parse(h.storage.get('journal-source-operation:obsidian')).sync.reference, 'accepted-sync');
    assert.match(h.fields['data-source-status'].textContent, /is running/);
    assert.equal(h.buttons['data-source-sync'].disabled, true);
    h.context.fetch = async () => ({ ok: true, json: async () => ({ source: 'obsidian', request_id: 'connection-reference', reference: 'accepted-sync', state: 'finished', terminal: true, exit_status: 'ok' }) });
    await h.run('checkSync(row)');
    assert.match(h.fields['data-source-status'].textContent, /media processing may continue/);
    assert.equal(h.run('row.syncPending'), false);
    cases++;
  }
  {
    const h = harness(crateDir);
    h.context.fetch = async (url, options) => { h.calls.push({ url, options }); throw new Error('delivery lost after acceptance'); };
    await h.run('sync(row)');
    assert.match(h.fields['data-source-status'].textContent, /could not confirm/);
    await h.run('refresh(row)');
    assert.equal(h.calls.filter(call => call.options?.method === 'POST').length, 1, 'return/reload recovery never replays an unknown sync');
    assert.equal(h.run('row.syncPending'), true);
    assert.equal(h.buttons['data-source-sync'].disabled, true);
    assert.equal(JSON.parse(h.storage.get('journal-source-operation:obsidian')).sync.request_id, 'connection-reference');
    h.context.fetch = async () => ({ ok: true, json: async () => ({ source: 'obsidian', request_id: 'connection-reference', reference: 'accepted-sync', state: 'finished', terminal: true, exit_status: 'ok' }) });
    await h.run('checkSync(row)');
    assert.equal(h.run('row.syncPending'), false, 'the persisted request id recovers acceptance even when the POST reply was lost');
    cases++;
  }
  {
    const h = harness(crateDir);
    h.run("retained(row.source,{sync:{request_id:'old-id',reference:'old-sync'}})");
    let reply;
    h.context.fetch = () => new Promise(resolve => { reply = resolve; });
    const earlier = h.run('checkSync(row)');
    h.run("retained(row.source,{sync:{request_id:'new-id',reference:'new-sync'}}); row.syncPending=true;");
    reply({ ok: true, json: async () => ({ source: 'obsidian', request_id: 'old-id', reference: 'old-sync', state: 'finished', terminal: true, exit_status: 'ok' }) });
    await earlier;
    assert.equal(JSON.parse(h.storage.get('journal-source-operation:obsidian')).sync.reference, 'new-sync', 'an old terminal read cannot retire a newer accepted reference');
    assert.equal(h.run('row.syncPending'), true);
    assert.ok(!h.fields['data-source-status'].textContent.includes('finished'));
    cases++;
  }
  {
    const h = harness(crateDir);
    h.run("retained(row.source,{change:'interrupted-folder-change'})");
    await h.run('refresh(row)');
    assert.match(h.fields['data-source-change-status'].textContent,/could not confirm/,'return reads keep a dispatched folder change visibly uncertain');
    assert.equal(h.calls.filter(call => call.options?.method).length,0,'return never replays a source change');
    cases++;
  }
  {
    const h = harness(crateDir);
    h.run("retained(row.source,{sync:{request_id:'prior-id'}})");
    h.context.fetch = async () => ({ok:true,json:async()=>({source:'obsidian',request_id:'prior-id',reference:'derived-prior-ref',state:'unknown',terminal:false})});
    await h.run('checkSync(row)');
    assert.equal(h.run('row.syncPending'),false,'unknown permits a deliberate new owner action without pretending the old action was absent');
    assert.equal(JSON.parse(h.storage.get('journal-source-operation:obsidian')).sync.request_id,'prior-id','unknown keeps its request id for further status reads');
    assert.match(h.fields['data-source-status'].textContent,/could not confirm/);
    h.context.fetch=async()=>({ok:true,json:async()=>({source:'obsidian',request_id:'prior-id',reference:'derived-prior-ref',state:'unknown',terminal:true,exit_status:'ok'})});
    await h.run('checkSync(row)');
    assert.equal(h.run('row.syncPending'),true,'a malformed terminal claim never retires the current request');
    assert.ok(!h.fields['data-source-status'].textContent.includes('finished'));
    cases++;
  }
  {
    const h = harness(crateDir);
    h.context.fetch=async()=>({ok:true,redirected:true,url:'https://journal.invalid/init',json:async()=>{throw new Error('setup HTML must not be parsed as status');}});
    await assert.rejects(h.run("read('/api/journal/sources/obsidian')"));
    assert.deepEqual(h.calls,[{navigation:'/init'}],'the common owner restore/setup gate uses the ordinary setup route');
    cases++;
  }
  return cases;
};
