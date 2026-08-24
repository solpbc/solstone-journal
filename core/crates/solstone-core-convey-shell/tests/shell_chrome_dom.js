// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');

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
    child.parentElement = this;
    this.children.push(child);
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
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    if (event.bubbles && !event.cancelBubble && this.parentElement) this.parentElement.dispatchEvent(event);
    return !event.defaultPrevented;
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

function shellFixture() {
  const groups = [
    ['your_journal', [
      ['home', 'home', 'primary', 0, false],
      ['timeline', 'timeline', 'primary', 1, false],
      ['transcripts', 'transcripts', null, 0, false],
      ['speakers', 'speakers', null, 0, false],
      ['body', 'body', null, 0, false],
      ['news', 'newsletters', null, 0, false],
    ]],
    ['understand', [
      ['search', 'search', 'primary', 2, false],
      ['entities', 'entities', 'primary', 3, true],
      ['thinking', 'thinking', 'primary', 4, false],
      ['stats', 'stats', null, 0, false],
      ['curation', 'curation', null, 0, false],
      ['activities', 'activities', null, 0, false],
    ]],
    ['manage', [
      ['import', 'import', 'management', 0, true],
      ['network', 'network', null, 0, false],
      ['backup', 'backup', null, 0, false],
      ['health', 'health', null, 0, false],
      ['support', 'support', null, 0, false],
      ['settings', 'settings', 'management', 1, true],
    ]],
  ];
  const apps = groups.flatMap(([launcherGroup, rows]) => rows.map(([name, label, railGroup, railRank, facetsEnabled], launcherRank) => ({
    app_bar: '',
    background_url: null,
    date_nav: false,
    facets_enabled: facetsEnabled,
    icon: name,
    icon_svg: '',
    label,
    launcher_group: launcherGroup,
    launcher_rank: launcherRank,
    name,
    rail_group: railGroup,
    rail_rank: railRank,
    workspace_url: '/app/' + name + '/',
  })));
  return { apps };
}

function createHarness() {
  const document = new Document();
  addStaticElement(document, 'nav', { id: 'app-rail', class: 'app-rail' });
  addStaticElement(document, 'nav', { id: 'app-dock', class: 'app-dock' });
  const launcher = addStaticElement(document, 'div', {
    id: 'app-launcher',
    role: 'dialog',
    'aria-modal': 'true',
    hidden: '',
    inert: '',
  });
  launcher.style.position = 'fixed';
  addStaticElement(document, 'div', { id: 'status-instrument' });
  addStaticElement(document, 'div', { id: 'status-pane', class: 'status-pane' });
  addStaticElement(document, 'nav', { id: 'facet-strip', class: 'facet-bar', hidden: '' });
  addStaticElement(document, 'main', { id: 'main-content' });

  const windowListeners = {};
  const window = {
    document,
    Element,
    MutationObserver: MutationObserverShim,
    setTimeout,
    clearTimeout,
    requestAnimationFrame(callback) { callback(); },
    getComputedStyle(element) {
      return {
        display: element.hidden || element.style.display === 'none' ? 'none' : 'block',
        position: element.style.position || 'static',
      };
    },
    sessionStorage: { getItem() { return null; }, setItem() {}, removeItem() {} },
    URL,
    location: { href: 'http://localhost/app/home/', pathname: '/app/home/' },
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
    whenShellReady(callback) { callback(shellFixture()); },
    resolveSolShellReady() {},
    fetch() { return Promise.reject(new Error('boot is replaced for DOM harness')); },
  };
  window.window = window;
  const context = vm.createContext({
    window,
    document,
    Element,
    MutationObserver: MutationObserverShim,
    console,
    URL,
    setTimeout,
    clearTimeout,
    requestAnimationFrame: window.requestAnimationFrame,
    getComputedStyle: window.getComputedStyle,
    CustomEvent: window.CustomEvent,
  });
  const read = (file) => fs.readFileSync(path.join(crateDir, 'assets/static', file), 'utf8');
  vm.runInContext(read('modal_layer.js'), context, { filename: 'modal_layer.js' });
  vm.runInContext(read('presentation_mode.js'), context, { filename: 'presentation_mode.js' });
  const bootSource = read('shell_boot.js').replace(
    /\s*boot\(\);\s*\}\)\(\);\s*$/,
    '\n  window.__shellChrome = { renderAppRail, renderAppDock, renderAppLauncher, renderFacetStrip, renderStatusInstrument, installLauncherInteractions };\n})();\n',
  );
  vm.runInContext(bootSource, context, { filename: 'shell_boot.js' });
  return { document, window, chrome: window.__shellChrome, shell: shellFixture() };
}

function renderChrome(harness, currentAppName) {
  const app = harness.shell.apps.find((candidate) => candidate.name === currentAppName);
  assert.ok(app, 'fixture app ' + currentAppName + ' exists');
  harness.chrome.renderAppRail(harness.shell, currentAppName);
  harness.chrome.renderAppDock(harness.shell, currentAppName);
  harness.chrome.renderAppLauncher(harness.shell, currentAppName);
  harness.chrome.renderFacetStrip(harness.shell, app);
  harness.chrome.renderStatusInstrument();
  harness.chrome.installLauncherInteractions();
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

testCase('rail composition', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const rail = harness.document.querySelector('#app-rail');
  assert.strictEqual(rail.children.length, 10);
  assert.ok(rail.children[0].hasAttribute('data-app-launcher-toggle'));
  assert.deepStrictEqual(appNames(rail.children.slice(1, 6)), ['home', 'timeline', 'search', 'entities', 'thinking']);
  assert.ok(rail.children[6].classList.contains('app-rail-spacer'));
  assert.ok(rail.children[7].classList.contains('app-rail-divider'));
  assert.deepStrictEqual(appNames(rail.children.slice(8)), ['import', 'settings']);
});

