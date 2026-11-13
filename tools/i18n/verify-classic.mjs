// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const [directory,expectedFile]=process.argv.slice(2);
const spec=JSON.parse(fs.readFileSync(expectedFile,'utf8'));
assert.ok(spec.cases.length>0);
const entry=spec.entry.replace(/^esm\//,'classic/').replace(/api\.mjs$/,'api.js');
const context=vm.createContext({Intl});
vm.runInContext(fs.readFileSync(path.join(directory,entry),'utf8'),context);
const api=context.SolstoneI18nBundles[entry.split('/')[1]];
for(const item of spec.cases) {
 const render=()=>api.render(item.locale,item.message_id,item.arguments);
 if(item.expected_error)assert.throws(render,item.error_contains?new RegExp(item.error_contains):undefined);
 else {
  assert.equal(render(),item.expected);
  assert.equal(api.render(item.locale,item.message_id,item.arguments,{useIsolating:false}),item.expected.replace(/[\u2068\u2069]/g,''));
  // Exercise both bundle cache modes in succession.
  assert.equal(render(),item.expected);
 }
}
console.log(`classic runtime: ${spec.cases.length} independent golden cases passed`);
