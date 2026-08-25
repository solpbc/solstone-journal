// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');

function staticSource(name) {
  return fs.readFileSync(path.join(crateDir, 'assets/static', name), 'utf8');
}

function functionSource(source, name) {
  const start = source.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `${name} is defined in the shipped source`);
  const open = source.indexOf('{', start);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === '{') depth += 1;
    if (source[index] === '}' && --depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`unterminated ${name}`);
}

function cookieDocument() {
  const listeners = {};
  const document = {
    readyState: 'loading',
    cookieWrites: [],
    addEventListener(type, listener) { (listeners[type] ||= []).push(listener); },
    dispatch(type) { (listeners[type] || []).forEach((listener) => listener()); },
    querySelector() { return null; },
    getElementById() { return null; },
  };
  Object.defineProperty(document, 'cookie', {
    get() { return ''; },
    set(value) { document.cookieWrites.push(String(value)); },
  });
  return document;
}

function websocketHarness() {
  const document = cookieDocument();
  let eventSource = null;
  class FakeEventSource {
    constructor(url) {
      this.url = url;
      eventSource = this;
    }
    close() {}
  }
  const window = {
    document,
    location: { href: '/app/entities/' },
    setTimeout,
    clearTimeout,
    AppServices: { notifications: { show() {} } },
  };
  window.window = window;
  const context = vm.createContext({
    window,
    document,
    EventSource: FakeEventSource,
    setTimeout,
    clearTimeout,
    console: { debug() {}, log() {}, warn() {}, error() {} },
    Date,
  });
  vm.runInContext(staticSource('websocket.js'), context, { filename: 'websocket.js' });
  document.dispatch('DOMContentLoaded');
  assert.ok(eventSource, 'websocket source connects after DOM ready');
  eventSource.onopen();
  return { document, eventSource, window };
}

function testNavigateMessages() {
  const pathAndFacet = websocketHarness();
  pathAndFacet.eventSource.onmessage({ data: JSON.stringify({
    tract: 'navigate', path: '/app/settings/?facet=project', facet: 'project',
  }) });
  assert.strictEqual(pathAndFacet.window.location.href, '/app/settings/?facet=project');
  assert.deepStrictEqual(pathAndFacet.document.cookieWrites, []);

  const facetOnly = websocketHarness();
  facetOnly.eventSource.onmessage({ data: JSON.stringify({ tract: 'navigate', facet: 'project' }) });
  assert.strictEqual(facetOnly.window.location.href, '/app/entities/');
  assert.deepStrictEqual(facetOnly.document.cookieWrites, []);
}

function testNotificationCardAction() {
  const document = cookieDocument();
  class Element {}
  const window = {
    document,
    location: { href: '/app/home/' },
    ConveyIcons: { svg() { return ''; } },
    convey: {},
    CONVEY_COPY: {},
    selectFacet() { throw new Error('notification card must not select a global facet'); },
  };
  window.window = window;
  const context = vm.createContext({
    window,
    document,
    Element,
    localStorage: { getItem() { return null; }, setItem() {} },
    navigator: {},
    console,
    Map,
    Set,
    Date,
    JSON,
    Promise,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
  });
  vm.runInContext(staticSource('app.js'), context, { filename: 'app.js' });
  let defaultPrevented = false;
  const card = {
    tagName: 'A',
    dataset: {},
    querySelector() { return null; },
  };
  window.AppServices.notifications._attachClickHandler(card, { action: '/app/import/?setting=from-card' });
  card.onclick({
    target: { closest() { return null; } },
    preventDefault() { defaultPrevented = true; },
  });
  assert.ok(defaultPrevented, 'notification anchors retain their navigation click behavior');
  assert.strictEqual(window.location.href, '/app/import/?setting=from-card');
  assert.deepStrictEqual(card.dataset, {});
  assert.deepStrictEqual(document.cookieWrites, []);
}

function testStatusHistoryAction() {
  const document = cookieDocument();
  const history = { innerHTML: '' };
  document.getElementById = (id) => id === 'notification-history' ? history : null;
  const window = {
    document,
    selectFacet() { throw new Error('status history must not select a global facet'); },
    AppServices: {
      escapeHtml(value) { return String(value); },
      notifications: {
        getHistory() {
          return [{
            action: '/app/entities/?setting=history', icon: 'mailbox', title: 'Updated', timestamp: Date.now(),
          }];
        },
        _resolveIcon() { return ''; },
        _getRelativeTime() { return 'just now'; },
      },
    },
  };
  window.window = window;
  const context = vm.createContext({ window, document, Date, console });
  vm.runInContext('let statusPaneOpen = true; let _lastHistoryLen = -1;', context);
  vm.runInContext(functionSource(staticSource('status_pane.js'), 'updateNotificationHistory'), context);
  vm.runInContext('updateNotificationHistory()', context);
  assert.ok(history.innerHTML.includes('<a href="/app/entities/?setting=history"'));
  assert.ok(!history.innerHTML.includes('data-facet'));
  assert.deepStrictEqual(document.cookieWrites, []);
}

testNavigateMessages();
testNotificationCardAction();
testStatusHistoryAction();
console.log('DOM CASES: 3 passed');
