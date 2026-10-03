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
  `  window.__today = { renderTodayHtml };
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
  JournalFormat: { time: () => '12:00 PM' },
};
window.window = window;
document.defaultView = window;

vm.runInNewContext(source, { window, document, console }, { filename: 'home.js' });
const { renderTodayHtml } = window.__today;
assert(renderTodayHtml, 'renderTodayHtml exported');

// A concurrent pair: both rows render, each with its own source element
// carrying the server's label. A row without a label gets no source element.
const html = renderTodayHtml({
  activities: [
    { id: 'a', display_time: '2026-06-02T12:00:00+00:00', description: 'first half', source_label: 'SOURCE-A' },
    { id: 'b', display_time: '2026-06-02T12:01:00+00:00', description: 'second half', source_label: 'SOURCE-<b>' },
    { id: 'c', display_time: '2026-06-02T09:00:00+00:00', description: 'alone' },
    { id: 'd', display_time: '2026-06-02T08:00:00+00:00', description: 'blank label', source_label: '  ' },
  ],
});

const rows = html.split('<div class="pulse-activity">').slice(1);
assert.strictEqual(rows.length, 4, html);
assert(rows[0].includes('first half<span class="pulse-activity-source">SOURCE-A</span>'), rows[0]);
assert(rows[1].includes('<span class="pulse-activity-source">SOURCE-&lt;b&gt;</span>'), rows[1]);
assert(!rows[2].includes('pulse-activity-source'), rows[2]);
assert(!rows[3].includes('pulse-activity-source'), rows[3]);
