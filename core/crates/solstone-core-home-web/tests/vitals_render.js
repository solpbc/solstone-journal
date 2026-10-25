// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const manifestDir = process.argv[2];
if (!manifestDir) throw new Error('manifest directory required');

let source = fs.readFileSync(path.join(manifestDir, 'assets/home.js'), 'utf8');
source = source.replace(
  '  window.selectAccountSide = selectAccountSide;\n',
  `  window.__vitals = { renderVitalsHtml, homeHealthGlance, receiveSupervisorStatus, invalidateHeldWork, refreshVitals, refreshNarrative, refreshChangedPulse,
    captureNarrative: () => { renderNarrative = data => { window.__narrative = data.narrative_content; }; },
    seedPulse: data => { lastPulse=data; pulseObservedAt=Date.now(); pulseReadFailed=false; pulseRequest=null; vitalsRequest=null; narrativeRequest=null; } };
  window.selectAccountSide = selectAccountSide;\n`,
);

let now = 1000;
let expiry;
class TestDate extends Date {
  constructor(...args) { super(...(args.length ? args : [now])); }
  static now() { return now; }
}
const location = { href: '' };
const document = {
  readyState: 'loading',
  addEventListener() {},
  querySelector() { return null; },
  getElementById() { return null; },
  querySelectorAll() { return []; },
};
const window = {
  location,
  document,
  addEventListener() {},
  apiJson() { return new Promise(() => {}); },
  clearTimeout() {},
  setTimeout(fn) { expiry=fn; return 1; },
};
window.window = window;
document.defaultView = window;

vm.runInNewContext(source, { window, document, console, Date:TestDate }, { filename: 'home.js' });
const { renderVitalsHtml } = window.__vitals;
assert(renderVitalsHtml, 'renderVitalsHtml exported');
const { homeHealthGlance, receiveSupervisorStatus, invalidateHeldWork, refreshVitals, seedPulse } = window.__vitals;

const okPulse = { health_glance: { verdict: 'ok', severity: 'green', headline: 'working', issues: [] }, narrative_content: null, needs_you_items: [] };
for (const testNow of [1000, 1000 + 14999]) {
  now = testNow;
  const res = homeHealthGlance(okPulse);
  assert.notStrictEqual(res.glance.verdict, 'unavailable');
  assert.ok(!res.notes.some(n => n.href === '/app/health/#processingHolds'));
}
now = 1000 + 15000;
const expiredGrace = homeHealthGlance(okPulse);
assert.strictEqual(expiredGrace.glance.verdict, 'unavailable');
assert.ok(expiredGrace.notes.some(n => n.href === '/app/health/#processingHolds'));

now = 1000;
receiveSupervisorStatus({
  event: 'status',
  queues: { daily: 1 },
  held: [{ partition: 'daily', reasons: ['root_live'] }]
});
const heldRes = homeHealthGlance(okPulse);
assert.strictEqual(heldRes.glance.verdict, 'attention');
assert.ok(heldRes.glance.issues.some(i => i.href === '/app/health/#processingHolds'));

receiveSupervisorStatus({event:'status'}); // authoritative empty queues for ordinary controls

const css = fs.readFileSync(path.join(manifestDir, 'assets/workspace.html'), 'utf8');
assert(css.includes('.pulse-vitals-dot.neutral'), 'missing .pulse-vitals-dot.neutral');
assert(css.includes('.pulse-vitals-verdict.neutral'), 'missing .pulse-vitals-verdict.neutral');

const calm = renderVitalsHtml({
  health_glance: {
    verdict: 'calm',
    severity: 'neutral',
    headline: 'no devices are running the solstone app yet. set one up to start your journal.',
    issues: [],
    cta: { text: 'set one up →', href: '/app/network/' },
  },
});
assert(calm.includes('pulse-vitals-dot neutral'), calm);
assert(calm.includes('pulse-vitals-verdict neutral'), calm);
assert.strictEqual(calm.includes('pulse-waiting'), false, calm);
assert.strictEqual(calm.includes('pulse-vitals-dot green'), false, calm);
assert.strictEqual(calm.includes('pulse-vitals-verdict green'), false, calm);
assert.strictEqual(calm.includes('pulse-vitals-dot amber'), false, calm);
assert.strictEqual(calm.includes('pulse-vitals-verdict amber'), false, calm);

