// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');
const generatedShell = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
assert.ok(Array.isArray(generatedShell.apps) && generatedShell.apps.length > 0, 'generated apps');
assert.strictEqual(generatedShell.pinned, undefined, 'cached payload carries no pinned list');

class ClassList {
  constructor(element) {
    this.element = element;
    this.values = new Set();
  }

  setFromString(value) {
    this.values = new Set(String(value || '').split(/\s+/).filter(Boolean));
  }

  sync() {
    if (this.values.size) {
      this.element.attributes.class = Array.from(this.values).join(' ');
    } else {
      delete this.element.attributes.class;
    }
  }

  add(...names) {
    names.forEach((name) => this.values.add(name));
    this.sync();
  }

  remove(...names) {
    names.forEach((name) => this.values.delete(name));
    this.sync();
  }

  toggle(name, force) {
    const present = force === undefined ? !this.values.has(name) : force;
    if (present) this.values.add(name);
    else this.values.delete(name);
    this.sync();
    return present;
  }

  contains(name) {
    return this.values.has(name);
  }
}

function dataKey(attribute) {
  return attribute.slice(5).replace(/-([a-z])/g, (_, character) => character.toUpperCase());
}

function parseAttributes(source, element) {
  const attributePattern = /([^\s=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g;
  let match;
  while ((match = attributePattern.exec(source))) {
    const name = match[1];
    const value = match[2] ?? match[3] ?? match[4] ?? '';
    element.setAttribute(name, value);
  }
}

function parseHtml(html, root) {
  root.children.slice().forEach((child) => root.removeChild(child));
  const stack = [root];
  const tagPattern = /<\/?[^>]+>/g;
  let match;
  while ((match = tagPattern.exec(html))) {
    const token = match[0];
    if (token.startsWith('</')) {
      const tagName = token.slice(2, -1).trim().toUpperCase();
      const index = stack.map((element) => element.tagName).lastIndexOf(tagName);
      if (index > 0) stack.length = index;
      continue;
    }
    if (token.startsWith('<!')) continue;
    const selfClosing = token.endsWith('/>');
    const parts = /^<\s*([^\s/>]+)([\s\S]*?)\/?>$/.exec(token);
    if (!parts) continue;
    const child = root.ownerDocument.createElement(parts[1]);
    parseAttributes(parts[2], child);
    stack[stack.length - 1].appendChild(child);
    if (!selfClosing && !['BR', 'IMG', 'INPUT', 'META', 'LINK'].includes(child.tagName)) {
      stack.push(child);
    }
  }
}

function selectorMatches(element, selector) {
  selector = selector.trim();
  const notMatch = selector.match(/:not\(([^)]+)\)$/);
  if (notMatch) {
    if (selectorMatches(element, notMatch[1])) return false;
    selector = selector.slice(0, notMatch.index);
  }

  const attributes = [...selector.matchAll(/\[([^\]=]+)(?:=["']?([^\]"']+)["']?)?\]/g)];
  selector = selector.replace(/\[[^\]]+\]/g, '');
  const idMatch = selector.match(/#([\w-]+)/);
  const classMatches = [...selector.matchAll(/\.([\w-]+)/g)];
  const tagMatch = selector.match(/^[a-zA-Z][\w-]*/);
  if (tagMatch && element.tagName !== tagMatch[0].toUpperCase()) return false;
  if (idMatch && element.id !== idMatch[1]) return false;
  if (classMatches.some((match) => !element.classList.contains(match[1]))) return false;
  return attributes.every((match) => {
    if (!element.hasAttribute(match[1])) return false;
    return match[2] === undefined || element.getAttribute(match[1]) === match[2];
  });
}

function matchesSelector(element, selector) {
  return selector.split(',').some((part) => selectorMatches(element, part));
}

function queryAll(root, selector) {
  const pieces = selector.trim().split(/\s+/);
  let candidates = [root];
  for (const piece of pieces) {
    const next = [];
    for (const candidate of candidates) {
      const visit = (element) => {
        element.children.forEach((child) => {
          if (matchesSelector(child, piece)) next.push(child);
          visit(child);
        });
      };
      visit(candidate);
    }
    candidates = next;
  }
  return candidates;
}

const observers = [];
function notifyMutation(record) {
  for (const observer of observers) {
    if (!observer.connected || !observer.options.attributes) continue;
    if (observer.options.attributeFilter && !observer.options.attributeFilter.includes(record.attributeName)) continue;
    observer.callback([record]);
  }
}

class Element {
  constructor(tagName, ownerDocument) {
    this.tagName = tagName.toUpperCase();
    this.ownerDocument = ownerDocument;
    this.attributes = {};
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.style = {};
    this.dataset = {};
    this.classList = new ClassList(this);
    this.value = '';
    this.disabled = false;
    this.isContentEditable = false;
    this._innerHTML = '';
  }

  get id() {
    return this.getAttribute('id') || '';
  }

  set id(value) {
    this.setAttribute('id', value);
  }

  get className() {
    return this.getAttribute('class') || '';
  }

  set className(value) {
    this.setAttribute('class', value);
  }

  get href() {
    return this.getAttribute('href') || '';
  }

  set href(value) {
    if (value) this.setAttribute('href', value);
    else this.removeAttribute('href');
  }

  get hidden() {
    return this.hasAttribute('hidden');
  }

  set hidden(value) {
    if (value) this.setAttribute('hidden', '');
    else this.removeAttribute('hidden');
  }

  get inert() {
    return this.hasAttribute('inert');
  }

  set inert(value) {
    if (value) this.setAttribute('inert', '');
    else this.removeAttribute('inert');
  }

  get isConnected() {
    let current = this;
    while (current.parentElement) current = current.parentElement;
    return current === this.ownerDocument.body || current === this.ownerDocument.documentElement;
  }

  get innerHTML() {
    return this._innerHTML;
  }

  set innerHTML(value) {
    this._innerHTML = String(value);
    parseHtml(this._innerHTML, this);
  }

  get textContent() {
    return this._textContent || '';
  }

  set textContent(value) {
    this._textContent = String(value);
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'class') this.classList.setFromString(value);
    if (name.startsWith('data-')) this.dataset[dataKey(name)] = String(value);
    notifyMutation({ target: this, attributeName: name });
  }

  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  hasAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name);
  }

  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'class') this.classList.setFromString('');
    if (name.startsWith('data-')) delete this.dataset[dataKey(name)];
    notifyMutation({ target: this, attributeName: name });
  }

  appendChild(child) {
    if (child.parentElement) child.parentElement.removeChild(child);
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  append(...children) {
    children.forEach((child) => this.appendChild(child));
  }

  replaceChildren(...children) {
    this.children.slice().forEach(child => this.removeChild(child));
    this.append(...children);
  }

  insertBefore(child, before) {
    if (!before) return this.appendChild(child);
    if (child.parentElement) child.parentElement.removeChild(child);
    child.parentElement = this;
    this.children.splice(this.children.indexOf(before), 0, child);
    return child;
  }

  removeChild(child) {
    this.children = this.children.filter((item) => item !== child);
    child.parentElement = null;
  }

  contains(candidate) {
    if (candidate === this) return true;
    return this.children.some((child) => child.contains(candidate));
  }

  matches(selector) {
    return matchesSelector(this, selector);
  }

  closest(selector) {
    let current = this;
    while (current) {
      if (current.matches(selector)) return current;
      current = current.parentElement;
    }
    return null;
  }

  querySelector(selector) {
    return queryAll(this, selector)[0] || null;
  }

  querySelectorAll(selector) {
    return queryAll(this, selector);
  }

  addEventListener(type, listener) {
    (this.listeners[type] ||= []).push(listener);
  }

  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    event.preventDefault ||= function () { this.defaultPrevented = true; };
    event.stopPropagation ||= function () { this.cancelBubble = true; };
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    if (event.bubbles && !event.cancelBubble && this.parentElement) this.parentElement.dispatchEvent(event);
    else if (event.bubbles && !event.cancelBubble && this === this.ownerDocument.documentElement) {
      this.ownerDocument.dispatchEvent(event);
    }
    return !event.defaultPrevented;
  }

  getClientRects() {
    return this.isConnected && !this.hidden && this.style.display !== 'none' ? [{}] : [];
  }

  focus() {
    this.ownerDocument.activeElement = this;
  }
}

