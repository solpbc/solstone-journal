// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Home is on the view contract: its script registers, and the page mounts it.
// A harness runs the page's own read store and view contract beside home's
// source, then mounts home the way the page does.

const fs = require('fs');
const path = require('path');

const shellStatic = path.join(__dirname, '../../solstone-core-convey-shell/assets/static');
const runtime = ['store.js', 'views.js']
  .map((name) => fs.readFileSync(path.join(shellStatic, name), 'utf8'))
  .join('\n');

// Where home's code hands its functions to a harness.
const EXPORT_ANCHOR = "    ctx.global('selectAccountSide', selectAccountSide);\n";

function withViewRuntime(source, view = 'home') {
  return `${runtime}\n${source}\nwindow.SolViews.mount(${JSON.stringify(view)});\n`;
}

module.exports = { withViewRuntime, EXPORT_ANCHOR, runtime };