const green = renderVitalsHtml({
  health_glance: {
    verdict: 'ok',
    severity: 'green',
    headline: "everything's working",
    issues: [],
  },
});
assert(green.includes('pulse-vitals-dot green'), green);
assert(green.includes('pulse-vitals-verdict green'), green);
assert.strictEqual(green.includes('pulse-waiting'), false, green);

const amber = renderVitalsHtml({
  health_glance: {
    verdict: 'attention',
    severity: 'amber',
    headline: '1 thing needs your attention',
    issues: [{ text: "the solstone app on one of your devices hasn't added anything to your journal recently.", severity: 'amber', href: '/app/health' }],
  },
});
assert(amber.includes('pulse-vitals-dot amber'), amber);
assert(amber.includes('pulse-vitals-verdict amber'), amber);
// A health issue is a row of what's waiting, under a headline that names the list.
assert((amber.match(/class="pulse-waiting-row"/g) || []).length === 1, amber);
assert(amber.includes('aria-labelledby="pulse-status-headline"'), amber);
assert.strictEqual(amber.includes('pulse-waiting-heading'), false, amber);

const red = renderVitalsHtml({
  health_glance: {
    verdict: 'attention',
    severity: 'red',
    headline: '1 thing needs your attention',
    issues: [{ text: "the solstone app hasn't added anything to your journal recently.", severity: 'red', href: '/app/health' }],
  },
});
assert(red.includes('pulse-vitals-dot red'), red);
assert(red.includes('pulse-vitals-verdict red'), red);
assert(red.includes('pulse-waiting-row red'), red);

// Not yet, and that's fine: the note rides any verdict as a calm row, never a
// chip and never in the count (the day-0 rule).
const noted = renderVitalsHtml({
  health_glance: {
    verdict: 'attention',
    severity: 'amber',
    headline: '1 thing needs your attention',
    issues: [{ text: 'thinking needs a key', severity: 'amber', href: '/app/thinking/' }],
    note: { text: 'your journal starts catching up once you choose where the thinking happens.', href: '/app/health/#backlogVerdict' },
    cta: { text: 'set one up →', href: '/app/network/#devices' },
  },
});
// A journal with no devices keeps the way to add one beside any count.
assert(noted.includes('<a class="pulse-vitals-item pulse-vitals-cta" href="/app/network/#devices">'), noted);
assert(noted.includes('<a class="pulse-vitals-note" href="/app/health/#backlogVerdict">'), noted);
assert(noted.includes('pulse-vitals-ring'), noted);
assert(noted.includes('your journal starts catching up once you choose where the thinking happens. →'), noted);
assert.strictEqual((noted.match(/pulse-waiting-row/g) || []).length, 1, noted);
const calmNoted = renderVitalsHtml({
  health_glance: {
    verdict: 'calm',
    severity: 'neutral',
    headline: 'the solstone app is adding to your journal.',
    last_observation: '2 minutes ago',
    issues: [],
    note: { text: "you'll see whether your journal is caught up after its first night.", href: '/app/health/#backlogVerdict' },
  },
});
assert(calmNoted.includes('pulse-vitals-note'), calmNoted);
assert(calmNoted.includes('last reached your journal 2 minutes ago'), calmNoted);
assert.strictEqual(calmNoted.includes('pulse-vitals-dot green'), false, calmNoted);
assert.strictEqual(amber.includes('pulse-vitals-note'), false, amber);
assert(css.includes('.pulse-vitals > a.pulse-vitals-note'), 'missing note row style');


