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
    addEventListener(type, handler) { this[type] = handler; }
    close() {}
  }
  const window = {
    document,
    location: { href: '/app/entities/', origin: 'http://localhost' },
    setTimeout,
    clearTimeout,
    AppServices: { sameOriginPath: appHarness().AppServices.sameOriginPath, notifications: { show() {} } },
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

function appHarness() {
  const document = cookieDocument();
  class Element {}
  const window = {
    document,
    location: { href: '/app/home/', origin: 'http://localhost' },
    ConveyIcons: { svg() { return ''; } },
    convey: {},
    CONVEY_COPY: {},
    selectFacet() { throw new Error('notification card must not select a global facet'); },
  };
  window.window = window;
  const context = vm.createContext({
    window,
    document,
    URL,
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
  return window;
}

function testNotificationCardAction() {
  const window = appHarness();
  const document = window.document;
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
      sameOriginPath: appHarness().AppServices.sameOriginPath,
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

function testContinuity() {
  const {eventSource, window} = websocketHarness();
  assert.strictEqual(window.appEvents.getMetrics().state, 'connecting', 'HTTP open is not bus evidence');
  let dataCount = 0;
  window.appEvents.listen('*', () => dataCount++);
  eventSource.continuity({data: JSON.stringify({state: 'connected'})});
  assert.strictEqual(window.appEvents.getMetrics().connected, true);
  assert.strictEqual(window.appEvents.getMetrics().lastMessageAt, null);
  eventSource.onmessage({data: JSON.stringify({tract: 'supervisor', event: 'status'})});
  assert.strictEqual(dataCount, 1);
  eventSource.continuity({data: JSON.stringify({state: 'gapped'})});
  assert.strictEqual(window.appEvents.getMetrics().connected, false);
  assert.strictEqual(window.appEvents.getMetrics().lastMessageAt, null);
  assert.strictEqual(dataCount, 1, 'control traffic must not become activity');
  eventSource.continuity({data: JSON.stringify({state: 'connected'})});
  assert.strictEqual(dataCount, 1, 'transport recovery does not replay missed data');
}

function testNotificationBoundaries() {
  const window = appHarness();
  const services = window.AppServices;
  const notifications = services.notifications;
  notifications._render = () => {};
  notifications._startDismissTimer = () => {};
  for (const action of ['javascript:alert(1)', '//evil.example/', '/\\evil.example/', 'https://evil.example/', '/.//evil.example/', 3]) {
    assert.strictEqual(services.sameOriginPath(action), null);
    const id = notifications.show({action, autoDismiss: '"><img src=x onerror=alert(1)>'});
    const item = notifications._stack.find(n => n.id === id);
    assert.strictEqual(item.action, null);
    assert.strictEqual(item.autoDismiss, null);
  }
  const valid = '/app/health/?q="hello"#detail';
  assert.strictEqual(services.sameOriginPath(valid), '/app/health/?q=%22hello%22#detail');
  const id = notifications.show({key: 'test', action: valid, autoDismiss: 3000});
  assert.strictEqual(notifications._stack.find(n => n.id === id).autoDismiss, 3000);
  notifications.show({key: 'test', action: 'javascript:alert(1)'});
  assert.strictEqual(notifications._stack.find(n => n.id === id).action, null);
  const h = websocketHarness();
  for (const path of ['javascript:alert(1)', '//evil.example/', '/\\evil.example/']) {
    h.eventSource.onmessage({data: JSON.stringify({tract:'navigate', path})});
    assert.strictEqual(h.window.location.href, '/app/entities/');
  }
}

testContinuity();
testNotificationBoundaries();
testNavigateMessages();
testNotificationCardAction();
testStatusHistoryAction();
console.log('DOM CASES: 5 passed');
