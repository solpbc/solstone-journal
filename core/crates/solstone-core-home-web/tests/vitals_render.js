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
  `  window.__vitals = { renderVitalsHtml };
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
const { renderVitalsHtml } = window.__vitals;
assert(renderVitalsHtml, 'renderVitalsHtml exported');

const css = fs.readFileSync(path.join(manifestDir, 'assets/workspace.html'), 'utf8');
assert(css.includes('.pulse-vitals-dot.neutral'), 'missing .pulse-vitals-dot.neutral');
assert(css.includes('.pulse-vitals-verdict.neutral'), 'missing .pulse-vitals-verdict.neutral');
const dotNeutral = css.match(/\.pulse-vitals-dot\.neutral\s*\{[^}]*background:\s*([^;]+);/);
const verdictNeutral = css.match(/\.pulse-vitals-verdict\.neutral\s*\{[^}]*color:\s*([^;]+);/);
assert(dotNeutral, 'neutral dot has no background');
assert(verdictNeutral, 'neutral verdict has no color');
for (const forbidden of ['#4ade80', '#fbbf24', '#dc2626', '#166534', '#b45309', '#b91c1c']) {
  assert.notStrictEqual(dotNeutral[1].trim(), forbidden, 'neutral dot reuses an alert color');
  assert.notStrictEqual(verdictNeutral[1].trim(), forbidden, 'neutral verdict reuses an alert color');
}

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
assert.strictEqual(calm.includes('pulse-vitals-chip'), false, calm);
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
assert.strictEqual(green.includes('pulse-vitals-chip'), false, green);

const amber = renderVitalsHtml({
  health_glance: {
    verdict: 'attention',
    severity: 'amber',
    headline: '1 thing needs your attention',
    issues: [{ text: 'the solstone app on one of your devices has not added anything to your journal recently.', severity: 'amber', href: '/app/health' }],
  },
});
assert(amber.includes('pulse-vitals-dot amber'), amber);
assert(amber.includes('pulse-vitals-verdict amber'), amber);
assert(amber.includes('pulse-vitals-chip amber'), amber);

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
assert(red.includes('pulse-vitals-chip red'), red);

console.log('vitals render contract passed');