class Document {
  constructor() {
    this.listeners = {};
    this.documentElement = new Element('html', this);
    this.body = new Element('body', this);
    this.documentElement.appendChild(this.body);
    this.activeElement = this.body;
  }

  createElement(tagName) {
    return new Element(tagName, this);
  }

  getElementById(id) {
    return this.querySelector('#' + id);
  }

  querySelector(selector) {
    if (selector === 'body') return this.body;
    if (selector === 'html') return this.documentElement;
    return this.documentElement.querySelector(selector);
  }

  querySelectorAll(selector) {
    return this.documentElement.querySelectorAll(selector);
  }

  addEventListener(type, listener) {
    (this.listeners[type] ||= []).push(listener);
  }

  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    event.preventDefault ||= function () { this.defaultPrevented = true; };
    event.stopPropagation ||= function () { this.cancelBubble = true; };
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    return !event.defaultPrevented;
  }
}

class MutationObserverShim {
  constructor(callback) {
    this.callback = callback;
    this.connected = false;
    this.options = {};
    observers.push(this);
  }

  observe(_target, options) {
    this.connected = true;
    this.options = options;
  }
}

function event(type, options = {}) {
  return {
    type,
    bubbles: Boolean(options.bubbles),
    key: options.key,
    detail: options.detail,
    target: options.target,
    defaultPrevented: false,
    cancelBubble: false,
    preventDefault() { this.defaultPrevented = true; },
    stopPropagation() { this.cancelBubble = true; },
    stopImmediatePropagation() { this.cancelBubble = true; },
  };
}

function addStaticElement(document, tagName, attributes = {}) {
  const element = document.createElement(tagName);
  Object.entries(attributes).forEach(([name, value]) => element.setAttribute(name, value));
  document.body.appendChild(element);
  return element;
}

function shellFixture(options = {}) {
  const shell = JSON.parse(JSON.stringify(generatedShell));
  shell.pinned = Object.prototype.hasOwnProperty.call(options, 'pinned')
    ? options.pinned.slice()
    : ['search', 'entities', 'thinking'];
  if (options.boot) {
    const support = shell.apps.find((app) => app.name === 'support');
    if (support) support.background_url = '/app/support/background';
  }
  return shell;
}

class AbortController {
  constructor() {
    this.signal = {
      aborted: false,
      listeners: [],
      addEventListener(_type, listener) { this.listeners.push(listener); },
    };
  }

  abort() {
    if (this.signal.aborted) return;
    this.signal.aborted = true;
    for (const listener of this.signal.listeners) listener();
  }
}

function createHarness(options = {}) {
  const fixtureOptions = { boot: options.boot };
  if (Object.prototype.hasOwnProperty.call(options, 'pinned')) {
    fixtureOptions.pinned = options.pinned;
  }
  const shell = options.shell || shellFixture(fixtureOptions);
  const fetchCalls = [];
  const surfaceCalls = [];
  const logErrors = [];
  const backgroundFailures = [];
  const readyCalls = [];
  const setTimeoutFn = options.setTimeout || setTimeout;
  const clearTimeoutFn = options.clearTimeout || clearTimeout;
  const fetch = (url) => {
    const requested = String(url);
    fetchCalls.push(requested);
    const fixture = options.fetchResponses?.[requested];
    if (!fixture) {
      if (requested === '/api/streams/labels') {
        return Promise.resolve({
          ok: true,
          status: 200,
          text() { return Promise.resolve(JSON.stringify({ status: 'complete', labels: {} })); },
          json() { return Promise.resolve({ status: 'complete', labels: {} }); },
        });
      }
      return Promise.reject(new Error('unexpected fetch ' + requested));
    }
    const status = fixture.status !== undefined ? fixture.status : 200;
    const body = fixture.body || '';
    return Promise.resolve({
      ok: fixture.ok !== undefined ? fixture.ok : (status >= 200 && status < 300),
      status,
      text() { return typeof fixture.text === 'function' ? fixture.text() : Promise.resolve(body); },
      json() { return typeof fixture.json === 'function' ? fixture.json() : Promise.resolve().then(() => JSON.parse(body)); },
    });
  };
  const document = new Document();
  addStaticElement(document, 'nav', { id: 'app-rail', class: 'app-rail', 'aria-label': 'views' });
  addStaticElement(document, 'nav', { id: 'app-dock', class: 'app-dock', 'aria-label': 'views' });
  addStaticElement(document, 'div', { id: 'status-instrument' });
  addStaticElement(document, 'div', { id: 'status-pane', class: 'status-pane' });
  addStaticElement(document, 'main', { id: 'main-content', tabindex: '-1' });

  const windowListeners = {};
  const navigator = { userAgent: options.userAgent || 'Mozilla/5.0 (X11; Linux x86_64)' };
  const location = {
    href: 'http://localhost' + (options.pathname || '/app/home/'),
    origin: 'http://localhost',
    pathname: options.pathname || '/app/home/',
    assigned: null,
    assign(url) { this.assigned = url; },
  };
  const window = {
    document,
    Element,
    MutationObserver: MutationObserverShim,
    setTimeout: setTimeoutFn,
    clearTimeout: clearTimeoutFn,
    navigator,
    location,
    top: null,
    matchMedia(query) {
      const text = String(query || '');
      const matches = text.includes('hover: none') && text.includes('pointer: coarse')
        ? Boolean(options.coarse)
        : Boolean(options.mobile);
      return { matches, addEventListener() {} };
    },
    requestAnimationFrame(callback) { callback(); },
    getComputedStyle(element) {
      return {
        display: element.hidden || element.style.display === 'none' ? 'none' : 'block',
        position: element.style.position || 'static',
      };
    },
    sessionStorage: { getItem() { return null; }, setItem() {}, removeItem() {} },
    localStorage: { getItem() { return null; }, setItem() {}, removeItem() {} },
    URL,
    addEventListener(type, listener) { (windowListeners[type] ||= []).push(listener); },
    removeEventListener(type, listener) {
      windowListeners[type] = (windowListeners[type] || []).filter((item) => item !== listener);
    },
    dispatchEvent(dispatched) {
      dispatched.target ||= window;
      for (const listener of windowListeners[dispatched.type] || []) listener.call(window, dispatched);
      return !dispatched.defaultPrevented;
    },
    CustomEvent: class CustomEvent {
      constructor(type, options = {}) { this.type = type; this.detail = options.detail; }
    },
    whenShellReady(callback) { callback(shell); },
    resolveSolShellReady(data) { readyCalls.push(data); },
    apiJson(url) {
      if (url === '/api/shell') return Promise.resolve(shell);
      return Promise.reject(new Error('unexpected apiJson ' + url));
    },
    fetch,
  };
  if (options.boot) {
    window.SurfaceState = {
      empty(state) {
        surfaceCalls.push({ kind: 'empty', state });
        return '<div class="surface-state surface-state--empty" data-surface-state="empty"></div>';
      },
      error(state) {
        surfaceCalls.push({ kind: 'error', state });
        return '<div class="surface-state surface-state--error" data-surface-state="error"></div>';
      },
    };
    window.logError = (error, context) => { logErrors.push({ error, context }); };
    window.AppServices = {
      markBackgroundFailing(appName, error) { backgroundFailures.push({ appName, error }); },
    };
  }
  window.window = window;
  window.top = window;
  const context = vm.createContext({
    window,
    document,
    Element,
    navigator,
    location,
    localStorage: window.localStorage,
    MutationObserver: MutationObserverShim,
    console,
    URL,
    URLSearchParams,
    setTimeout: setTimeoutFn,
    clearTimeout: clearTimeoutFn,
    requestAnimationFrame: window.requestAnimationFrame,
    getComputedStyle: window.getComputedStyle,
    CustomEvent: window.CustomEvent,
    fetch,
    AbortController,
  });
  const read = (file) => fs.readFileSync(path.join(crateDir, 'assets/static', file), 'utf8');
  const surfaceStub = window.SurfaceState;
  const servicesStub = window.AppServices;
  vm.runInContext(read('convey_icons.js'), context, { filename: 'convey_icons.js' });
  vm.runInContext(read('app.js'), context, { filename: 'app.js' });
  if (options.boot) {
    window.SurfaceState = surfaceStub;
    Object.assign(window.AppServices, servicesStub);
  }
  vm.runInContext(read('journal-web-host.js'), context, { filename: 'journal-web-host.js' });
  if (options.capable) {
    navigator.userAgent = `${navigator.userAgent} ${window.solstoneJournalWebHost.userAgentProduct}`;
  }
  if (options.capability) {
    window[window.solstoneJournalWebHost.javascriptCapability] = 1;
  }
  vm.runInContext(read('external-navigation.js'), context, { filename: 'external-navigation.js' });
  vm.runInContext(read('modal_layer.js'), context, { filename: 'modal_layer.js' });
  vm.runInContext(read('presentation_mode.js'), context, { filename: 'presentation_mode.js' });
  if (options.report) {
    vm.runInContext(read('report-error.js'), context, { filename: 'report-error.js' });
  }
  vm.runInContext(read('date_format.js'), context, { filename: 'date_format.js' });
  if (options.boot) {
    vm.runInContext(read('convey_copy.js'), context, { filename: 'convey_copy.js' });
    vm.runInContext(read('mount-workspace.js'), context, { filename: 'mount-workspace.js' });
  }
  const bootSource = read('shell_boot.js').replace(
    /\s*boot\(\);\s*\}\)\(\);\s*$/,
    '\n  window.__shellChrome = window.solShellChrome;\n'
      + (options.boot ? '  window.__shellBoot = boot;\n' : '')
      + '})();\n',
  );
  vm.runInContext(bootSource, context, { filename: 'shell_boot.js' });
  vm.runInContext(
    fs.readFileSync(path.join(crateDir, 'assets/more/more.js'), 'utf8'),
    context,
    { filename: 'more.js' },
  );
  return {
    document,
    window,
    chrome: window.__shellChrome,
    shell,
    boot: options.boot ? () => window.__shellBoot() : null,
    fetchCalls,
    surfaceCalls,
    logErrors,
    backgroundFailures,
    readyCalls,
  };
}

