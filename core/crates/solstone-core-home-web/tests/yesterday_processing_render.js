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
  `  window.__yesterday = { renderYesterdayProcessingHtml };
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
const { renderYesterdayProcessingHtml } = window.__yesterday;
assert(renderYesterdayProcessingHtml, 'renderYesterdayProcessingHtml exported');

// Failures and neutral summaries mixed in one degraded day split into two
// labeled groups, with the failure count surfaced instead of left implicit.
const degraded = renderYesterdayProcessingHtml({
  yesterday_processing: {
    mode: 'degraded',
    gap_links: [
      { text: "The conversation run didn't finish.", href: '/app/thinking/#runs/x' },
      { text: '2 document runs didn\'t finish.', href: '/app/thinking/#runs/y' },
    ],
    details: [
      "I didn't produce any facet newsletters.",
      'your busiest stretches were 9-10am · 2-3pm · 5-6pm.',
    ],
    failed_run_count: 19,
  },
});
assert(degraded.includes('what didn&#39;t finish · 19 runs') || degraded.includes('what didn\'t finish · 19 runs'), degraded);
assert(degraded.includes('everything else'), degraded);
const gapListStart = degraded.indexOf("conversation run");
const detailsListStart = degraded.indexOf('facet newsletters');
assert(gapListStart > -1 && detailsListStart > -1 && gapListStart < detailsListStart, degraded);

// Singular count reads "1 run", not "1 runs".
const singular = renderYesterdayProcessingHtml({
  yesterday_processing: {
    mode: 'degraded',
    gap_links: [{ text: "The conversation run didn't finish.", href: '/app/thinking/#runs/x' }],
    details: ["I didn't produce any facet newsletters."],
    failed_run_count: 1,
  },
});
assert(singular.includes('1 run<') || singular.includes('1 run '), singular);
assert(!singular.includes('1 runs'), singular);

// Only one kind of line present (the common healthy-day case) — no split,
// no label noise.
const healthy = renderYesterdayProcessingHtml({
  yesterday_processing: {
    mode: 'healthy',
    gap_links: [],
    details: ["I wrote 2 newsletters and prepared your morning briefing."],
    failed_run_count: 0,
  },
});
assert.strictEqual(healthy.includes('pulse-yesterday-shelf-label'), false, healthy);
assert(healthy.includes('I wrote 2 newsletters'), healthy);

console.log('yesterday-processing render contract passed');
