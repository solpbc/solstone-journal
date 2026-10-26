// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(require('node:path').join(__dirname, '../assets/owner.js'), 'utf8');

async function main() {
  const calls = [], cancellations = [];
  let nextCall;
  const context = {Request, Response, Headers, URL, TextEncoder, Uint8Array, DOMException, atob,
    setTimeout, clearTimeout, location: {href: 'https://journal.invalid/init', origin: 'https://journal.invalid'},
    JournalOwner: {
      fetch(encoded) { const request = JSON.parse(encoded); calls.push(request); nextCall?.(request); },
      cancel(token, id) { cancellations.push({token, id}); },
    },
    __journalOwnerToken: 'current-page', addEventListener() {},
  };
  context.window = context;
  vm.runInNewContext(source, context);
  const request = (url, options) => {
    const submitted = new Promise(resolve => { nextCall = resolve; });
    const response = context.fetch(url, options);
    return {submitted, response};
  };
  const reply = (call, value, extra={}) => context.__journalOwnerReply('current-page', call.id, {
    status: 200, headers: {'content-type':'application/json'}, url: call.url,
    body: Buffer.from(JSON.stringify(value)).toString('base64'), ...extra,
  });
  const first = request('https://journal.invalid/init/mark/lock', {
    method:'POST', headers:{'content-type':'application/json'}, body:JSON.stringify({text:'café 🐦'}),
  });
  const call = await first.submitted;
  assert.equal(JSON.parse(call.body).text, 'café 🐦');
  assert.equal(new Map(call.headers).get('content-type'), 'application/json');
  context.__journalOwnerReply('previous-page',call.id,{error:'stale response'});
  reply(call,{kept:'café 🐦'},{url:'https://journal.invalid/init'});
  const response=await first.response;
  assert.equal(response.redirected,true); assert.equal(response.url,'https://journal.invalid/init');
  assert.deepEqual(await response.json(),{kept:'café 🐦'});
  const form=request('https://journal.invalid/init/finalize',{method:'POST',body:'{}'});
  const formCall=await form.submitted;
  assert.match(new Map(formCall.headers).get('content-type'),/^text\/plain/);
  reply(formCall,{error:'rejected'},{status:415});
  assert.equal((await form.response).status,415);
  const before=calls.length;
  await assert.rejects(context.fetch('https://outside.invalid/'),/private journal/);
  assert.equal(calls.length,before);
  const controller=new AbortController();
  const aborted=request('https://journal.invalid/app/backup/api/restore',{method:'POST',
    headers:{'content-type':'application/json'},body:'{}',signal:controller.signal});
  await aborted.submitted; controller.abort();
  await assert.rejects(aborted.response,/aborted/);
  assert.equal(cancellations.length,1);
  assert.equal(calls.length,before+1,'lost mutation must not be replayed');
  const earlyController=new AbortController();
  const beforeEarly=calls.length;
  const early=context.fetch('https://journal.invalid/init/finalize',{method:'POST',body:'{}',signal:earlyController.signal});
  earlyController.abort();
  await assert.rejects(early,/aborted/);
  assert.equal(calls.length,beforeEarly,'abort during body read must not submit a mutation');
  const payload = new Uint8Array(300 * 1024);
  payload[0] = 0; payload[1] = 255; payload[payload.length - 1] = 255;
  const chunks = [];
  const opens = [];
  context.JournalOwner.openStream = encoded => { opens.push(JSON.parse(encoded)); return JSON.stringify({ok:true}); };
  context.JournalOwner.writeChunk = (_token, _id, text) => { chunks.push(text); return JSON.stringify({accepted:true}); };
  context.JournalOwner.finishStream = () => JSON.stringify({
    status: 200, headers: {'content-type':'application/json'},
    url: 'https://journal.invalid/app/import/api/save',
    body: Buffer.from('{"kept":true}').toString('base64'),
  });
  const saved = await context.fetch('https://journal.invalid/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'}, body: payload,
  });
  assert.equal(saved.status, 200);
  assert.deepEqual(await saved.json(), {kept: true});
  assert.equal(opens.length, 1);
  assert.equal(opens[0].body, undefined);
  assert.ok(payload.length > 256 * 1024);
  const joined = Buffer.concat(chunks.map(text => {
    const bytes = Buffer.alloc(text.length);
    for (let index = 0; index < text.length; index += 1) bytes[index] = text.charCodeAt(index);
    return bytes;
  }));
  assert.deepEqual(joined, Buffer.from(payload));
  assert.equal(calls.length, beforeEarly, 'file bytes stay out of the one-shot fetch');
  const streamController = new AbortController();
  let streamWrites = 0;
  context.JournalOwner.writeChunk = () => {
    streamWrites += 1;
    streamController.abort();
    return JSON.stringify({accepted: true});
  };
  const beforeStream = opens.length;
  await assert.rejects(context.fetch('https://journal.invalid/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'multipart/form-data; boundary=bound'},
    body: payload, signal: streamController.signal,
  }), /aborted/);
  assert.equal(opens.length, beforeStream + 1, 'abort must not open a second save');
  assert.equal(streamWrites, 1);
  assert.ok(cancellations.some(item => item.id === opens[opens.length - 1].id));

  context.__journalOwnerToken = 'later-page';
  const hungAbort = hangingBody();
  const abortController = new AbortController();
  const abortDuringRead = context.fetch('https://journal.invalid/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'},
    body: hungAbort.stream, duplex: 'half', signal: abortController.signal,
  });
  abortController.abort();
  await assert.rejects(abortDuringRead, /aborted/);
  assert.ok(hungAbort.state.cancelled >= 1, 'cancel must abandon the pending read');
  assert.equal(opens[opens.length - 1].token, 'current-page');
  assert.equal(cancellations[cancellations.length - 1].token, 'current-page');
  assert.equal(cancellations[cancellations.length - 1].id, opens[opens.length - 1].id);

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
  const stalled = context.fetch('https://journal.invalid/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'},
    body: hungStall.stream, duplex: 'half',
  });
  assert.equal(stalledTimers.length, 1);
  assert.ok(stalledTimers[0].ms >= 1000 && stalledTimers[0].ms <= 60000);
  stalledTimers[0].fn();
  await assert.rejects(stalled, /transfer stopped/);
  assert.ok(hungStall.state.cancelled >= 1, 'stall must abandon the pending read');
  assert.equal(opens[opens.length - 1].token, 'current-page');
  assert.equal(cancellations[cancellations.length - 1].token, 'current-page');
  assert.equal(cancellations[cancellations.length - 1].id, opens[opens.length - 1].id);
  assert.ok(!cancellations.some(item => item.token === 'later-page'));

  let now = 0;
  context.Date = {now() { return now; }};
  let finishMode = 'hold';
  context.JournalOwner.finishStream = () => {
    if (finishMode === 'hold') {
      finishMode = 'release';
      now += 120000;
      return JSON.stringify({pending: true});
    }
    if (finishMode === 'pending-cancel') return JSON.stringify({pending: true});
    if (finishMode === 'lose') {
      return JSON.stringify({error: 'journal response unavailable; check the result before trying again'});
    }
    return JSON.stringify({
      status: 200, headers: {'content-type':'application/json'},
      url: 'https://journal.invalid/app/import/api/save',
      body: Buffer.from('{"kept":true}').toString('base64'),
    });
  };
  const held = await context.fetch('https://journal.invalid/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'}, body: new Uint8Array([7]),
  });
  assert.equal(held.status, 200);
  assert.deepEqual(await held.json(), {kept: true});
  assert.ok(now >= 120000);
  const heldId = opens[opens.length - 1].id;
  assert.ok(!cancellations.some(item => item.id === heldId));

  finishMode = 'pending-cancel';
  const saveController = new AbortController();
  const cancelDuringSave = context.fetch('https://journal.invalid/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'},
    body: new Uint8Array([8]), duplex: 'half', signal: saveController.signal,
  });
  const saveId = opens[opens.length - 1].id;
  await new Promise(resolve => setTimeout(resolve, 0));
  saveController.abort();
  await assert.rejects(cancelDuringSave, /aborted/);
  assert.equal(cancellations.filter(item => item.id === saveId && item.token === 'current-page').length, 1);

  finishMode = 'lose';
  const beforeLoss = cancellations.length;
  await assert.rejects(context.fetch('https://journal.invalid/app/import/api/save', {
    method: 'POST', headers: {'content-type': 'application/octet-stream'}, body: new Uint8Array([9]),
  }), /journal response unavailable/);
  assert.equal(cancellations.length, beforeLoss + 1);
  assert.equal(cancellations[cancellations.length - 1].token, 'current-page');
  console.log('owner fetch: representation, Unicode, redirect, stale reply, origin, abort, binary upload, stalled read, and post-EOF completion passed');
}

function hangingBody() {
  const state = {cancelled: 0};
  const stream = new ReadableStream({
    pull() { return new Promise(() => {}); },
    cancel() { state.cancelled += 1; },
  });
  return {stream, state};
}
main().catch(error=>{console.error(error);process.exitCode=1;});