function renderChrome(harness, currentAppName) {
  const app = harness.shell.apps.find((candidate) => candidate.name === currentAppName);
  assert.ok(app, 'fixture app ' + currentAppName + ' exists');
  harness.window.location.pathname = '/app/' + currentAppName + '/';
  harness.chrome.renderAppRail(harness.shell, currentAppName);
  harness.chrome.renderAppDock(harness.shell, currentAppName);
  harness.chrome.renderStatusInstrument();
}

function namedApps(slot) {
  return slot.querySelectorAll('[data-app-name]').map((element) => element.getAttribute('data-app-name'));
}

function moreLabel(html) {
  const match = html.match(/data-app-name="more"[^>]*>[\s\S]*?<span class="app-chrome-label">([^<]*)<\/span>/);
  return match && match[1];
}

function appNames(elements) {
  return elements.map((element) => {
    const named = element.getAttribute('data-app-name');
    if (named) return named;
    const link = element.querySelector('a');
    return link.getAttribute('href').split('/').filter(Boolean)[1];
  });
}

let cases = 0;
function testCase(name, fn) {
  try {
    fn();
    cases += 1;
  } catch (error) {
    error.message = name + ': ' + error.message;
    throw error;
  }
}

const asyncCases = [];
function asyncCase(name, fn) {
  asyncCases.push(async () => {
    try {
      await fn();
      cases += 1;
    } catch (error) {
      error.message = name + ': ' + error.message;
      throw error;
    }
  });
}

testCase('launcher markup and wiring are gone', () => {
  const html = fs.readFileSync(path.join(crateDir, 'assets/static/shell.html'), 'utf8');
  const boot = fs.readFileSync(path.join(crateDir, 'assets/static/shell_boot.js'), 'utf8');
  const css = fs.readFileSync(path.join(crateDir, 'assets/static/app.css'), 'utf8');
  assert.ok(!html.includes('app-launcher'));
  assert.ok(!html.includes('journal apps'));
  assert.ok(html.includes('aria-label="views"'));
  assert.ok(!boot.includes('renderAppLauncher'));
  assert.ok(!boot.includes('data-app-launcher'));
  assert.ok(!boot.includes('presentation-mode:change'));
  assert.ok(!boot.includes('journal apps'));
  assert.ok(!css.includes('app-launcher'));
});

testCase('app navigation uses full documents, never workspace fragments', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  for (const slot of ['#app-rail', '#app-dock']) {
    const links = harness.document.querySelector(slot).querySelectorAll('a');
    assert(links.length > 0);
    for (const link of links) {
      const href = link.getAttribute('href');
      if (href.includes('/app/health/#') || href === '/app/settings/#about') continue;
      assert(/^\/app\/[^/]+\/$/.test(href), `${slot}: ${href} must open the full application`);
    }
  }
});

testCase('version survives chrome navigation and opens about', () => {
  const harness = createHarness();
  for (const app of ['home', 'network', 'health', 'settings']) {
    renderChrome(harness, app);
    const link = harness.document.querySelector('#app-rail .journal-about-link');
    assert.ok(link, 'rail has a version destination');
    assert.strictEqual(link.getAttribute('href'), '/app/settings/#about');
    assert.strictEqual(harness.document.querySelector('#app-dock .journal-about-link'), null);
  }
});

testCase('status instrument stays the same node across chrome rebuilds', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const icon = harness.document.querySelector('#status-instrument .status-icon');
  renderChrome(harness, 'search');
  assert.strictEqual(harness.document.querySelector('#status-instrument .status-icon'), icon);
});

