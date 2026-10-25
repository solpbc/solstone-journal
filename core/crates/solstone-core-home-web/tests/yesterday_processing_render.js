// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// A night that went wrong stays visible as at most two rows of what's waiting:
// the missing account, unless the yesterday side already says so, and one row
// for everything else that didn't finish.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const manifestDir = process.argv[2];
if (!manifestDir) throw new Error('manifest directory required');

let source = fs.readFileSync(path.join(manifestDir, 'assets/home.js'), 'utf8');
source = source.replace(
  '  window.selectAccountSide = selectAccountSide;\n',
  `  window.__night = { nightRows,
    paint: (side, briefing) => { paintedSide = side; briefingData = briefing; } };
  window.selectAccountSide = selectAccountSide;\n`,
);

const document = {
  readyState: 'loading',
  addEventListener() {},
  querySelector() { return null; },
};
const window = { location: { href: '' }, document, addEventListener() {} };
window.window = window;
document.defaultView = window;

vm.runInNewContext(source, { window, document, console }, { filename: 'home.js' });
const { nightRows, paint } = window.__night;

const runs = '/app/thinking/#runs/20261023';
const account = { text: "yesterday wasn't written up.", href: runs + '/morning_briefing' };
const conversation = { text: "the conversation run didn't finish.", href: runs + '/conversation/u1' };
const documents = { text: "2 document runs didn't finish.", href: runs + '/documents' };
const night = (mode, links, reasons = []) => ({
  briefing_analysis_day: '20261023',
  yesterday_processing: {
    mode,
    summary_line: reasons.includes('pipeline_unavailable')
      ? 'yesterday’s processing status is unavailable.'
      : "some overnight processing didn't finish.",
    gap_links: links,
    status_reasons: reasons,
  },
});
// Rows come from the page's own realm; bring them into this one to compare.
const hrefs = rows => Array.from(rows, row => row.href);
const missing = { exists: false, phase: 'missing' };

// One thing didn't finish: that link, as it is.
paint('today', missing);
assert.deepStrictEqual(hrefs(nightRows(night('degraded', [conversation], ['pipeline_warning']))), [conversation.href]);

// Several: one row to the night's runs.
assert.deepStrictEqual(hrefs(nightRows(night('degraded', [conversation, documents], ['pipeline_warning']))), [runs]);

// Unknown status: the unavailable row, keyed on the status reason.
const unknown = nightRows(night('degraded', [], ['pipeline_unavailable']));
assert.deepStrictEqual(hrefs(unknown), [runs]);
assert.strictEqual(unknown[0].text, 'yesterday’s processing status is unavailable.');

// Unknown status and a missing account, painted on today: two rows, the
// account's first.
assert.deepStrictEqual(
  hrefs(nightRows(night('degraded', [account], ['pipeline_unavailable', 'briefing_missing']))),
  [account.href, runs],
);

// The briefing's own failed run is the same event as the missing account:
// one row, never both.
const failedBriefing = { text: "the daily briefing run didn't finish.", href: runs + '/morning_briefing/u9' };
assert.deepStrictEqual(
  hrefs(nightRows(night('degraded', [failedBriefing, account], ['pipeline_warning', 'briefing_missing']))),
  [account.href],
);

// Painted on yesterday, the yesterday side already says the account is
// missing: no row for it, and nothing else went wrong.
paint('yesterday', missing);
assert.deepStrictEqual(hrefs(nightRows(night('degraded', [account], ['briefing_missing']))), []);
assert.deepStrictEqual(hrefs(nightRows(night('degraded', [failedBriefing, account], ['pipeline_warning', 'briefing_missing']))), []);

// The welcome state has no switch, so the account row shows.
paint(null, missing);
assert.deepStrictEqual(hrefs(nightRows(night('degraded', [account], ['briefing_missing']))), [account.href]);

// An account that has since arrived takes its row away.
paint('today', { exists: true, phase: 'active' });
assert.deepStrictEqual(hrefs(nightRows(night('degraded', [account], ['briefing_missing']))), []);

// Healthy and sparse nights add none, whatever a sparse night's reasons say.
paint('today', missing);
assert.deepStrictEqual(hrefs(nightRows(night('healthy', []))), []);
assert.deepStrictEqual(hrefs(nightRows(night('sparse', [], ['pipeline_unavailable', 'briefing_missing']))), []);
assert.deepStrictEqual(hrefs(nightRows({ briefing_analysis_day: '20261023' })), []);

console.log('failed-night rows contract passed');
