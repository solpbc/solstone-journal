// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// What's waiting on the owner: one list under the status line, only when
// something is there. Needs-you items, health issues and the originals row are
// its rows.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const manifestDir = process.argv[2];
if (!manifestDir) throw new Error('manifest directory required');

let source = fs.readFileSync(path.join(manifestDir, 'assets/home.js'), 'utf8');
source = source.replace(
  '  window.selectAccountSide = selectAccountSide;\n',
  `  window.__waiting = { renderVitalsHtml, readOriginalsTotal, readVitals,
    seedPulse: data => { lastPulse = data; pulseObservedAt = Date.now(); pulseReadFailed = false; },
    rendered: () => renderedVitals,
    requests: () => originalsReads };
  window.selectAccountSide = selectAccountSide;\n`,
);
// Count originals reads and keep the last status block the page rendered,
// without a DOM.
source = source.replace(
  '  function readOriginalsTotal() {\n',
  '  let originalsReads = 0;\n  function readOriginalsTotal() {\n    originalsReads += 1;\n',
);
source = source.replace(
  '  function renderVitals(pulse) {\n',
  '  let renderedVitals = null;\n  function renderVitals(pulse) {\n    renderedVitals = renderVitalsHtml(pulse);\n    return;\n',
);

let resolveOriginals = null;
let rejectOriginals = null;
let pulseResponse = null;
const location = { href: '', pathname: '/app/home/' };
const document = {
  readyState: 'loading',
  addEventListener() {},
  querySelector() { return null; },
  querySelectorAll() { return []; },
  getElementById() { return null; },
};
const window = {
  location,
  document,
  addEventListener() {},
  clearTimeout() {},
  setTimeout() { return 1; },
  apiJson(url) {
    if (url.startsWith('/app/home/api/removals')) {
      return new Promise((resolve, reject) => { resolveOriginals = resolve; rejectOriginals = reject; });
    }
    if (url === '/app/home/api/pulse') return Promise.resolve(pulseResponse);
    return new Promise(() => {});
  },
};
window.window = window;
document.defaultView = window;

vm.runInNewContext(source, { window, document, console }, { filename: 'home.js' });
const ui = window.__waiting;

const calm = { verdict: 'ok', severity: 'green', headline: "everything's working", issues: [], last_observation: '4 minutes ago' };
const voice = {
  text: 'set up your voice',
  kind: 'route',
  payload: { href: '/app/speakers/#your-voice' },
  disabled: false,
  reason: '',
};
const rows = html => (html.match(/<li class="pulse-waiting-row/g) || []).length;

(async () => {
  // Nothing waiting: no heading, no list, the line as it was.
  const quiet = ui.renderVitalsHtml({ health_glance: calm, needs_you_items: [] });
  assert.strictEqual(quiet.includes('pulse-waiting'), false, quiet);
  assert(quiet.includes('last reached your journal'), quiet);

  // A needs-you route is a link to where it goes; a disabled item shows its
  // reason and is not a link; a note is text.
  const needs = ui.renderVitalsHtml({
    health_glance: calm,
    needs_you_items: [
      voice,
      { text: 'unsafe', kind: 'route', disabled: true, reason: "this link isn't available from here.", payload: { href: '//elsewhere' } },
      { text: 'the invoice', kind: 'note', disabled: false, reason: '' },
    ],
  });
  assert.strictEqual(rows(needs), 3, needs);
  assert(needs.includes('<a href="/app/speakers/#your-voice">'), needs);
  assert.strictEqual(needs.includes('//elsewhere'), false, needs);
  assert(needs.includes('pulse-waiting-reason'), needs);
  // A calm verdict keeps its own headline; the list gets its own heading.
  assert(needs.includes('pulse-waiting-heading'), needs);
  assert(needs.includes('aria-labelledby="pulse-waiting-heading"'), needs);
  assert(needs.includes("everything&#39;s working"), needs);

  // Rows stand in order: health, needs-you, originals.
  ui.seedPulse({ health_glance: calm, needs_you_items: [], narrative_content: null });
  const read = ui.readOriginalsTotal();
  // The total resolving before any paint still reaches the first paint.
  resolveOriginals({ state: 'list.ready', total_rows: 47, offset: 0, next_offset: 20, removals: new Array(20).fill({}) });
  await read;
  const ordered = ui.renderVitalsHtml({
    health_glance: {
      verdict: 'attention', severity: 'amber', headline: '1 thing needs your attention',
      issues: [{ text: 'a health issue', severity: 'amber', href: '/app/health' }],
    },
    needs_you_items: [voice],
  });
  const order = ['/app/health', '/app/speakers/#your-voice', '/app/home/originals/'].map(href => ordered.indexOf('href="' + href + '"'));
  assert(order.every(index => index > 0), ordered);
  assert.deepStrictEqual(order, [...order].sort((a, b) => a - b), ordered);
  // The count is the list's whole item total, not the page's rows.
  assert(ordered.includes('47'), ordered);
  assert.strictEqual(ordered.includes('>20'), false, ordered);

  // A total that arrives after the paint adds the heading and the row; a
  // later vitals refresh keeps them without reading the list again.
  ui.seedPulse({ health_glance: calm, needs_you_items: [], narrative_content: null });
  const before = ui.requests();
  const late = ui.readOriginalsTotal();
  resolveOriginals({ state: 'list.ready', total_rows: 3, offset: 0, next_offset: null, removals: [{}, {}, {}] });
  await late;
  assert(ui.rendered().includes('pulse-waiting-heading'), ui.rendered());
  assert(ui.rendered().includes('href="/app/home/originals/"'), ui.rendered());
  pulseResponse = { health_glance: calm, needs_you_items: [], narrative_content: null };
  await ui.readVitals();
  assert(ui.rendered().includes('href="/app/home/originals/"'), ui.rendered());
  assert.strictEqual(ui.requests(), before + 1, 'a refresh does not re-read the list');

  // An item too large to review still counts.
  const large = ui.readOriginalsTotal();
  resolveOriginals({ state: 'list.review_too_large', total_rows: 1, offset: 0, next_offset: null, removals: [{}], review_context: null });
  await large;
  assert.strictEqual(rows(ui.renderVitalsHtml({ health_glance: calm, needs_you_items: [] })), 1);

  // Nothing marked: no row, no list.
  const empty = ui.readOriginalsTotal();
  resolveOriginals({ state: 'list.empty', total_rows: 0, offset: 0, next_offset: null, removals: [] });
  await empty;
  assert.strictEqual(ui.renderVitalsHtml({ health_glance: calm, needs_you_items: [] }).includes('pulse-waiting'), false);

  // A list that can't be read is still a row, linking to the originals page.
  const unavailable = ui.readOriginalsTotal();
  resolveOriginals({ state: 'list.register_unavailable' });
  await unavailable;
  const failedRow = ui.renderVitalsHtml({ health_glance: calm, needs_you_items: [] });
  assert.strictEqual(rows(failedRow), 1, failedRow);
  assert(failedRow.includes('href="/app/home/originals/"'), failedRow);
  const rejected = ui.readOriginalsTotal();
  rejectOriginals(new Error('offline'));
  await rejected;
  assert.strictEqual(rows(ui.renderVitalsHtml({ health_glance: calm, needs_you_items: [] })), 1);

  console.log('waiting-list render contract passed');
})().catch(error => {
  console.error(error);
  process.exit(1);
});