testCase('rail and dock follow the resolved pin list', () => {
  const pinned = createHarness({ pinned: ['map', 'search'] });
  renderChrome(pinned, 'home');
  const rail = pinned.document.querySelector('#app-rail');
  assert.strictEqual(rail.getAttribute('aria-label'), 'views');
  assert.strictEqual(rail.children.length, 12);
  assert.deepStrictEqual(namedApps(rail), ['home', 'map', 'search', 'more', 'import', 'settings', 'network', 'agents']);
  assert.ok(!namedApps(rail).includes('entities'));
  assert.ok(!namedApps(rail).includes('thinking'));
  assert.ok(rail.children[4].classList.contains('app-rail-spacer'));
  assert.strictEqual(rail.children[5].id, 'status-instrument');
  assert.ok(rail.children[6].classList.contains('app-rail-divider'));
  assert.deepStrictEqual(
    ['import', 'settings', 'network', 'agents'],
    rail.children.slice(7, 11).map((element) => element.getAttribute('data-app-name'))
  );
  assert.ok(rail.children[11].classList.contains('journal-about-link'));
  const desktopDock = pinned.document.querySelector('#app-dock');
  assert.deepStrictEqual(namedApps(desktopDock), ['home', 'map', 'search', 'more']);
  assert.strictEqual(desktopDock.children.length, 4);

  const phone = createHarness({ pinned: ['map', 'search'], mobile: true });
  renderChrome(phone, 'home');
  const dock = phone.document.querySelector('#app-dock');
  assert.strictEqual(dock.getAttribute('aria-label'), 'views');
  assert.strictEqual(dock.children.length, 5);
  assert.deepStrictEqual(namedApps(dock), ['home', 'map', 'search', 'more']);
  assert.strictEqual(dock.children[4].id, 'status-instrument');

  const empty = createHarness({ pinned: [] });
  renderChrome(empty, 'home');
  const emptyRail = empty.document.querySelector('#app-rail');
  assert.strictEqual(emptyRail.children.length, 10);
  assert.deepStrictEqual(namedApps(emptyRail), ['home', 'more', 'import', 'settings', 'network', 'agents']);
  assert.deepStrictEqual(namedApps(empty.document.querySelector('#app-dock')), ['home', 'more']);

  const emptyPhone = createHarness({ pinned: [], mobile: true });
  renderChrome(emptyPhone, 'home');
  const emptyDock = emptyPhone.document.querySelector('#app-dock');
  assert.strictEqual(emptyDock.children.length, 3);
  assert.deepStrictEqual(namedApps(emptyDock), ['home', 'more']);
  assert.strictEqual(emptyDock.children[2].id, 'status-instrument');
});

testCase('more keeps its label and current treatment follows each surface', () => {
  const pinned = ['map', 'search'];
  for (const name of ['home', 'activities', 'more']) {
    const harness = createHarness({ pinned });
    renderChrome(harness, name);
    assert.strictEqual(moreLabel(harness.document.querySelector('#app-rail').innerHTML), 'more');
    assert.strictEqual(moreLabel(harness.document.querySelector('#app-dock').innerHTML), 'more');
  }

  const onMore = createHarness({ pinned });
  renderChrome(onMore, 'more');
  for (const slot of ['#app-rail', '#app-dock']) {
    const link = onMore.document.querySelector(slot + ' [data-app-name="more"]');
    assert.ok(link.classList.contains('is-current'));
    assert.strictEqual(link.getAttribute('aria-current'), 'page');
  }

  const offRail = createHarness({ pinned });
  renderChrome(offRail, 'activities');
  for (const slot of ['#app-rail', '#app-dock']) {
    const link = offRail.document.querySelector(slot + ' [data-app-name="more"]');
    assert.ok(link.classList.contains('is-current'));
    assert.ok(!link.hasAttribute('aria-current'));
  }

  const third = createHarness({ pinned: ['map', 'search', 'entities'] });
  renderChrome(third, 'entities');
  const railMore = third.document.querySelector('#app-rail [data-app-name="more"]');
  const dockMore = third.document.querySelector('#app-dock [data-app-name="more"]');
  assert.ok(third.document.querySelector('#app-rail [data-app-name="entities"]').classList.contains('is-current'));
  assert.ok(!railMore.classList.contains('is-current'));
  assert.ok(!railMore.hasAttribute('aria-current'));
  assert.ok(dockMore.classList.contains('is-current'));
  assert.ok(!dockMore.hasAttribute('aria-current'));
  assert.strictEqual(moreLabel(third.document.querySelector('#app-rail').innerHTML), 'more');
  assert.strictEqual(moreLabel(third.document.querySelector('#app-dock').innerHTML), 'more');
});

function viewBlob(app) {
  return [app.label, app.name, app.description, app.source, ...(app.keywords || [])].join(' ').toLowerCase();
}

async function paintMore(harness, options) {
  const root = addStaticElement(harness.document, 'div', { id: 'more-root' });
  await harness.window.renderMorePage(root, harness.shell, options || { timeout: 30 });
  return root;
}

asyncCase('more page groups every served view except more', async () => {
  const harness = createHarness({ pinned: ['search', 'entities', 'thinking'] });
  const root = await paintMore(harness);
  const html = root.innerHTML;
  assert.ok(html.includes('<h1>more</h1>'));
  assert.ok(html.includes('every view your journal has: what each one shows, and where its contents come from.'));
  assert.ok(html.indexOf('what your journal keeps') < html.indexOf('what you shape'));
  assert.ok(html.indexOf('what you shape') < html.indexOf('how it runs'));
  assert.ok(html.includes('your days, as your journal holds them'));
  assert.ok(html.includes('fix and tidy what it keeps'));
  assert.ok(html.includes('set it up and keep it running'));
  assert.ok(html.includes('placeholder="type to filter views"'));
  assert.ok(html.includes('>filter views</label>'));
  const input = root.querySelector('#more-filter');
  assert.notStrictEqual(harness.document.activeElement, input);

  const groupNames = (group) => root.querySelectorAll('[data-focus="' + group + '"] .more-card')
    .map((card) => card.getAttribute('data-app-name'));
  assert.deepStrictEqual(groupNames('keeps'), ['home', 'activities', 'transcripts', 'map', 'body', 'search']);
  assert.deepStrictEqual(groupNames('shape'), ['entities', 'speakers', 'curation']);
  assert.deepStrictEqual(groupNames('runs'), ['thinking', 'health', 'network', 'agents', 'backup', 'import', 'stats', 'settings', 'support']);
  const names = root.querySelectorAll('.more-card').map((card) => card.getAttribute('data-app-name'));
  assert.strictEqual(new Set(names).size, names.length);
  assert.ok(!names.includes('more'));
  const served = harness.shell.apps.map((app) => app.name).filter((name) => name !== 'more').sort();
  assert.deepStrictEqual(names.slice().sort(), served);

  for (const card of root.querySelectorAll('.more-card')) {
    const name = card.getAttribute('data-app-name');
    const app = harness.shell.apps.find((item) => item.name === name);
    const link = card.querySelector('a.more-card-link');
    const button = card.querySelector('button');
    assert.strictEqual(link.getAttribute('href'), '/app/' + name + '/');
    if (app.pinnable) {
      assert.ok(button, name);
      assert.strictEqual(button.parentElement, card);
      assert.strictEqual(link.querySelector('button'), null);
      const pinned = harness.shell.pinned.includes(name);
      assert.strictEqual(button.getAttribute('aria-pressed'), pinned ? 'true' : 'false');
    } else {
      assert.strictEqual(button, null, name);
      assert.strictEqual(card.querySelector('.more-pin-static').getAttribute('title'), 'always in the side rail');
    }
  }
  const curation = root.querySelector('[data-pin-view="curation"]');
  assert.strictEqual(curation.getAttribute('aria-label'), 'pin suggestions to the side rail');
  assert.strictEqual(curation.getAttribute('title'), 'pin suggestions to the side rail');
});

