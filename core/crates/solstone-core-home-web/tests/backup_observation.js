// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Payloads arrive from real admitted execution and Home HTTP composition.
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const cases = JSON.parse(fs.readFileSync(0, 'utf8'));
const manifest = process.argv[2];
let source = fs.readFileSync(path.join(manifest, 'assets/home.js'), 'utf8');
const anchor = '  window.selectAccountSide = selectAccountSide;\n';
assert(source.includes(anchor));
source = source.replace(anchor, `  window.__backup = {observeBackup, renderVitalsHtml, homeHealthGlance, receiveSupervisorStatus,
  fail: value => { pulseReadFailed=value; }, refreshVitals,
  seed: value => { lastPulse=value; observeBackup(value); pulseReadFailed=false; pulseObservedAt=Date.now(); pulseRequest=null; vitalsRequest=null; narrativeRequest=null; }};
${anchor}`);
const document = {readyState:'loading',addEventListener(){},querySelector(){return null;},getElementById(){return null;},querySelectorAll(){return [];}};
const window = {document,location:{href:''},addEventListener(){},setTimeout(){return 1;},clearTimeout(){},apiJson(){return new Promise(()=>{});}};
window.window=window;document.defaultView=window;
vm.runInNewContext(source,{window,document,console,Date},{filename:'home.js'});
const ui=window.__backup;
ui.receiveSupervisorStatus({event:'status',queues:{},held:[]});
function backupIssues(payload) { return ui.homeHealthGlance(payload).glance.issues.filter(issue => issue.href === '/app/health/#backup'); }
function render(name) {ui.seed(cases[name]); return ui.renderVitalsHtml(cases[name]);}
assert(cases.current.backup.issue && cases.operated.backup.issue);
assert(!cases.superseded.backup.issue);
for (const name of ['current','operated','binding_restored']) {
  const html=render(name);assert(html.includes('href="/app/health/#backup"'),html);
  assert.strictEqual(ui.homeHealthGlance(cases[name]).glance.verdict,'attention');
}
for (const name of ['superseded','credentials_changed','success','off','legacy']) {
  render(name);assert.strictEqual(backupIssues(cases[name]).length,0,name);
}
render('current');assert(render('malformed_config').includes('last known:'));
render('current');const unavailable=render('unknown');
assert(unavailable.includes('last known:'),unavailable);
assert(ui.homeHealthGlance(cases.unknown).notes.some(note => note.href === '/app/health/#backup'));
const malformed=structuredClone(cases.current);malformed.backup.issue=null;
ui.seed(cases.current);ui.observeBackup(malformed);
assert(ui.renderVitalsHtml(malformed).includes('last known:'));
assert(ui.homeHealthGlance(malformed).notes.some(note=>note.href==='/app/health/#backup'));
render('operated');assert(render('binding_unknown').includes('last known:'));
assert(ui.homeHealthGlance(cases.binding_unknown).notes.some(note => note.href === '/app/health/#backup'));
assert(!render('binding_restored').includes('last known:'));
ui.fail(true);const failed=ui.renderVitalsHtml(cases.binding_restored);assert(failed.includes('last known:'));
ui.fail(false);
const priority=structuredClone(cases.current);
priority.health_glance.issues=[{text:'existing service issue',severity:'red',href:'/app/health/'}];
ui.seed(priority);const ordered=ui.homeHealthGlance(priority).glance;
assert.strictEqual(ordered.severity,'red');assert.strictEqual(ordered.issues[0].text,'existing service issue');
assert.strictEqual(ordered.issues.length,2);
for (const payload of Object.values(cases)) {
 assert(!JSON.stringify(payload).includes('synthetic-secret'));
 assert(!JSON.stringify(payload).includes('synthetic-broker-token'));
}
async function refreshProof() {
 ui.seed(cases.current);window.apiJson=async()=>cases.superseded;
 await ui.refreshVitals();assert.strictEqual(backupIssues(cases.superseded).length,0);
 window.apiJson=async()=>cases.current;await ui.refreshVitals();assert.strictEqual(backupIssues(cases.current).length,1);
 window.apiJson=async()=>{throw new Error('synthetic read failure');};await ui.refreshVitals();
 assert(ui.renderVitalsHtml(cases.current).includes('last known:'));
 window.apiJson=async()=>cases.success;await ui.refreshVitals();assert.strictEqual(backupIssues(cases.success).length,0);
 console.log('PASS: admitted current/superseded HTTP lineage, credential change, quiet controls, binding uncertainty/restoration, last-known, priority, refresh and recovery');
}
refreshProof().catch(error=>{console.error(error);process.exitCode=1;});
