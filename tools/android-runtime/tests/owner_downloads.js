// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(process.argv[2] || require('node:path').join(__dirname, '../assets/owner-downloads.js'), 'utf8');
async function exercise(bytes, refuseChunk = false, retire = false, streamError = false) {
  const calls = [], notices = [], listeners = {};
  let click, done, timer;
  const finished = new Promise((resolve, reject) => {
    done = resolve;
    timer = setTimeout(() => reject(new Error('native transfer was never ended')), 2000);
  });
  const context = { Blob, Map, URL: class extends URL {}, btoa,
    __journalOwnerToken: 'page-one',
    document: {addEventListener(type, handler) { assert.equal(type, 'click');click = handler; }},
    addEventListener(type, handler) { listeners[type] = handler; },
    alert(message) { notices.push(message); },
    JournalOwner: {
      beginDownload(token, name, mime, size) { calls.push({token, name, mime, size});return 'export-one'; },
      downloadChunk(token, id, chunk) {
        const decoded = Buffer.from(chunk, 'base64');
        assert(decoded.length <= 32768);
        assert.equal(token, 'page-one');assert.equal(id, 'export-one');
        calls.push(decoded);
        return !refuseChunk;
      },
      endDownload(token, id, complete) { done(complete); },
    },
  };
  context.window = context;
  vm.runInNewContext(source, context);
  assert.equal(typeof click, 'function');
  const blob = new Blob([bytes], {type: 'text/plain'});
  if (streamError) blob.stream = () => new ReadableStream({start(controller) {controller.error(new Error("source lost"));}});
  const href = context.URL.createObjectURL(blob);
  const link = {href, download: 'journal-copy.txt'};
  let prevented = false;
  click({target: {closest() {return link;}}, preventDefault() {prevented = true;}});
  // Health's actual caller revokes synchronously after click.
  context.URL.revokeObjectURL(href);
  if (retire) listeners.pagehide();
  assert(prevented);
  let complete;
  try { complete = await finished; }
  finally { clearTimeout(timer); }
  assert.equal(complete, !refuseChunk && !retire && !streamError);
  assert.equal(calls[0].size, bytes.length);
  if (complete) assert.deepEqual(Buffer.concat(calls.slice(1)), bytes);
  if (!streamError) assert.deepEqual(notices, []);
}
(async () => {
  await exercise(Buffer.from('complete recovery key\n'));
  await exercise(Buffer.from(Array.from({length: 200003}, (_, i) => i % 256)));
  await exercise(Buffer.alloc(70000, 13), true);
  await exercise(Buffer.alloc(70000, 17), false, true);
  await exercise(Buffer.from("failed source"), false, false, true);
  console.log('owner downloads: exact bytes, immediate revocation, bounded chunks, refusal and page retirement pass');
})().catch(error => { console.error(error);process.exitCode = 1; });
