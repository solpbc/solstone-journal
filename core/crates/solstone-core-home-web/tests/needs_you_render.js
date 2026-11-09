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
  `  window.__waiting = { renderVitalsHtml, readVitals,
    seedPulse: data => { lastPulse = data; pulseObservedAt = Date.now(); pulseReadFailed = false; },
    rendered: () => renderedVitals };
  window.selectAccountSide = selectAccountSide;\n`,
);
// Keep the last status block the page rendered, without a DOM.
source = source.replace(
  '  function renderVitals(pulse) {\n',
  '  let renderedVitals = null;\n  function renderVitals(pulse) {\n    renderedVitals = renderVitalsHtml(pulse);\n    return;\n',
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

  // Pulse carries the originals list's item total; home never reads the list.
  const pulseWith = total => ({ health_glance: calm, needs_you_items: [], narrative_content: null, originals_total: total });
  ui.seedPulse(pulseWith(undefined));
  pulseResponse = pulseWith(47);
  await ui.readVitals();
  // Rows stand in order: health, needs-you, originals.
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
  // The count is the list's whole item total.
  assert(ordered.includes('47'), ordered);

  // A refresh carrying the total keeps the heading and the row.
  pulseResponse = pulseWith(3);
  await ui.readVitals();
  assert(ui.rendered().includes('pulse-waiting-heading'), ui.rendered());
  assert(ui.rendered().includes('href="/app/home/originals/"'), ui.rendered());

  // Nothing marked: no row, no list.
  pulseResponse = pulseWith(0);
  await ui.readVitals();
  assert.strictEqual(ui.renderVitalsHtml({ health_glance: calm, needs_you_items: [] }).includes('pulse-waiting'), false);

  // A list that can't be read is still a row, linking to the originals page.
  pulseResponse = pulseWith(null);
  await ui.readVitals();
  const failedRow = ui.renderVitalsHtml({ health_glance: calm, needs_you_items: [] });
  assert.strictEqual(rows(failedRow), 1, failedRow);
  assert(failedRow.includes('href="/app/home/originals/"'), failedRow);
  assert.deepStrictEqual(listReads, [], 'home reads the count from pulse, never the list');

  console.log('waiting-list render contract passed');
})().catch(error => {
  console.error(error);
  process.exit(1);
});
