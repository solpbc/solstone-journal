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
  await assert.rejects(context.fetch('https://journal.invalid/app/import/api/upload',{method:'POST',
    headers:{'content-type':'application/octet-stream'},body:new Uint8Array([0,255,3])}),/file uploads/);
  assert.equal(calls.length,beforeEarly,'unsupported binary writes must be refused before text conversion');
  console.log('owner fetch: representation, Unicode, redirect, stale reply, origin and abort passed');
}
main().catch(error=>{console.error(error);process.exitCode=1;});
