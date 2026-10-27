// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(require('node:path').join(__dirname, '../owner-stream.js'), 'utf8');

function hangingBody() {
  const state = {cancelled: 0};
  const stream = new ReadableStream({
    pull() { return new Promise(() => {}); },
    cancel() { state.cancelled += 1; },
  });
  return {stream, state};
}

async function waitFor(predicate) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  throw new Error('timed out waiting for the upload to arm');
}

async function main() {
  const messages = [];
  let issued = 4;
  let finishMode = 'ready';
  let now = 0;
  let folderSequence = 0;
  const folderReplies = new Map();
  const context = {
    crypto: { randomUUID() { return `folder-${++folderSequence}`; } },
    Date: {now() { return now; }},
    Request, Response, Headers, URL, TextEncoder, Uint8Array, DOMException, fetch,
    setTimeout, clearTimeout, addEventListener() {},
    atob(value) { return Buffer.from(value, 'base64').toString('binary'); },
    location: {href: 'journal-owner://local/init'},
    webkit: {messageHandlers: {journalOwner: {postMessage(message) {
      messages.push(JSON.parse(JSON.stringify(message)));
      if (message.op === 'bind') return {ok: true, authority: issued};
      if (message.op === 'chooseFolder' || message.op === 'chooseSource') {
        return new Promise(resolve => folderReplies.set(message.id, {message, resolve}));
      }
      if (message.op === 'open') return {ok: true};
      if (message.op === 'chunk') return {accepted: true};
      if (message.op === 'finish') {
        if (finishMode === 'hold') {
          finishMode = 'release';
          now += 120000;
          return {pending: true};
        }
        if (finishMode === 'release') {
          finishMode = 'ready';
          return {
            status: 200, headers: {'content-type': 'application/json'},
            body: Buffer.from('{"kept":true}').toString('base64'), url: message.url,
          };
        }
        if (finishMode === 'pending-cancel') return {pending: true};
        if (finishMode === 'lose') {
          return {error: 'journal response unavailable; check the result before trying again'};
        }
        return {
          status: 200, headers: {'content-type': 'application/json'},
          body: Buffer.from('{"kept":true}').toString('base64'), url: message.url,
        };
      }
      return {ok: true};
    }}}},
  };
  context.window = context;
  vm.runInNewContext(source, context);

  const payload = new Uint8Array([0, 255, 65]);
  const saved = await context.fetch('journal-owner://local/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'}, body: payload,
  });
  assert.equal(saved.status, 200);
  assert.deepEqual(await saved.json(), {kept: true});
  assert.equal(messages.filter((message) => message.op === 'bind').length, 1);
  assert.equal(messages.find((message) => message.op === 'open').authority, 4);

  finishMode = 'hold';
  const held = await context.fetch('journal-owner://local/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'}, body: new Uint8Array([7]),
  });
  assert.equal(held.status, 200);
  assert.deepEqual(await held.json(), {kept: true});
  assert.ok(now >= 120000);
  assert.equal(messages.filter((message) => message.op === 'cancel' && message.id === '2').length, 0);

  finishMode = 'pending-cancel';
  const cancelController = new AbortController();
  const cancelDuringSave = context.fetch('journal-owner://local/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'},
    body: new Uint8Array([8]), duplex: 'half', signal: cancelController.signal,
  });
  await waitFor(() => messages.some((message) => message.op === 'finish' && message.id === '3'));
  cancelController.abort();
  await assert.rejects(cancelDuringSave, /aborted/);
  const saveCancel = messages.filter((message) => message.op === 'cancel' && message.id === '3');
  assert.equal(saveCancel.length, 1);
  assert.equal(saveCancel[0].authority, 4);

  finishMode = 'lose';
  await assert.rejects(context.fetch('journal-owner://local/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'}, body: new Uint8Array([9]),
  }), /journal response unavailable/);
  assert.equal(messages.filter((message) => message.op === 'cancel' && message.id === '4').length, 1);
  finishMode = 'ready';

  issued = 9;
  context.__journalAuthority = 9;
  const hungAbort = hangingBody();
  const abortController = new AbortController();
  const abortDuringRead = context.fetch('journal-owner://local/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'},
    body: hungAbort.stream, duplex: 'half', signal: abortController.signal,
  });
  await waitFor(() => messages.some((message) => message.op === 'open' && message.id === '5'));
  await new Promise((resolve) => setTimeout(resolve, 0));
  abortController.abort();
  await assert.rejects(abortDuringRead, /aborted/);
  assert.ok(hungAbort.state.cancelled >= 1);
  const abortCancel = messages.filter((message) => message.op === 'cancel' && message.id === '5');
  assert.equal(abortCancel.length, 1);
  assert.equal(abortCancel[0].authority, 4);

  const realSetTimeout = context.setTimeout;
  const realClearTimeout = context.clearTimeout;
  const stalledTimers = [];
  context.setTimeout = (fn, ms, ...args) => {
    if (ms >= 1000) {
      const handle = {fn, ms, cleared: false};
      stalledTimers.push(handle);
      return handle;
    }
    return realSetTimeout(fn, ms, ...args);
  };
  context.clearTimeout = (handle) => {
    if (handle && typeof handle === 'object' && handle.fn) { handle.cleared = true; return; }
    return realClearTimeout(handle);
  };
  const hungStall = hangingBody();
  const stalled = context.fetch('journal-owner://local/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'},
    body: hungStall.stream, duplex: 'half',
  });
  await waitFor(() => stalledTimers.length === 1);
  assert.ok(stalledTimers[0].ms >= 1000 && stalledTimers[0].ms <= 60000);
  stalledTimers[0].fn();
  await assert.rejects(stalled, /transfer stopped/);
  assert.ok(hungStall.state.cancelled >= 1);
  const stallCancel = messages.filter((message) => message.op === 'cancel' && message.id === '6');
  assert.equal(stallCancel.length, 1);
  assert.equal(stallCancel[0].authority, 4);
  assert.equal(messages.filter((message) => message.op === 'bind').length, 1);
  const connected = context.JournalOwnerSources.change('obsidian', 'begin', null, 'retained-source-reference', () => true);
  await waitFor(() => folderReplies.size === 1);
  const selected = folderReplies.get('folder-1');
  assert.deepEqual(JSON.parse(JSON.stringify(selected.message)), {op: 'chooseSource', authority: 4, id: 'folder-1', source: 'obsidian', action: 'begin', expected_identity: null, client_item_id: 'retained-source-reference'});
  assert.equal(context.__journalFolderSelected(9, 'folder-1'), false, 'another page cannot acknowledge this source');
  assert.equal(context.__journalFolderSelected(4, 'folder-1'), true);
  selected.resolve({state: 'updated', connection: {folder_name: 'picked notes'}});
  assert.equal((await connected).state, 'updated');
  folderReplies.clear();

  const forgotten = context.JournalOwnerSources.change('audio', 'forget', 'displayed-identity', 'forget-reference', () => false);
  await waitFor(() => folderReplies.has('folder-2'));
  assert.equal(context.__journalFolderSelected(4, 'folder-2'), false, 'a failed retention acknowledgement must refuse native permission changes');
  context.JournalOwnerSources.cancel();
  assert.deepEqual(JSON.parse(JSON.stringify(await forgotten)), {state: 'outcome_unknown', client_item_id: 'forget-reference'});
  const later = context.JournalOwnerSources.change('audio', 'reauthorize', 'displayed-identity', 'later-reference', () => undefined);
  await waitFor(() => folderReplies.has('folder-3'));
  folderReplies.get('folder-2').resolve({state: 'updated'});
  assert.equal(context.__journalFolderSelected(4, 'folder-3'), false, 'only explicit true acknowledges retained custody');
  folderReplies.get('folder-3').resolve({state: 'cancelled'});
  assert.equal((await later).state, 'cancelled', 'a late old reply cannot settle a newer selection');
  console.log('owner stream: saved bytes, post-EOF completion, pending-read cancel, and stalled read passed');
}

main().catch((error) => { console.error(error); process.exitCode = 1; });