async function issueReadCases() {
  const ordinary={health_glance:{verdict:'ok',severity:'green',headline:'working',issues:[]},narrative_content:null,needs_you_items:[]};
  seedPulse(ordinary);
  invalidateHeldWork();
  assert.strictEqual(homeHealthGlance(ordinary).glance.verdict, 'unavailable');
  assert.strictEqual(homeHealthGlance(ordinary).glance.issues.length, 0);
  const status={event:'status',queues:{daily:2,segment:1},held:[{partition:'daily',reasons:['root_live']}],tasks:[{partition:'segment'}]};
  receiveSupervisorStatus(status);
  let g=homeHealthGlance(ordinary).glance;
  assert.strictEqual(g.verdict, 'attention');
  assert.strictEqual(g.issues.length, 1);
  assert.strictEqual(g.issues[0].href, '/app/health/#processingHolds');
  receiveSupervisorStatus({...status,queue_hold:{reason:'records_unreadable'},held:[...status.held,{partition:'segment'}]});
  assert.strictEqual(homeHealthGlance(ordinary).glance.issues.length, 3, 'retain global and partition scopes');
  const diagnosed={...ordinary,health_glance:{verdict:'attention',severity:'red',headline:'known fault',issues:[{text:'known device fault',severity:'red',href:'/app/health/#registeredClientsCard'}],note:{text:'not yet',href:'/app/health/'}}};
  const combined=homeHealthGlance(diagnosed).glance;
  assert.strictEqual(combined.issues[0].text, 'known device fault');
  assert.strictEqual(combined.severity, 'red');
  assert.strictEqual(combined.note.text, 'not yet');
  now += 30000;
  g=homeHealthGlance(ordinary).glance;
  assert.strictEqual(g.verdict, 'unavailable', 'clock expiry applies even before the timer is delivered');
  assert.ok(g.issues.every(issue=>issue.text.startsWith('last known:')));
  expiry();
  receiveSupervisorStatus({event:'held',partition:'daily'});
  assert.strictEqual(homeHealthGlance(ordinary).glance.verdict, 'unavailable', 'one-off held event is not an authority');
  receiveSupervisorStatus(status);
  receiveSupervisorStatus({event:'status-error'});
  assert.strictEqual(homeHealthGlance(ordinary).glance.verdict, 'unavailable');
  assert.ok(homeHealthGlance(ordinary).notes[0].text.includes('last check:'));
  receiveSupervisorStatus({...status,queues:{}});
  assert.strictEqual(homeHealthGlance(ordinary).glance.verdict, 'ok', 'empty holds stay quiet');
  receiveSupervisorStatus({event:'status',queues:{daily:2}});
  assert.strictEqual(homeHealthGlance(ordinary).glance.issues.length, 0, 'omitted optional fields clear the hold');
  const progressing={...ordinary,health_glance:{verdict:'progressing',severity:'amber',headline:'processing',issues:[]}};
  assert.strictEqual(homeHealthGlance(progressing).glance.verdict, 'progressing');
  seedPulse(diagnosed);
  seedPulse(diagnosed);
  window.apiJson=()=>Promise.reject(new Error('synthetic read failed'));
  await refreshVitals();
  g=homeHealthGlance(diagnosed).glance;
  assert.strictEqual(g.verdict, 'unavailable');
  assert.ok(g.issues[0].text.startsWith('last known:'));
  assert.strictEqual(g.issues[0].href, '/app/health/#registeredClientsCard');
  // Event bursts share one read and one paint per section; a later event
  // after settlement can observe new state.
  let resolveRead;
  let requests = 0;
  window.apiJson=()=>{ requests += 1; return new Promise(resolve=>{resolveRead=resolve;}); };
  const firstRead=refreshVitals();
  const sharedRead=refreshVitals();
  window.__vitals.captureNarrative();
  const narrativeRead=window.__vitals.refreshNarrative();
  assert.strictEqual(firstRead, sharedRead);
  assert.strictEqual(requests, 1);
  resolveRead({...ordinary,narrative_content:'new narrative'});
  await Promise.all([firstRead, sharedRead, narrativeRead]);
  assert.strictEqual(homeHealthGlance(ordinary).glance.verdict, 'ok');
  assert.strictEqual(window.__narrative, 'new narrative');
  window.apiJson=()=>{ requests += 1; return Promise.resolve(diagnosed); };
  await refreshVitals();
  assert.strictEqual(requests, 2, 'a later event reads fresh state');
  let oldResult, newResult;
  window.apiJson=()=>new Promise(resolve=>{ oldResult=resolve; });
  const oldRead=refreshVitals();
  const afterFinish=window.__vitals.refreshChangedPulse();
  assert.strictEqual(window.__vitals.refreshChangedPulse(), afterFinish, 'invalidations queue one trailing read');
  window.apiJson=()=>new Promise(resolve=>{ newResult=resolve; });
  oldResult(ordinary);
  await oldRead;
  for (let i=0; i<8; i++) await Promise.resolve();
  assert.strictEqual(typeof newResult, 'function', 'finished work is read after the older observation');
  newResult({...ordinary,narrative_content:'finished narrative'});
  await afterFinish;
  assert.strictEqual(window.__narrative, 'finished narrative');
  console.log('vitals render and issue-read contracts passed');
}
issueReadCases().catch(error=>{console.error(error);process.exitCode=1;});