testCase('launcher completeness and order', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const groups = harness.document.querySelectorAll('[data-launcher-group]');
  assert.deepStrictEqual(groups.map((group) => group.getAttribute('data-launcher-group')), ['your_journal', 'understand', 'manage']);
  assert.deepStrictEqual(appNames(groups[0].querySelectorAll('[data-launcher-app]')), ['home', 'timeline', 'transcripts', 'speakers', 'body', 'news']);
  assert.deepStrictEqual(appNames(groups[1].querySelectorAll('[data-launcher-app]')), ['search', 'entities', 'thinking', 'stats', 'curation', 'activities']);
  assert.deepStrictEqual(appNames(groups[2].querySelectorAll('[data-launcher-app]')), ['import', 'network', 'backup', 'health', 'support', 'settings']);
  const allApps = harness.document.querySelectorAll('[data-launcher-app]');
  assert.strictEqual(allApps.length, 18);
  assert.strictEqual(new Set(appNames(allApps)).size, 18);
});

testCase('dock composition', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const dock = harness.document.querySelector('#app-dock');
  assert.strictEqual(dock.children.length, 4);
  assert.deepStrictEqual(appNames(dock.children.slice(0, 3)), ['home', 'timeline', 'search']);
  assert.ok(dock.children[3].hasAttribute('data-app-launcher-toggle'));
});

testCase('launcher toggle accessible name and current treatment', () => {
  const unpinned = createHarness();
  renderChrome(unpinned, 'curation');
  const unpinnedToggle = unpinned.document.querySelector('#app-rail [data-app-launcher-toggle]');
  assert.strictEqual(unpinnedToggle.getAttribute('aria-label'), 'journal apps, current: curation');
  assert.ok(unpinnedToggle.classList.contains('is-current'));

  const pinned = createHarness();
  renderChrome(pinned, 'home');
  const pinnedToggle = pinned.document.querySelector('#app-rail [data-app-launcher-toggle]');
  assert.ok(!pinnedToggle.classList.contains('is-current'));
  assert.ok(pinned.document.querySelector('#app-rail [data-app-name="home"]').classList.contains('is-current'));
});

testCase('launcher open and close lifecycle', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const launcher = harness.document.querySelector('#app-launcher');
  const toggle = harness.document.querySelector('#app-rail [data-app-launcher-toggle]');
  toggle.dispatchEvent(event('click'));
  assert.ok(!launcher.hidden && !launcher.inert);
  harness.document.dispatchEvent(event('keydown', { key: 'Escape' }));
  assert.ok(launcher.hidden && launcher.inert);
  toggle.dispatchEvent(event('click'));
  launcher.dispatchEvent(event('click', { target: launcher }));
  assert.ok(launcher.hidden && launcher.inert);
  toggle.dispatchEvent(event('click'));
  launcher.querySelector('[data-launcher-app] a').dispatchEvent(event('click', { bubbles: true }));
  assert.ok(launcher.hidden && launcher.inert);
});

testCase('launcher remains inert while closed', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const launcher = harness.document.querySelector('#app-launcher');
  const toggle = harness.document.querySelector('#app-rail [data-app-launcher-toggle]');
  assert.ok(launcher.inert);
  toggle.dispatchEvent(event('click'));
  assert.ok(!launcher.inert);
  harness.document.dispatchEvent(event('keydown', { key: 'Escape' }));
  assert.ok(launcher.inert);
});

testCase('launcher no-match filter state', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const input = harness.document.querySelector('#app-launcher-filter');
  input.value = 'nothing matches';
  input.dispatchEvent(event('input'));
  const empty = harness.document.querySelector('.app-launcher-empty');
  assert.ok(!empty.hidden);
  assert.ok(harness.document.querySelectorAll('[data-launcher-app]:not([hidden])').length === 0);
});

testCase('presentation mode closes launcher', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const launcher = harness.document.querySelector('#app-launcher');
  harness.document.querySelector('#app-rail [data-app-launcher-toggle]').dispatchEvent(event('click'));
  assert.ok(!launcher.hidden);
  harness.window.solstonePresentation.on();
  assert.ok(launcher.hidden && launcher.inert);
});

testCase('status instrument target', () => {
  const harness = createHarness();
  renderChrome(harness, 'home');
  const icon = harness.document.querySelector('#status-instrument .status-icon');
  assert.ok(icon);
  assert.strictEqual(icon.getAttribute('aria-controls'), 'status-pane');
});

testCase('facet strip conditional visibility', () => {
  const facetsEnabled = createHarness();
  renderChrome(facetsEnabled, 'entities');
  assert.ok(!facetsEnabled.document.querySelector('#facet-strip').hidden);

  const facetsDisabled = createHarness();
  renderChrome(facetsDisabled, 'home');
  assert.ok(facetsDisabled.document.querySelector('#facet-strip').hidden);
});

console.log('DOM CASES: ' + cases + ' passed');
