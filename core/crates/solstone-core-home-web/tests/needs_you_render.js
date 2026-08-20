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
  `  window.__needsYou = { needsYouItemHtml, dispatchNeedsYouItem };
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
const { needsYouItemHtml, dispatchNeedsYouItem } = window.__needsYou;
assert(needsYouItemHtml, 'needsYouItemHtml exported');
assert(dispatchNeedsYouItem, 'dispatchNeedsYouItem exported');

const noteHtml = needsYouItemHtml({
  text: 'the invoice',
  kind: 'note',
  disabled: false,
  reason: '',
});
assert.strictEqual(noteHtml, '<div class="pulse-needs-item">the invoice</div>');
assert.strictEqual(noteHtml.includes('role='), false);
assert.strictEqual(noteHtml.includes('tabindex'), false);
assert.strictEqual(noteHtml.includes('data-needs-you-item'), false);

const disabledHtml = needsYouItemHtml({
  text: 'unsafe',
  kind: 'route',
  disabled: true,
  reason: 'this link isn\'t available from here.',
});
assert.strictEqual(disabledHtml.includes('pulse-needs-item-disabled'), true);
assert.strictEqual(disabledHtml.includes('role="button"'), false);
assert.strictEqual(disabledHtml.includes('data-needs-you-item'), false);

const routeHtml = needsYouItemHtml({
  text: 'open health',
  kind: 'route',
  disabled: false,
  reason: '',
  payload: { href: '/app/health' },
});
assert.strictEqual(routeHtml.includes('role="button"'), true);
assert.strictEqual(routeHtml.includes('tabindex="0"'), true);
assert.strictEqual(routeHtml.includes('data-needs-you-item'), true);

location.href = '';
dispatchNeedsYouItem({
  text: 'open health',
  kind: 'route',
  disabled: false,
  payload: { href: '/app/health' },
});
assert.strictEqual(location.href, '/app/health');

location.href = 'keep';
dispatchNeedsYouItem({
  text: 'the invoice',
  kind: 'note',
  disabled: false,
});
assert.strictEqual(location.href, 'keep');

console.log('needs-you render contract passed');