asyncCase('more page filter uses a generated keyword and escape clears it', async () => {
  const harness = createHarness({ pinned: [] });
  const root = await paintMore(harness);
  const map = harness.shell.apps.find((app) => app.name === 'map');
  const keyword = map.keywords.find((word) => {
    const query = word.toLowerCase();
    const hits = harness.shell.apps.filter((app) => app.name !== 'more' && viewBlob(app).includes(query));
    return hits.length === 1 && hits[0].name === 'map';
  });
  assert.ok(keyword, 'a generated map keyword matches only map');
  const input = root.querySelector('#more-filter');
  input.focus();
  input.value = keyword.toUpperCase();
  input.dispatchEvent(event('input'));
  assert.deepStrictEqual(
    root.querySelectorAll('.more-card:not([hidden])').map((card) => card.getAttribute('data-app-name')),
    ['map']
  );
  const visibleGroups = root.querySelectorAll('.more-group:not([hidden])');
  assert.strictEqual(visibleGroups.length, 1);
  assert.strictEqual(visibleGroups[0].getAttribute('data-focus'), 'keeps');

  input.value = 'zzzz-no-view';
  input.dispatchEvent(event('input'));
  assert.strictEqual(root.querySelectorAll('.more-card:not([hidden])').length, 0);
  assert.strictEqual(root.querySelectorAll('.more-group:not([hidden])').length, 0);
  assert.strictEqual(root.querySelector('.more-empty').hidden, false);
  assert.ok(root.innerHTML.includes('no views match that.'));

  harness.document.dispatchEvent(event('keydown', { key: 'Escape' }));
  assert.strictEqual(input.value, '');
  assert.strictEqual(root.querySelector('.more-empty').hidden, true);
  assert.strictEqual(root.querySelectorAll('.more-card[hidden]').length, 0);
});

asyncCase('successful pin toggle updates rail and dock from the returned list', async () => {
  const harness = createHarness({ pinned: ['search'] });
  renderChrome(harness, 'home');
  let release;
  const pending = new Promise((resolve) => { release = resolve; });
  const calls = [];
  harness.window.apiJson = (url, options) => {
    if (url === '/api/shell/pins') {
      calls.push(options);
      return pending;
    }
    return Promise.reject(new Error('state ' + url));
  };
  const root = await paintMore(harness);
  const button = root.querySelector('[data-pin-view="map"]');
  button.dispatchEvent(event('click', { bubbles: true }));
  assert.strictEqual(button.disabled, true);
  assert.strictEqual(calls.length, 1);
  button.dispatchEvent(event('click', { bubbles: true }));
  assert.strictEqual(calls.length, 1, 'a second click during the request issues no second request');
  release({ pinned: ['entities'] });
  await pending;
  await settle();
  await settle();
  assert.deepStrictEqual(namedApps(harness.document.querySelector('#app-rail')), ['home', 'entities', 'more', 'import', 'settings', 'network', 'agents']);
  assert.deepStrictEqual(namedApps(harness.document.querySelector('#app-dock')), ['home', 'entities', 'more']);
  assert.ok(!namedApps(harness.document.querySelector('#app-rail')).includes('map'));
  assert.ok(!namedApps(harness.document.querySelector('#app-rail')).includes('search'));
  assert.strictEqual(root.querySelector('[data-pin-view="map"]').getAttribute('aria-pressed'), 'false');
  assert.strictEqual(root.querySelector('[data-pin-view="entities"]').getAttribute('aria-pressed'), 'true');
  assert.strictEqual(root.querySelector('.more-live').textContent, '');
  assert.strictEqual(button.disabled, false);
});

asyncCase('failed pin save restores the previous list and says so', async () => {
  const harness = createHarness({ pinned: ['search'] });
  renderChrome(harness, 'home');
  let shellReads = 0;
  harness.window.apiJson = (url) => {
    if (url === '/api/shell') {
      shellReads += 1;
      return Promise.reject(new Error('read failed'));
    }
    return Promise.reject(new Error('save failed'));
  };
  const root = await paintMore(harness);
  root.querySelector('[data-pin-view="map"]').dispatchEvent(event('click', { bubbles: true }));
  await settle();
  await settle();
  assert.strictEqual(shellReads, 1);
  assert.deepStrictEqual(namedApps(harness.document.querySelector('#app-rail')), ['home', 'search', 'more', 'import', 'settings', 'network', 'agents']);
  assert.strictEqual(root.querySelector('.more-live').textContent, "couldn't save that. try again.");
  assert.strictEqual(root.querySelector('[data-pin-view="map"]').getAttribute('aria-pressed'), 'false');
});

asyncCase('state lines skip a rejection, a hang, and an unexpected shape', async () => {
  const harness = createHarness({ pinned: [] });
  harness.window.apiJson = (url, options) => {
    if (url === '/app/activities/api/index') return Promise.reject(new Error('no'));
    if (url === '/app/transcripts/api/index') {
      return new Promise((_resolve, reject) => {
        options.signal.addEventListener('abort', () => reject(new Error('aborted')));
      });
    }
    if (url === '/app/map/api/index') return Promise.resolve({ unexpected: true });
    if (url === '/app/body/api/index') return Promise.resolve({ coverage: null });
    if (url === '/app/speakers/api/index') return Promise.resolve({ coverage: { start: '20260303', end: '20261025' } });
    if (url === '/app/entities/api/journal/summary') return Promise.resolve({ folder_counts: { all: 4 } });
    if (url === '/app/import/api/list') return Promise.resolve({ total: 1 });
    if (url === '/app/network/api/clients') return Promise.resolve({ clients: [] });
    if (url === '/app/thinking/api/state') return Promise.resolve({ providers: { active_lane: { lane: 'none' }, saved_lane: { lane: 'local' } } });
    return Promise.reject(new Error(url));
  };
  const root = await paintMore(harness, { timeout: 40 });
  const stateOf = (name) => root.querySelector('.more-card[data-app-name="' + name + '"] .more-state');
  for (const name of ['activities', 'transcripts', 'map']) {
    assert.strictEqual(stateOf(name).hidden, true, name);
    assert.strictEqual(stateOf(name).textContent, '', name);
  }
  assert.strictEqual(stateOf('body').textContent, 'nothing yet');
  assert.ok(!stateOf('body').classList.contains('more-state-action'));
  const since = harness.window.JournalFormat.sinceDay('20260303');
  assert.ok(since);
  assert.strictEqual(stateOf('speakers').textContent, 'since ' + since);
  assert.strictEqual(stateOf('entities').textContent, '4 kept');
  assert.strictEqual(stateOf('import').textContent, '1 import');
  assert.strictEqual(stateOf('network').textContent, 'no devices yet');
  assert.ok(stateOf('network').classList.contains('more-state-action'));
  assert.strictEqual(stateOf('thinking').textContent, 'not set up yet');
  assert.ok(stateOf('thinking').classList.contains('more-state-action'));
  assert.strictEqual(root.querySelector('.more-live').textContent, '');
  assert.ok(!root.innerHTML.includes("couldn't"));
});

asyncCase('more boots with its own title', async () => {
  const harness = createHarness({
    boot: true,
    pathname: '/app/more/',
    pinned: ['search'],
    fetchResponses: {
      '/app/support/background': { status: 200, body: '' },
      '/app/more/workspace': { status: 200, body: '<div data-workspace="more"></div>' },
    },
  });
  await harness.boot();
  assert.strictEqual(harness.document.title, 'more - journal');
  assert.strictEqual(moreLabel(harness.document.querySelector('#app-rail').innerHTML), 'more');
});

testCase('status instrument target', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const icon = harness.document.querySelector('#status-instrument .status-icon');
  assert.ok(icon);
  assert.strictEqual(icon.tagName, 'A');
  assert.strictEqual(icon.getAttribute('href'), '/app/health/#healthSystemDetails');
  assert.ok(!icon.hasAttribute('aria-controls'));
});


