// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Calm owner curation stands in its own `ready to shape` card (never inflating
// the status fault count), while damaged voice profiles remain status issues.

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
  `  window.__shape = { renderVitalsHtml, renderReadyToShapeHtml, homeHealthGlance, readVitals,
    seedPulse: data => { lastPulse = data; pulseObservedAt = Date.now(); pulseReadFailed = false; },
    rendered: () => ({ vitals: renderedVitals, shape: renderedShape }) };
${EXPORT_ANCHOR}`,
);
source = source.replace(
  '    function renderVitals(pulse) {\n',
  '    let renderedVitals = null;\n    let renderedShape = null;\n    function renderVitals(pulse) {\n      renderedVitals = renderVitalsHtml(pulse);\n      renderedShape = renderReadyToShapeHtml(pulse);\n      return;\n',
);

let pulseResponse = null;
const listReads = [];
const location = { href: '', pathname: '/app/home/' };
const document = {
  readyState: 'loading',
  addEventListener() {},
  querySelector() { return null; },
  querySelectorAll() { return []; },
  getElementById() { return null; },
};
const window = { createViewReady: () => () => {},
  location,
  document,
  addEventListener() {},
  clearTimeout() {},
  setTimeout() { return 1; },
  apiJson(url) {
    if (url.startsWith('/app/home/api/removals')) listReads.push(url);
    if (url === '/app/home/api/pulse') return Promise.resolve(pulseResponse);
    return new Promise(() => {});
  },
};
window.window = window;
document.defaultView = window;

vm.runInNewContext(withViewRuntime(source), { window, document, console }, { filename: 'home.js' });
const ui = window.__shape;

const calm = { verdict: 'ok', severity: 'green', headline: 'ok', issues: [], last_observation: '4m' };
const voiceTeach = {
  source_id: 'owner_voice:not_established',
  text: 'voice setup',
  kind: 'route',
  payload: { href: '/app/speakers/#your-voice' },
  disabled: false,
  reason: '',
};
const voiceCandidate = {
  source_id: 'owner_voice:candidate_ready',
  text: 'voice candidate',
  kind: 'route',
  payload: { href: '/app/speakers/#your-voice' },
  disabled: false,
  reason: '',
};
const voiceDamaged = {
  source_id: 'owner_voice:damaged',
  text: 'voice profile unreadable',
  kind: 'route',
  payload: { href: '/app/speakers/#your-voice' },
  disabled: false,
  reason: '',
};
const faultRows = html => (html.match(/<li class="pulse-waiting-row/g) || []).length;
const shapeRows = html => (html.match(/<li class="pulse-shape-row/g) || []).length;

(async () => {
  // Nothing waiting: no fault list in status, no ready-to-shape card.
  const quietPulse = { health_glance: calm, needs_you_items: [] };
  const quietVitals = ui.renderVitalsHtml(quietPulse);
  assert.strictEqual(quietVitals.includes('pulse-waiting'), false, quietVitals);
  assert.strictEqual(ui.renderReadyToShapeHtml(quietPulse), '');

  // Calm needs-you items (voice teach/candidate, custom routes, disabled routes, notes)
  // render in ready-to-shape and never turn status into an attention fault.
  const calmItemsPulse = {
    health_glance: calm,
    needs_you_items: [
      voiceTeach,
      voiceCandidate,
      { text: 'unsafe', kind: 'route', disabled: true, reason: 'unavailable', payload: { href: '//elsewhere' } },
      { text: 'custom note', kind: 'note', disabled: false, reason: '' },
    ],
  };
  const calmVitals = ui.renderVitalsHtml(calmItemsPulse);
  assert.strictEqual(faultRows(calmVitals), 0, calmVitals);
  assert.strictEqual(ui.homeHealthGlance(calmItemsPulse).glance.verdict, 'ok');

  const shapeHtml = ui.renderReadyToShapeHtml(calmItemsPulse);
  assert(shapeHtml.includes('data-home-surface="ready-to-shape"'), shapeHtml);
  assert.strictEqual(shapeRows(shapeHtml), 4, shapeHtml);
  assert(shapeHtml.includes('href="/app/speakers/#your-voice"'), shapeHtml);
  assert.strictEqual(shapeHtml.includes('//elsewhere'), false, shapeHtml);
  assert(shapeHtml.includes('pulse-waiting-reason'), shapeHtml);

  // Damaged voice profile stays in status faults and never enters ready-to-shape.
  const damagedPulse = {
    health_glance: calm,
    needs_you_items: [voiceDamaged],
  };
  const damagedGlance = ui.homeHealthGlance(damagedPulse).glance;
  assert.strictEqual(damagedGlance.verdict, 'attention');
  assert.strictEqual(damagedGlance.issues.length, 1);
  const damagedVitals = ui.renderVitalsHtml(damagedPulse);
  assert.strictEqual(faultRows(damagedVitals), 1, damagedVitals);
  assert(damagedVitals.includes('href="/app/speakers/#your-voice"'), damagedVitals);
  assert.strictEqual(ui.renderReadyToShapeHtml(damagedPulse), '');

  // Suggestions (from solShellBadges.curation), possible duplicates, originals, and voice
  // stand in order inside ready-to-shape without touching status faults.
  window.solShellBadges = { curation: 5 };
  const pulseWith = (total, duplicates = 0, items = []) => ({
    health_glance: calm,
    needs_you_items: items,
    possible_duplicates_count: duplicates,
    narrative_content: null,
    originals_total: total,
  });
  ui.seedPulse(pulseWith(undefined));
  pulseResponse = pulseWith(47, 2, [voiceTeach]);
  await ui.readVitals();
  const fullShape = ui.rendered().shape;
  assert.strictEqual(shapeRows(fullShape), 4, fullShape);
  const order = [
    '/app/curation/',
    '/app/entities/?folder=duplicates',
    '/app/home/originals/',
    '/app/speakers/#your-voice',
  ].map(href => fullShape.indexOf('href="' + href + '"'));
  assert(order.every(index => index > 0), fullShape);
  assert.deepStrictEqual(order, [...order].sort((a, b) => a - b), fullShape);
  assert(fullShape.includes('5'), fullShape);
  assert(fullShape.includes('2'), fullShape);
  assert(fullShape.includes('47'), fullShape);
  assert.strictEqual(faultRows(ui.rendered().vitals), 0, ui.rendered().vitals);

  // Zero counts hide ready-to-shape completely.
  window.solShellBadges = { curation: 0 };
  pulseResponse = pulseWith(0, 0, []);
  await ui.readVitals();
  assert.strictEqual(ui.rendered().shape, '');

  // An unreadable originals total (null) still renders a row in ready-to-shape linking to originals,
  // and Home never fetches /app/home/api/removals directly.
  pulseResponse = pulseWith(null, 0, []);
  await ui.readVitals();
  assert.strictEqual(shapeRows(ui.rendered().shape), 1, ui.rendered().shape);
  assert(ui.rendered().shape.includes('href="/app/home/originals/"'), ui.rendered().shape);
  assert.deepStrictEqual(listReads, [], 'home reads originals count from pulse, never the list');

  console.log('ready-to-shape render contract passed');
})().catch(error => {
  console.error(error);
  process.exit(1);
});
