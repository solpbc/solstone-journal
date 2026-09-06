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
  '  window.toggleBriefingCard = toggleBriefingCard;\n',
  `  window.__home = { renderNarrativeHtml, briefingPlaceholderHtml };
  window.toggleBriefingCard = toggleBriefingCard;\n`,
);

const location = { href: '' };
const document = {
  readyState: 'loading',
  addEventListener() {},
  querySelector() { return null; },
};
const window = {
  location,
  document,
  addEventListener() {},
};
window.window = window;
document.defaultView = window;

vm.runInNewContext(source, { window, document, console }, { filename: 'home.js' });
const { renderNarrativeHtml, briefingPlaceholderHtml } = window.__home;
assert(renderNarrativeHtml, 'renderNarrativeHtml exported');
assert(briefingPlaceholderHtml, 'briefingPlaceholderHtml exported');

// A day with no audio in it yet is not a fault, so it offers no fault recovery:
// the health page is green and sending the owner there teaches them to ignore
// recovery links.
const emptyDay = renderNarrativeHtml({ narrative_content: null, segment_count: 0 });
assert(emptyDay.includes('no narrative yet'), emptyDay);
assert.strictEqual(emptyDay.includes('/app/health'), false, emptyDay);
assert.strictEqual(emptyDay.includes('check system health'), false, emptyDay);

// A day that has audio but no analysis yet is a third state again, and still
// offers no recovery.
const awaitingAnalysis = renderNarrativeHtml({ narrative_content: null, segment_count: 4 });
assert(awaitingAnalysis.includes('analysis will be available after the next processing cycle.'), awaitingAnalysis);
assert.strictEqual(awaitingAnalysis.includes('/app/health'), false, awaitingAnalysis);

// The placeholder belongs to the morning phase only. Outside it there is no
// briefing being prepared to talk about.
assert.strictEqual(briefingPlaceholderHtml({ phase: 'eod', exists: false }, {}), '');
assert.strictEqual(briefingPlaceholderHtml({ phase: 'active', exists: false }, {}), '');
assert.strictEqual(briefingPlaceholderHtml({ phase: 'pending', exists: true }, {}), '');

const pending = briefingPlaceholderHtml({ phase: 'pending', exists: false }, { briefing_lateness: { late: false } });
assert(pending.includes('your morning briefing is being prepared'), pending);

const late = briefingPlaceholderHtml(
  { phase: 'pending', exists: false },
  { briefing_lateness: { late: true, late_hours: 3 }, today: '20260905' },
);
assert(late.includes("your briefing is usually ready by 10 am; it's 3h late."), late);
assert(late.includes('/app/thinking/#runs/20260905/morning_briefing'), late);
assert.strictEqual(/\bI\b|I'm|\bmy\b/.test(late), false, late);

// A briefing that was never prepared says so instead of disappearing. X-04.
const missing = briefingPlaceholderHtml(
  { phase: 'missing', exists: false },
  { briefing_lateness: { late: true, late_hours: 1 }, today: '20260906' },
);
assert(missing.includes("your morning briefing wasn't prepared."), missing);
assert(missing.includes('/app/thinking/#runs/20260906/morning_briefing'), missing);
assert(missing.includes('see the run'), missing);
assert.strictEqual(/\bI\b|I'm|\bmy\b/.test(missing), false, missing);
assert.strictEqual(briefingPlaceholderHtml({ phase: 'missing', exists: true }, {}), '');

// The narrative card is named for what it holds, not for the run that wrote it.
// X-03.
const live = renderNarrativeHtml({
  narrative_content: 'the morning remains steady.',
  narrative_header: 'pulse',
  segment_count: 4,
  today: '20260906',
});
assert(live.includes('today so far'), live);
assert.strictEqual(live.toLowerCase().includes('>pulse<'), false, live);

console.log('narrative and briefing render contract passed');