asyncCase('generic workspace failures still reach the shell error state', async () => {
  const harness = createHarness({
    boot: true,
    fetchResponses: {
      '/app/support/background': { status: 200, body: '' },
      '/app/home/workspace': { status: 500, body: '<h1>server error</h1>' },
    },
  });

  await harness.boot();

  assert.strictEqual(harness.surfaceCalls.filter((call) => call.kind === 'empty').length, 0);
  assert.strictEqual(harness.surfaceCalls.filter((call) => call.kind === 'error').length, 2);
  assert.deepStrictEqual(
    harness.surfaceCalls.filter((call) => call.kind === 'error').map((call) => call.state.detail.status),
    [500, 500]
  );
  assert.ok(harness.document.querySelector('[data-surface-state="error"]'));
  assert.strictEqual(harness.logErrors.length, 2);
});

asyncCase('failure-time context reaches a later report without its stack', async () => {
  const harness = createHarness({
    report: true,
    pathname: '/app/health/',
    fetchResponses: {
      '/app/support/api/context': {
        status: 200,
        body: JSON.stringify({ version: '2.0.3', os: 'ubuntu', os_version: '24.04', about: 'journal 2.0.3 · ubuntu 24.04 · x86_64' }),
      },
    },
  });

  harness.window.convey.reportError({
    app: 'network',
    route: '/app/network/',
    apiError: {
      status: 404,
      message: 'Request failed (HTTP 404)',
      stack: 'at http://localhost:46171/static/mount-workspace.js:122:15',
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  const labels = harness.document.querySelectorAll('dt');
  const values = harness.document.querySelectorAll('dd');
  assert.strictEqual(labels[labels.length - 1].textContent, 'error code');
  assert.strictEqual(values[values.length - 1].textContent, '404');
  assert.strictEqual(values[0].textContent, 'journal 2.0.3 · ubuntu 24.04 · x86_64');
  assert.strictEqual(values[1].textContent, 'network');
  assert.strictEqual(values[2].textContent, '/app/network/');
  const recent = harness.document.querySelector('#report-error-recent');
  assert.strictEqual(recent.value, 'Request failed (HTTP 404)');
  assert.strictEqual(recent.value.includes('localhost'), false);
  const send = harness.document.querySelector('a');
  assert.strictEqual(send.hasAttribute('data-solstone-outside'), true);
  const fragment = new URLSearchParams(new URL(send.href).hash.slice(1));
  assert.strictEqual(fragment.get('about'), 'journal 2.0.3 · ubuntu 24.04 · x86_64');
  assert.strictEqual(fragment.get('app'), 'network');
  assert.strictEqual(fragment.get('route'), '/app/network/');
  assert.strictEqual(fragment.get('error_code'), '404');
  assert.strictEqual(fragment.get('recent'), 'Request failed (HTTP 404)');
});

asyncCase('capable host intercepts report-error send link click and assigns dynamically updated URL', async () => {
  const harness = createHarness({
    report: true,
    capable: true,
    pathname: '/app/health/',
    fetchResponses: {
      '/app/support/api/context': {
        status: 200,
        body: JSON.stringify({ version: '2.0.3', os: 'Linux', os_version: '7.0' }),
      },
    },
  });

  harness.window.convey.reportError({
    app: 'network',
    route: '/app/network/',
    apiError: {
      status: 500,
      message: 'Internal server error',
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  const recent = harness.document.querySelector('#report-error-recent');
  recent.value = 'User updated error description';
  recent.dispatchEvent({ type: 'input', bubbles: true });

  const send = harness.document.querySelector('a');
  assert.ok(send, 'send anchor exists');
  assert.strictEqual(send.hasAttribute('data-solstone-outside'), true);

  send.dispatchEvent({ type: 'click', bubbles: true });
  assert.ok(harness.window.location.assigned, 'location assigned by external navigation');
  assert.ok(harness.window.location.assigned.includes('User+updated+error+description') || harness.window.location.assigned.includes('User%20updated%20error%20description'));
});

asyncCase('converted workspace mounts normally', async () => {
  const workspace = '<section data-workspace="home"></section>';
  const harness = createHarness({
    boot: true,
    fetchResponses: {
      '/app/support/background': { status: 200, body: '' },
      '/app/home/workspace': { status: 200, body: workspace },
    },
  });
  let mounted = null;
  harness.document.addEventListener('workspace:mounted', (event) => { mounted = event.detail; });

  await harness.boot();

  assert.strictEqual(harness.document.querySelector('#main-content').getAttribute('tabindex'), '-1');
  assert.strictEqual(harness.document.querySelector('#main-content').innerHTML, workspace);
  assert.strictEqual(mounted.appName, 'home');
  assert.strictEqual(mounted.url, '/app/home/workspace');
  assert.strictEqual(harness.surfaceCalls.filter((call) => call.kind === 'empty').length, 0);
  assert.strictEqual(harness.surfaceCalls.filter((call) => call.kind === 'error').length, 0);
});


function aboutHarness() {
  const document = new Document();
  for (const id of ['Block', 'Copy', 'Retry', 'Feedback', 'Devices', 'DevicesFeedback', 'DevicesRetry']) {
    addStaticElement(document, 'div', {id: 'settingsAbout' + id});
  }
  const copies = [];
  const replies = new Map();
  const window = {
    apiJson: async url => { const response = replies.get(url); if (response instanceof Error) throw response; return response; },
    convey: {copyToClipboard: async text => copies.push(text)},
    logError() {},
  };
  const context = vm.createContext({document, window});
  const source = fs.readFileSync(path.join(crateDir, '../solstone-core-settings-web/assets/workspace.html'), 'utf8');
  const start = source.indexOf("let settingsAbout = '';");
  const end = source.indexOf('function switchSection', start);
  assert.ok(start > 0 && end > start, 'test exercises the production About handlers');
  vm.runInContext(source.slice(start, end), context);
  return {document, window, copies, replies, context, load: () => vm.runInContext('loadAbout()', context)};
}

asyncCase('about copies only reported facts and handles null and partial rows', async () => {
  const h = aboutHarness();
  const journal = 'journal 2.0.29 · ubuntu 24.04 · x86_64';
  h.replies.set('/api/system/about', {about: journal});
  h.replies.set('/app/network/api/clients', {clients: [
    {display_label: 'PRIVATE DEVICE', id: 'PRIVATE ID', reported: {
      app_id: 'solstone-ios', app_version: '2.0.6', platform: 'ios',
      hostname: 'PRIVATE HOST', path: '/PRIVATE/PATH', model: 'PRIVATE MODEL',
    }}, null, {reported: {platform: 'android'}},
  ]});
  await h.load();
  h.document.getElementById('settingsAboutCopy').dispatchEvent(event('click'));
  await new Promise(resolve => setImmediate(resolve));
  assert.strictEqual(h.copies[0], journal);
  const rows = h.document.getElementById('settingsAboutDevices').children;
  assert.strictEqual(rows.length, 3);
  assert.strictEqual(rows[0].children[0].textContent, 'PRIVATE DEVICE', 'positive control: identity is present in its separate orientation label');
  rows[0].children[2].dispatchEvent(event('click'));
  await new Promise(resolve => setImmediate(resolve));
  assert.strictEqual(h.copies[1], 'solstone-ios · 2.0.6 · ios');
  assert.ok(h.copies.every(text => !text.includes('PRIVATE')), 'populated forbidden inputs stay out of copy');
  assert.strictEqual(rows[1].children.length, 2, 'missing facts have no copy button');
  assert.strictEqual(rows[2].children[1].textContent, 'android');
});

asyncCase('about and devices fail independently, retry recovers, failed copy stays honest', async () => {
  const h = aboutHarness();
  h.replies.set('/api/system/about', new Error('unavailable'));
  h.replies.set('/app/network/api/clients', {clients: []});
  await h.load();
  assert.strictEqual(h.document.getElementById('settingsAboutRetry').hidden, false);
  assert.strictEqual(h.document.getElementById('settingsAboutDevicesFeedback').textContent, 'no paired devices.');
  h.replies.set('/api/system/about', {about: 'journal 2.0.29 · windows 11 26100 · x86_64'});
  h.replies.set('/app/network/api/clients', new Error('unavailable'));
  h.document.getElementById('settingsAboutRetry').dispatchEvent(event('click'));
  await new Promise(resolve => setImmediate(resolve));
  assert.strictEqual(h.document.getElementById('settingsAboutFeedback').textContent, '');
  assert.strictEqual(h.document.getElementById('settingsAboutRetry').hidden, true);
  assert.strictEqual(h.document.getElementById('settingsAboutDevicesRetry').hidden, false);
  h.window.convey.copyToClipboard = async () => {throw new Error('denied');};
  h.document.getElementById('settingsAboutCopy').dispatchEvent(event('click'));
  await new Promise(resolve => setImmediate(resolve));
  assert.ok(h.document.getElementById('settingsAboutFeedback').textContent.includes("couldn't copy"));
  h.replies.set('/app/network/api/clients', {clients: []});
  h.document.getElementById('settingsAboutDevicesRetry').dispatchEvent(event('click'));
  await new Promise(resolve => setImmediate(resolve));
  assert.strictEqual(h.document.getElementById('settingsAboutDevicesRetry').hidden, true);
});

asyncCase('a pending about read does not delay known device facts', async () => {
  const h = aboutHarness();
  let finish;
  h.replies.set('/api/system/about', new Promise(resolve => { finish = resolve; }));
  h.replies.set('/app/network/api/clients', {clients: [{reported: {app_id: 'solstone-ios', app_version: '2.0.6'}}]});
  const loading = h.load();
  await new Promise(resolve => setImmediate(resolve));
  assert.strictEqual(h.document.getElementById('settingsAboutDevices').children.length, 1);
  finish({about: 'journal 2.0.29 · ubuntu 24.04 · x86_64'});
  await loading;
  assert.strictEqual(h.document.getElementById('settingsAboutBlock').textContent, 'journal 2.0.29 · ubuntu 24.04 · x86_64');
});

asyncCase('boot proceeds after labels timeout and late response installs stream labels', async () => {
  let timerCb = null;
  let timerDelay = null;
  const fakeSetTimeout = (cb, delay) => {
    timerCb = cb;
    timerDelay = delay;
    return 999;
  };
  const fakeClearTimeout = () => {};

  let releaseLabels;
  const labelsPromise = new Promise((resolve) => {
    releaseLabels = resolve;
  });

  const harness = createHarness({
    boot: true,
    setTimeout: fakeSetTimeout,
    clearTimeout: fakeClearTimeout,
    fetchResponses: {
      '/api/streams/labels': {
        status: 200,
        json: () => labelsPromise,
        text: () => labelsPromise.then((body) => JSON.stringify(body)),
      },
      '/app/support/background': { status: 200, body: '' },
      '/app/home/workspace': { status: 200, body: '<div id="home-workspace">home</div>' },
    },
  });

  let mountCount = 0;
  const origMount = harness.window.mountWorkspaceFragment;
  harness.window.mountWorkspaceFragment = async function (...args) {
    mountCount += 1;
    return origMount.apply(this, args);
  };

  const bootPromise = harness.boot();
  await new Promise((resolve) => setImmediate(resolve));

  assert.ok(timerCb, 'race timeout timer was registered');
  assert.ok(timerDelay > 0 && timerDelay <= 1500, 'race timer delay is up to 1500ms');

  timerCb();
  await bootPromise;

  assert.strictEqual(mountCount, 1, 'workspace mounted once after timeout');
  assert.strictEqual(
    harness.logErrors.filter((entry) => entry.context && entry.context.context === 'stream-labels').length,
    0,
    'a labels load that is only slow is not an error on the page'
  );

  releaseLabels({
    status: 'complete',
    labels: {
      ja1r: 'LABEL-ONE',
    },
  });

  await new Promise((resolve) => setImmediate(resolve));

  assert.strictEqual(mountCount, 1, 'mount count is still 1 after late labels response');
  assert.strictEqual(
    harness.window.JournalFormat.stream('ja1r'),
    'LABEL-ONE',
    'JournalFormat.stream returns seeded label'
  );
});

asyncCase('settings has no devices section and its old links land on network devices', async () => {
  const source = fs.readFileSync(path.join(crateDir, '../solstone-core-settings-web/assets/workspace.html'), 'utf8');
  for (const marker of ['id="tab-devices"', 'id="section-devices"', '<option value="devices">']) {
    assert.ok(!source.includes(marker), marker);
  }
  const start = source.indexOf('function redirectRetiredSection(raw) {');
  const end = source.indexOf("window.addEventListener('hashchange'", start);
  assert.ok(start > 0 && end > start, 'test exercises the production redirect and hash entry');
  for (const [hash, expected] of [['devices', '/app/network/#devices'], ['observer', '/app/network/#devices'], ['vision', null]]) {
    const replaced = [];
    const switched = [];
    const context = vm.createContext({
      window: { location: { hash: '#' + hash, replace: (url) => replaced.push(url) } },
      VALID_SECTIONS: ['vision'],
      switchSection: (section) => switched.push(section),
    });
    vm.runInContext(source.slice(start, end), context);
    vm.runInContext('initFromHash()', context);
    assert.deepStrictEqual(replaced, expected ? [expected] : [], hash);
    assert.deepStrictEqual(switched, expected ? [] : [hash], hash);
  }
});

// JournalFormat's provenance labels, against a controllable labels endpoint.
// Seeded labels are synthetic; fallback words are compared with each other,
// never with owner copy.
function labelFormat(extra = {}) {
  const reads = [];
  const logs = [];
  const warnings = [];
  const context = {
    window: { logError: (error, ctx) => logs.push({ error, ctx }) },
    Intl, Date, Math, Number, String, Object, Array, Set, Map, Promise, JSON, Error,
    console: { warn: (message) => warnings.push(message), log() {}, error() {} },
    fetch: (url, options) => new Promise((resolve, reject) => {
      reads.push({ url, resolve, reject });
      options?.signal?.addEventListener('abort', () => reject(new Error('aborted')));
    }),
    ...extra,
  };
  vm.createContext(context);
  vm.runInContext(fs.readFileSync(path.join(crateDir, 'assets/static/date_format.js'), 'utf8'), context, { filename: 'date_format.js' });
  const answer = (body) => reads.shift().resolve({ ok: true, status: 200, json: () => Promise.resolve(body) });
  const fail = () => reads.shift().reject(new Error('offline'));
  return { format: context.window.JournalFormat, reads, logs, warnings, answer, fail };
}

const settle = () => new Promise((resolve) => setImmediate(resolve));

asyncCase('stream labels: a failed table decides only dotted names, never a device word', async () => {
  const { format, logs, fail } = labelFormat();
  const loading = format.beginStreamLabels();
  fail();
  assert.strictEqual(await loading, false);
  assert.strictEqual(logs.length, 1, 'the failure is logged');
  const unknown = format.stream('_default');
  for (const name of ['device_9e92ab54', 'macos_screen', 'pixel-7', 'fedora', 'device_omi-audio',
    'device_mac_screen', 'agent-memory-' + 'a'.repeat(64), 'device.omi.audio']) {
    assert.strictEqual(format.stream(name), unknown, name);
  }
  const terminal = format.stream('fedora.tmux');
  assert.strictEqual(format.stream('suze.tmux'), terminal);
  assert.notStrictEqual(terminal, unknown);
  const watch = format.stream('fedora.watch');
  const oura = format.stream('import.oura');
  assert.strictEqual(new Set([terminal, watch, oura, unknown]).size, 4);
  for (const name of ['fedora.tmux', 'fedora.watch', 'import.oura', 'device_9e92ab54']) {
    const label = format.stream(name);
    assert.notStrictEqual(label, name);
    assert.notStrictEqual(label, name.replace(/[._-]+/g, ' '));
  }
  assert.strictEqual(format.streamSays('import.oura').imported, true);
  assert.strictEqual(format.streamSays('fedora.tmux').imported, false);
});

asyncCase('stream labels: a loaded table wins, and a name it lacks is unknown', async () => {
  const { format, answer } = labelFormat();
  const loading = format.beginStreamLabels();
  answer({ status: 'complete', labels: { macos_screen: 'LABEL-M' }, says: { macos_screen: { what: 'screen', imported: false } } });
  assert.strictEqual(await loading, true);
  assert.strictEqual(format.stream('macos_screen'), 'LABEL-M');
  assert.strictEqual(format.streamSays('macos_screen').what, 'screen');
  assert.strictEqual(format.stream('fedora.tmux'), format.stream('_default'));
  assert.strictEqual(format.streamSays('import.oura').imported, false);
});

asyncCase('stream labels: a failed re-read falls back for the new names only, logged once', async () => {
  const { format, logs, answer, fail } = labelFormat();
  const first = format.beginStreamLabels();
  answer({ status: 'complete', labels: { macos_screen: 'LABEL-M' } });
  await first;
  const unknown = format.stream('_default');
  const tmux = format.noteLiveStream('fedora.tmux');
  const watch = format.noteLiveStream('suze.watch');
  fail();
  await Promise.all([tmux, watch]);
  assert.strictEqual(format.stream('macos_screen'), 'LABEL-M', 'shown labels stay');
  assert.notStrictEqual(format.stream('fedora.tmux'), unknown, 'first new name falls back');
  assert.notStrictEqual(format.stream('suze.watch'), unknown, 'second new name falls back');
  assert.strictEqual(format.needsStreamLabel('fedora.tmux'), false, 'one re-read per name');
  const another = format.noteLiveStream('laptop.tmux');
  fail();
  await another;
  assert.strictEqual(logs.length, 1, 'repeated failures log once');
  // A later read that answers replaces every fallback with what it says.
  const later = format.noteLiveStream('device_new');
  answer({ status: 'complete', labels: { macos_screen: 'LABEL-M', device_new: 'LABEL-N' } });
  await later;
  assert.strictEqual(format.stream('device_new'), 'LABEL-N');
  assert.strictEqual(format.stream('fedora.tmux'), unknown);
});

asyncCase('stream labels: a partial re-read adds new names but never changes a complete table', async () => {
  const { format, logs, warnings, answer } = labelFormat();
  const first = format.beginStreamLabels();
  answer({ status: 'complete', labels: { a: 'LABEL-A' } });
  await first;
  const second = format.noteLiveStream('b');
  answer({ status: 'partial', labels: { a: 'LABEL-A-PARTIAL', b: 'LABEL-B' } });
  await second;
  assert.strictEqual(format.stream('a'), 'LABEL-A');
  assert.strictEqual(format.stream('b'), 'LABEL-B');
  const third = format.noteLiveStream('c');
  answer({ status: 'partial', labels: { a: 'LABEL-A', b: 'LABEL-B' } });
  await third;
  assert.strictEqual(format.stream('c'), format.stream('_default'), 'a name the registry lacks is unknown');
  assert.strictEqual(logs.length, 0, 'a partial table is not an error on the page');
  assert.strictEqual(warnings.length, 1, 'partial is noted once');
});

asyncCase('stream labels: a name met during an earlier read gets a read of its own', async () => {
  const { format, reads, answer } = labelFormat();
  const boot = format.beginStreamLabels();
  const late = format.noteLiveStream('device_new');
  answer({ status: 'complete', labels: { macos_screen: 'LABEL-M' } });
  await boot;
  await settle();
  assert.strictEqual(reads.length, 1, 'the miss starts a fresh read');
  answer({ status: 'complete', labels: { macos_screen: 'LABEL-M', device_new: 'LABEL-N' } });
  await late;
  assert.strictEqual(format.stream('device_new'), 'LABEL-N');
});

asyncCase('stream labels: a read that hangs gives up, and the next stream reads again', async () => {
  const timers = [];
  const { format, reads, logs, warnings, answer } = labelFormat({
    AbortController,
    setTimeout: (fn) => { timers.push(fn); return timers.length; },
    clearTimeout: () => {},
  });
  const hung = format.noteLiveStream('fedora.tmux');
  assert.strictEqual(reads.length, 1);
  timers.shift()();
  await hung;
  reads.shift();
  assert.strictEqual(logs.length, 0, 'a slow read is not an error on the page');
  assert.strictEqual(warnings.length, 1);
  assert.notStrictEqual(format.stream('fedora.tmux'), format.stream('_default'), 'the waiting name falls back');
  const next = format.noteLiveStream('device_new');
  assert.strictEqual(reads.length, 1, 'a later stream starts a new read');
  answer({ status: 'complete', labels: { device_new: 'LABEL-N' } });
  await next;
  assert.strictEqual(format.stream('device_new'), 'LABEL-N');
});

asyncCase('stream labels: a dotted name with an empty side is unknown, as on the journal', async () => {
  const { format, fail } = labelFormat();
  const loading = format.beginStreamLabels();
  fail();
  await loading;
  for (const name of ['a..tmux', 'x.tmux.', '.tmux', 'import.']) {
    assert.strictEqual(format.stream(name), format.stream('_default'), name);
  }
  assert.strictEqual(format.streamSays('import.').imported, false);
});

asyncCase('stream labels: concurrent misses share reads, and a joiner the read lacks reads once more', async () => {
  const { format, reads, answer } = labelFormat();
  const a = format.noteLiveStream('x.tmux');
  const b = format.noteLiveStream('y.tmux');
  const c = format.noteLiveStream('z.tmux');
  assert.strictEqual(reads.length, 1, 'misses share the read in flight');
  answer({ status: 'complete', labels: { 'x.tmux': 'LABEL-X' } });
  await a;
  await settle();
  assert.strictEqual(reads.length, 1, 'the joiners share one follow-up read');
  answer({ status: 'complete', labels: { 'x.tmux': 'LABEL-X', 'y.tmux': 'LABEL-Y' } });
  await Promise.all([b, c]);
  assert.strictEqual(format.stream('x.tmux'), 'LABEL-X');
  assert.strictEqual(format.stream('y.tmux'), 'LABEL-Y');
  assert.strictEqual(format.stream('z.tmux'), format.stream('_default'));
  assert.strictEqual(format.needsStreamLabel('z.tmux'), false);
});

async function runAsyncCases() {
  for (const runCase of asyncCases) {
    await runCase();
  }
  console.log('DOM CASES: ' + cases + ' passed');
}

runAsyncCases().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
