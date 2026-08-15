// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

class Control {
  constructor(action, markId) {
    this.dataset = { removalAction: action, markId: markId || '' };
    this.listeners = {};
  }

  addEventListener(name, listener) {
    this.listeners[name] = listener;
  }

  click() {
    this.listeners.click();
  }
}

class Element {
  constructor() {
    this.children = [];
    this.controls = [];
    this._innerHTML = '';
  }

  appendChild(child) {
    this.children.push(child);
    return child;
  }

  get innerHTML() {
    return this._innerHTML;
  }

  set innerHTML(value) {
    this._innerHTML = String(value);
    this.controls = Array.from(this._innerHTML.matchAll(
      /<button[^>]*data-removal-action="([^"]+)"(?:[^>]*data-mark-id="([^"]*)")?[^>]*>/g
    )).map((match) => new Control(match[1], match[2]));
  }

  querySelectorAll(selector) {
    return selector === '[data-removal-action]' ? this.controls : [];
  }

  setAttribute() {}
}

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

async function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) throw new Error('manifest directory required');
  const source = fs.readFileSync(path.join(manifestDir, 'assets/removals.js'), 'utf8');
  const root = new Element();
  const stream = '<img src=x onerror=stream>';
  const staged = 'chronicle/<img src=x onerror=staged>';
  const name = '<img src=x onerror=name>';
  const marked = {
    id: 'marked',
    state: 'marked',
    origin: 'policy',
    day: '20260101',
    stream,
    count: 1,
    bytes: 1,
    size: '1 B'
  };
  const failed = {
    id: 'failed',
    state: 'failed',
    day: '20260101',
    stream,
    staged
  };
  const list = { state: 'list.ready', removals: [marked, failed] };
  const window = {
    apiJson(url) {
      if (url === '/app/home/api/removals') return Promise.resolve(list);
      if (url === '/app/home/api/approve') {
        return Promise.resolve({
          state: 'approve.refused_after_start',
          removed_count: 0,
          not_removed_count: 1,
          refusals: [{ state: 'refusal.item_named', name, reason: 'kept' }]
        });
      }
      throw new Error(`unexpected URL: ${url}`);
    }
  };
  const document = {
    readyState: 'complete',
    addEventListener() {},
    createElement() { return new Element(); },
    querySelector(selector) { return selector === '[data-home-root]' ? root : null; }
  };
  window.window = window;
  vm.runInNewContext(source, { document, Promise, setImmediate, window }, { filename: 'removals.js' });
  await flush();
  await flush();

  const card = root.children[0];
  assert(card, 'card mounted');
  card.controls.find((control) => control.dataset.removalAction === 'approve').click();
  await flush();
  card.controls.find((control) => control.dataset.removalAction === 'confirm').click();
  await flush();
  await flush();

  const html = card.innerHTML;
  for (const value of [stream, staged, name]) {
    assert(!html.includes(value), `raw journal value rendered: ${value}`);
    assert(html.includes(value.replace(/</g, '&lt;').replace(/>/g, '&gt;')), `escaped journal value missing: ${value}`);
  }
  assert(!html.includes('<img'), 'journal markup must not become live DOM');
}

main().catch((error) => {
  process.stderr.write(`${error && error.stack ? error.stack : error}\n`);
  process.exitCode = 1;
});
