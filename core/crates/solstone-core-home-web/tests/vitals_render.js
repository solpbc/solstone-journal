// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { withViewRuntime, EXPORT_ANCHOR } = require('./view_runtime');

const manifestDir = process.argv[2];
if (!manifestDir) throw new Error('manifest directory required');

let source = fs.readFileSync(path.join(manifestDir, 'assets/home.js'), 'utf8');
source = source.replace(
  EXPORT_ANCHOR,
  `  window.__vitals = { renderVitalsHtml, homeHealthGlance, receiveSupervisorStatus, invalidateHeldWork, refreshVitals, refreshNarrative, refreshChangedPulse,
    renderSecurity, securityLineHtml, syncHomeLiveBadge,
    captureNarrative: () => { renderNarrative = data => { window.__narrative = data.narrative_content; }; },
    seedPulse: data => { lastPulse=data; pulseObservedAt=Date.now(); pulseReadFailed=false; pulseRequest=null; vitalsRequest=null; narrativeRequest=null; } };
${EXPORT_ANCHOR}`,
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
const window = { createViewReady: () => () => {},
  location,
  document,
  addEventListener() {},
  apiJson() { return new Promise(() => {}); },
  clearTimeout() {},
  setTimeout(fn) { expiry=fn; return 1; },
};
window.window = window;
document.defaultView = window;

vm.runInNewContext(withViewRuntime(source), { window, document, console, Date:TestDate }, { filename: 'home.js' });
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

  // Status mark header, R7 integrity slot seam, per-device strip, crashed services,
  // pending import reviews, security states, and zero-double-count live badge delta.
  receiveSupervisorStatus({ event: 'status', queues: {}, held: [], crashed: [] });
  window.solShellStatus = { statusMarksBase: '/static/sol-status/', stem: 'mark-active' };
  const activeHeaderHtml = renderVitalsHtml(ordinary);
  assert(activeHeaderHtml.includes('data-home-status-mark'), activeHeaderHtml);
  assert(activeHeaderHtml.includes('src="/static/sol-status/mark-active.svg"'), activeHeaderHtml);
  assert(activeHeaderHtml.includes('href="/app/health/"'), activeHeaderHtml);
  assert(activeHeaderHtml.includes('data-home-integrity-slot'), activeHeaderHtml);

  // Day-0 empty registry device strip vs paired device strip
  const emptyRegistryHtml = renderVitalsHtml({
    ...ordinary,
    segment_count: 0,
    capture_health: { status: 'no_clients', registry: 'registry_empty', clients: [], unassessed: [] },
  });
  assert(emptyRegistryHtml.includes('data-home-device-strip="empty"'), emptyRegistryHtml);
  assert(emptyRegistryHtml.includes('href="/app/network/#devices"'), emptyRegistryHtml);
  assert(emptyRegistryHtml.includes('href="/app/sources/"'), emptyRegistryHtml);
  assert(!emptyRegistryHtml.includes('href="/app/transcripts/"'), emptyRegistryHtml);

  const pairedHtml = renderVitalsHtml({
    ...ordinary,
    segment_count: 3,
    capture_health: {
      status: 'degraded',
      registry: 'ok',
      clients: [
        { name: 'MacBook', status: 'active', failing: false, capture_elapsed_ms: 120000 },
        { name: 'iPhone', status: 'stale', failing: false, capture_elapsed_ms: 7200000 },
        { name: 'iPad', status: 'offline', failing: false, capture_elapsed_ms: null },
        { name: 'Watch', status: 'degraded', failing: true, capture_elapsed_ms: 60000 },
      ],
      unassessed: [{ name: 'Desk', reason: 'awaiting_first_delivery' }],
    },
  });
  assert(pairedHtml.includes('data-home-device-strip="paired"'), pairedHtml);
  assert(pairedHtml.includes('data-device-chip="active"'), pairedHtml);
  assert(pairedHtml.includes('data-device-chip="quiet"'), pairedHtml);
  assert.strictEqual((pairedHtml.match(/data-device-chip="awaiting"/g) || []).length, 2, pairedHtml);
  assert(pairedHtml.includes('data-device-chip="failing"'), pairedHtml);
  assert(pairedHtml.includes('href="/app/health/#registeredClientsCard"'), pairedHtml);
  assert(pairedHtml.includes('href="/app/sources/"'), pairedHtml);
  assert(pairedHtml.includes('href="/app/transcripts/"'), pairedHtml);

  // Per-source needs_attention chip when top-level failing === false, and Day-0 declared_quiet chip without "awaiting first upload"
  const perSourceAndDeclaredQuietHtml = renderVitalsHtml({
    ...ordinary,
    segment_count: 1,
    capture_health: {
      status: 'active',
      registry: 'registry_complete',
      clients: [
        {
          name: 'Studio Mac',
          status: 'active',
          failing: false,
          capture_elapsed_ms: 5000,
          source_delivery: {
            audio: { state: 'current', elapsed_ms: 5000, ingest_rejection: null },
            obsidian: {
              state: 'needs_attention',
              elapsed_ms: null,
              ingest_rejection: { reason_code: 'obsidian_invalid', active_count: 2 },
            },
          },
        },
      ],
      unassessed: [{ name: 'Pocket Phone', reason: 'declared_quiet' }],
    },
  });
  assert(perSourceAndDeclaredQuietHtml.includes('data-device-chip="failing"'), perSourceAndDeclaredQuietHtml);
  assert(perSourceAndDeclaredQuietHtml.includes('href="/app/health/#registeredClientsCard"'), perSourceAndDeclaredQuietHtml);
  assert(perSourceAndDeclaredQuietHtml.includes('data-device-chip="quiet"'), perSourceAndDeclaredQuietHtml);
  assert(!perSourceAndDeclaredQuietHtml.includes('data-device-chip="awaiting"'), perSourceAndDeclaredQuietHtml);
  assert(!perSourceAndDeclaredQuietHtml.includes('awaiting first upload'), perSourceAndDeclaredQuietHtml);

  // Crashed / crash-looping services in supervisor status
  receiveSupervisorStatus({
    event: 'status',
    queues: {},
    held: [],
    crashed: [{ name: 'think', phase: 'backoff' }],
  });
  const backoffGlance = homeHealthGlance(ordinary).glance;
  assert.strictEqual(backoffGlance.verdict, 'attention');
  assert.strictEqual(backoffGlance.severity, 'amber');
  assert.strictEqual(backoffGlance.issues.length, 1);

  receiveSupervisorStatus({
    event: 'status',
    queues: {},
    held: [],
    crashed: [{ name: 'parakeet', phase: 'crashed' }],
  });
  const stoppedGlance = homeHealthGlance(ordinary).glance;
  assert.strictEqual(stoppedGlance.verdict, 'attention');
  assert.strictEqual(stoppedGlance.severity, 'red');
  assert.strictEqual(stoppedGlance.issues.length, 1);
  receiveSupervisorStatus({ event: 'status', queues: {}, held: [], crashed: [] });

  // Pending import reviews render as a non-fault row and never inflate g.issues
  window.solShellBadges = {
    import: 3,
    import_reviews: [
      { id: 'imp-42', source: 'Apple Health', swept_after_hour: true },
    ],
  };
  const importHtml = renderVitalsHtml(ordinary);
  assert(importHtml.includes('data-home-import-review="imp-42"'), importHtml);
  assert(importHtml.includes('href="/app/import/imp-42"'), importHtml);
  assert(importHtml.includes('data-import-review-more'), importHtml);
  assert(!importHtml.includes('pulse-waiting-row'), importHtml);
  assert.strictEqual(homeHealthGlance(ordinary).glance.verdict, 'ok');
  window.solShellBadges = null;

  // Security states: 'checking' is neutral; 'unconfirmed' adds an uncertain note; restart flag renders restart control
  window.__vitals.renderSecurity({ security: { state: 'checking', flags: [] } });
  assert.strictEqual(homeHealthGlance(ordinary).glance.verdict, 'ok');
  window.__vitals.renderSecurity({ security: { state: 'unconfirmed', flags: [] } });
  const unconfirmedRes = homeHealthGlance(ordinary);
  assert.strictEqual(unconfirmedRes.glance.verdict, 'unavailable');
  assert(unconfirmedRes.notes.some(n => n.href === '/app/health/#healthSystemDetails'));
  window.__vitals.renderSecurity({ security: { state: 'ok', flags: [] } });
  assert.strictEqual(homeHealthGlance(ordinary).glance.verdict, 'ok');

  const restartHtml = window.__vitals.securityLineHtml({ subject: 'journal', restart: true, notes: 'https://solstone.app/sec' });
  assert(restartHtml.includes('data-security-restart-btn'), restartHtml);
  assert(restartHtml.includes('href="https://solstone.app/sec"'), restartHtml);

  // Live badge delta contributes ONLY Home-live issues (crashed, held, nightRows, damaged voice),
  // never double-counting server-side health_glance.issues or backup.
  let lastDelta = undefined;
  window.solShellChrome = { setHomeBadgeDelta(d) { lastDelta = d; } };
  receiveSupervisorStatus({
    event: 'status',
    queues: {},
    held: [],
    crashed: [{ name: 'think', phase: 'backoff' }],
  });
  window.__vitals.syncHomeLiveBadge({
    ...diagnosed,
    needs_you_items: [
      { source_id: 'owner_voice:damaged', text: 'damaged', kind: 'route', payload: { href: '/app/speakers/' } },
      { source_id: 'owner_voice:not_established', text: 'teach', kind: 'route', payload: { href: '/app/speakers/' } },
    ],
  });
  assert.deepStrictEqual({ ...lastDelta }, { issues: 2, severity: 'amber', uncertain: false });
  receiveSupervisorStatus({ event: 'status', queues: {}, held: [], crashed: [] });

  // Integrity slot rendering: calm not_checked_yet, warn incomplete/unavailable, hidden when quiet.
  window.solShellStatus = { statusMarksBase: '/static/sol-status/', stem: 'mark' };
  const notCheckedHtml = renderVitalsHtml({
    ...ordinary,
    health_glance: { verdict: 'calm', severity: 'neutral', headline: 'adding', issues: [] },
    integrity: { state: 'not_checked_yet', tone: 'calm', text: 'no integrity check has run yet.', href: '/app/health/#integrity' },
  });
  assert(notCheckedHtml.includes('data-home-integrity-slot data-integrity-state="not_checked_yet"'), notCheckedHtml);
  assert(notCheckedHtml.includes('pulse-integrity-slot--calm'), notCheckedHtml);
  assert(notCheckedHtml.includes('href="/app/health/#integrity"'), notCheckedHtml);
  assert(notCheckedHtml.includes('src="/static/sol-status/mark.svg"'), 'calm integrity leaves shell mark intact');

  // Incomplete/unavailable blocks green and renders amber slot, without inventing a nonexistent mark SVG.
  const incompleteHtml = renderVitalsHtml({
    ...ordinary,
    health_glance: { verdict: 'unavailable', severity: 'amber', headline: 'unclear', issues: [] },
    integrity: { state: 'incomplete', tone: 'warn', text: 'integrity check stopped at 4 of 10 days.', href: '/app/health/#integrity' },
  });
  assert(incompleteHtml.includes('pulse-integrity-slot--warn'), incompleteHtml);
  assert(incompleteHtml.includes('data-integrity-state="incomplete"'), incompleteHtml);
  assert(incompleteHtml.includes('src="/static/sol-status/mark.svg"'), 'incomplete with 0 urgent faults preserves shipped stem');

  // Urgent attention faults escalate calmer shell stems (mark, mark-paused, mark-connecting) to mark-attention.svg,
  // while preserving mark-offline / mark-error transport outage stems.
  const urgentPulse = {
    ...ordinary,
    health_glance: {
      verdict: 'attention',
      severity: 'red',
      headline: '1 thing needs your attention',
      issues: [{ text: '1 original file is missing.', severity: 'red', href: '/app/health/#integrity' }],
    },
    integrity: { state: 'needs_attention', tone: 'none', text: null, href: null },
  };
  for (const calmStem of ['mark', 'mark-paused', 'mark-connecting', 'mark-connecting-animated']) {
    window.solShellStatus = { statusMarksBase: '/static/sol-status/', stem: calmStem };
    const html = renderVitalsHtml(urgentPulse);
    assert(html.includes('src="/static/sol-status/mark-attention.svg"'), `escalates ${calmStem} to mark-attention`);
  }
  for (const severeStem of ['mark-offline', 'mark-error']) {
    window.solShellStatus = { statusMarksBase: '/static/sol-status/', stem: severeStem };
    const html = renderVitalsHtml(urgentPulse);
    assert(html.includes(`src="/static/sol-status/${severeStem}.svg"`), `preserves ${severeStem}`);
  }

  console.log('vitals render and issue-read contracts passed');
}
issueReadCases().catch(error=>{console.error(error);process.exitCode=1;});
