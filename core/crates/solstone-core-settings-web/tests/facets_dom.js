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
    if (this.values.size) this.element.attributes.class = Array.from(this.values).join(' ');
    else delete this.element.attributes.class;
  }
  add(...names) { names.forEach((name) => this.values.add(name)); this.sync(); }
  remove(...names) { names.forEach((name) => this.values.delete(name)); this.sync(); }
  toggle(name, force) {
    const present = force === undefined ? !this.values.has(name) : force;
    if (present) this.values.add(name); else this.values.delete(name);
    this.sync();
    return present;
  }
  contains(name) { return this.values.has(name); }
}

function dataKey(attribute) {
  return attribute.slice(5).replace(/-([a-z])/g, (_, character) => character.toUpperCase());
}

function parseAttributes(source, element) {
  const pattern = /([^\s=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g;
  let match;
  while ((match = pattern.exec(source))) {
    element.setAttribute(match[1], match[2] ?? match[3] ?? match[4] ?? '');
  }
}

function appendText(element, text) {
  const value = text.replace(/\s+/g, ' ').trim();
  if (!value) return;
  element._ownText = (element._ownText ? element._ownText + ' ' : '') + value;
}

function parseHtml(html, root) {
  root.replaceChildren();
  root._ownText = '';
  const stack = [root];
  const tags = /<\/?[^>]+>/g;
  const voidTags = new Set(['AREA', 'BASE', 'BR', 'COL', 'EMBED', 'HR', 'IMG', 'INPUT', 'LINK', 'META', 'OPTION', 'SOURCE']);
  let previous = 0;
  let match;
  while ((match = tags.exec(html))) {
    appendText(stack[stack.length - 1], html.slice(previous, match.index));
    previous = tags.lastIndex;
    const token = match[0];
    if (token.startsWith('<!--') || token.startsWith('<!')) continue;
    if (token.startsWith('</')) {
      const index = stack.map((element) => element.tagName).lastIndexOf(token.slice(2, -1).trim().toUpperCase());
      if (index > 0) stack.length = index;
      continue;
    }
    const parts = /^<\s*([^\s/>]+)([\s\S]*?)\/?>$/.exec(token);
    if (!parts) continue;
    const child = root.ownerDocument.createElement(parts[1]);
    parseAttributes(parts[2], child);
    stack[stack.length - 1].appendChild(child);
    if (!token.endsWith('/>') && !voidTags.has(child.tagName)) stack.push(child);
  }
  appendText(stack[stack.length - 1], html.slice(previous));
}

function selectorMatches(element, selector) {
  selector = selector.trim();
  let notMatch;
  while ((notMatch = selector.match(/:not\(([^)]+)\)/))) {
    if (selectorMatches(element, notMatch[1])) return false;
    selector = selector.slice(0, notMatch.index) + selector.slice(notMatch.index + notMatch[0].length);
  }
  const attributes = [...selector.matchAll(/\[([^\]=]+)(?:=["']?([^\]"']+)["']?)?\]/g)];
  selector = selector.replace(/\[[^\]]+\]/g, '');
  const idMatch = selector.match(/#([\w-]+)/);
  const classMatches = [...selector.matchAll(/\.([\w-]+)/g)];
  const tagMatch = selector.match(/^[a-zA-Z][\w-]*/);
  if (tagMatch && element.tagName !== tagMatch[0].toUpperCase()) return false;
  if (idMatch && element.id !== idMatch[1]) return false;
  if (classMatches.some((match) => !element.classList.contains(match[1]))) return false;
  return attributes.every((match) => element.hasAttribute(match[1])
    && (match[2] === undefined || element.getAttribute(match[1]) === match[2]));
}

function matchesSelector(element, selector) {
  return selector.split(',').some((part) => selectorMatches(element, part));
}

function queryAll(root, selector) {
  if (selector.includes(',')) {
    return selector.split(',').flatMap((part) => queryAll(root, part));
  }
  let candidates = [root];
  for (const piece of selector.trim().split(/\s+/)) {
    const next = [];
    for (const candidate of candidates) {
      const visit = (element) => element.children.forEach((child) => {
        if (matchesSelector(child, piece)) next.push(child);
        visit(child);
      });
      visit(candidate);
    }
    candidates = next;
  }
  return candidates;
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
    this.checked = false;
    this.disabled = false;
    this._ownText = '';
    this._innerHTML = '';
  }
  get id() { return this.getAttribute('id') || ''; }
  set id(value) { this.setAttribute('id', value); }
  get className() { return this.getAttribute('class') || ''; }
  set className(value) { this.setAttribute('class', value); }
  get href() { return this.getAttribute('href') || ''; }
  set href(value) { this.setAttribute('href', value); }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(value) { if (value) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  get parentNode() { return this.parentElement; }
  get firstChild() { return this.children[0] || null; }
  get nextSibling() {
    if (!this.parentElement) return null;
    return this.parentElement.children[this.parentElement.children.indexOf(this) + 1] || null;
  }
  get innerHTML() { return this._innerHTML; }
  set innerHTML(value) { this._innerHTML = String(value); parseHtml(this._innerHTML, this); }
  get textContent() {
    return [this._ownText, ...this.children.map((child) => child.textContent)].filter(Boolean).join(' ');
  }
  set textContent(value) { this._ownText = String(value); this.children = []; this._innerHTML = ''; }
  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'class') this.classList.setFromString(value);
    if (name.startsWith('data-')) this.dataset[dataKey(name)] = String(value);
  }
  getAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null; }
  hasAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attributes, name); }
  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'class') this.classList.setFromString('');
    if (name.startsWith('data-')) delete this.dataset[dataKey(name)];
  }
  appendChild(child) { child.parentElement = this; this.children.push(child); return child; }
  append(...items) {
    items.forEach((item) => item instanceof Element ? this.appendChild(item) : appendText(this, String(item)));
  }
  replaceChildren(...items) {
    this.children.forEach((child) => { child.parentElement = null; });
    this.children = [];
    this._ownText = '';
    this._innerHTML = '';
    this.append(...items);
  }
  removeChild(child) {
    this.children = this.children.filter((item) => item !== child);
    child.parentElement = null;
  }
  remove() { if (this.parentElement) this.parentElement.removeChild(this); }
  insertBefore(child, reference) {
    child.parentElement = this;
    const index = this.children.indexOf(reference);
    if (index < 0) this.children.push(child); else this.children.splice(index, 0, child);
    return child;
  }
  matches(selector) { return matchesSelector(this, selector); }
  closest(selector) {
    let current = this;
    while (current) {
      if (current.matches(selector)) return current;
      current = current.parentElement;
    }
    return null;
  }
  querySelector(selector) { return queryAll(this, selector)[0] || null; }
  querySelectorAll(selector) { return queryAll(this, selector); }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    const propertyHandler = this['on' + event.type];
    if (typeof propertyHandler === 'function') propertyHandler.call(this, event);
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    if (event.bubbles && !event.cancelBubble && this.parentElement) this.parentElement.dispatchEvent(event);
    return !event.defaultPrevented;
  }
  focus() { this.ownerDocument.activeElement = this; }
  select() {}
}

class Document {
  constructor() {
    this.listeners = {};
    this.documentElement = new Element('html', this);
    this.body = new Element('body', this);
    this.documentElement.appendChild(this.body);
    this.activeElement = this.body;
    this.readyState = 'loading';
    this.cookieWrites = [];
  }
  get cookie() { return ''; }
  set cookie(value) { this.cookieWrites.push(String(value)); }
  createElement(tagName) { return new Element(tagName, this); }
  getElementById(id) { return this.querySelector('#' + id); }
  querySelector(selector) {
    if (selector === 'body') return this.body;
    if (selector === 'html') return this.documentElement;
    return this.documentElement.querySelector(selector);
  }
  querySelectorAll(selector) { return this.documentElement.querySelectorAll(selector); }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
}

function event(type, options = {}) {
  return {
    type,
    bubbles: Boolean(options.bubbles),
    key: options.key,
    target: options.target,
    defaultPrevented: false,
    cancelBubble: false,
    preventDefault() { this.defaultPrevented = true; },
    stopPropagation() { this.cancelBubble = true; },
  };
}

function response(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, async json() { return body; } };
}

function slugify(title) {
  return title.toLowerCase().trim().replace(/[^a-z0-9]+/g, '-').replace(/(^-|-$)/g, '');
}

function initialFacets() {
  return new Map([
    ['work-life', { title: 'Work Life', description: 'work notes', color: '#334455', emoji: '💼', icon: '', muted: false, path: 'facets/work-life' }],
    ['zeta-project', { title: 'Zeta Project', description: 'zeta notes', color: '#334455', emoji: '🧭', icon: '', muted: false, path: 'facets/zeta-project' }],
    ['muted-thing', { title: 'Muted Thing', description: 'muted notes', color: '#334455', emoji: '🔕', icon: '', muted: true, path: 'facets/muted-thing' }],
  ]);
}

function publicFacet(name, config) {
  return { name, title: config.title, color: config.color, emoji: config.emoji, icon: config.icon, icon_svg: '', muted: Boolean(config.muted) };
}

function createHarness() {
  const document = new Document();
  const workspace = fs.readFileSync(path.join(crateDir, 'assets', 'workspace.html'), 'utf8');
  const scriptMatch = workspace.match(/<script>([\s\S]*)<\/script>\s*$/);
  assert.ok(scriptMatch, 'workspace contains one executable inline script');
  parseHtml(workspace.slice(0, scriptMatch.index), document.body);

  const requests = [];
  const facets = initialFacets();
  const windowListeners = {};
  const historyEntries = [''];
  let historyIndex = 0;
  const window = {
    document,
    Element,
    AppServices: { escapeHtml(value) { return String(value); } },
    SurfaceState: { loading() { return ''; }, error() { return ''; } },
    location: { href: '/app/settings/', pathname: '/app/settings/', hash: '' },
    history: {
      pushState(_state, _title, next) {
        historyEntries.splice(historyIndex + 1);
        historyEntries.push(String(next));
        historyIndex = historyEntries.length - 1;
        window.location.hash = historyEntries[historyIndex];
      },
      back() {
        if (historyIndex === 0) return;
        historyIndex -= 1;
        window.location.hash = historyEntries[historyIndex];
        for (const listener of windowListeners.hashchange || []) listener(event('hashchange'));
      },
    },
    setTimeout,
    clearTimeout,
    requestAnimationFrame(callback) { callback(); },
    addEventListener(type, listener) { (windowListeners[type] ||= []).push(listener); },
    fetch: null,
    apiJson: null,
  };

  async function fetchMock(url, options = {}) {
    const requestUrl = String(url);
    const method = String(options.method || 'GET').toUpperCase();
    const body = options.body ? JSON.parse(options.body) : null;
    requests.push({ url: requestUrl, method, body });
    if (method === 'GET' && requestUrl === '/app/settings/api/state') return response({ settings_copy: {}, install_copy: {} });
    if (method === 'GET' && requestUrl === '/app/settings/api/facets?all=true') {
      return response({ facets: [...facets.entries()].map(([name, config]) => publicFacet(name, config)) });
    }
    if (method === 'POST' && requestUrl === '/app/settings/api/facet') {
      const facet = slugify(body.title);
      if (!facet || facets.has(facet)) return response({ error: 'facet already exists' }, 400);
      const config = { title: body.title, description: '', color: body.color, emoji: '📦', icon: '', muted: false, path: 'facets/' + facet };
      facets.set(facet, config);
      return response({ success: true, facet, config }, 201);
    }
    const activities = requestUrl.match(/^\/app\/settings\/api\/facet\/([^/]+)\/activities(?:\/[^/]+)?$/);
    if (method === 'GET' && activities) return response({ attached: [], defaults: [] });
    const logs = requestUrl.match(/^\/app\/settings\/api\/facet\/([^/]+)\/logs(?:\?.*)?$/);
    if (method === 'GET' && logs) return response({ day: '', entries: [], next_cursor: null });
    const match = requestUrl.match(/^\/app\/settings\/api\/facet\/([^/?]+)$/);
    if (match) {
      const facet = decodeURIComponent(match[1]);
      const config = facets.get(facet);
      if (!config) return response({ error: 'facet not found' }, 404);
      if (method === 'GET') return response({ facet, config: { ...config } });
      if (method === 'PUT') {
        Object.assign(config, body);
        return response({ success: true, facet, config: { ...config } });
      }
    }
    return response({ error: 'unexpected request ' + method + ' ' + requestUrl }, 404);
  }

  window.fetch = fetchMock;
  window.apiJson = async (url, options) => {
    const result = await fetchMock(url, options);
    const body = await result.json();
    if (!result.ok) throw new Error(body.error || 'request failed');
    return body;
  };
  window.window = window;
  const context = vm.createContext({
    window, document, Element, console, URL, fetch: fetchMock, setTimeout, clearTimeout,
    history: window.history,
    requestAnimationFrame: window.requestAnimationFrame,
  });
  vm.runInContext(fs.readFileSync(path.join(crateDir, 'assets', 'settings.js'), 'utf8'), context, { filename: 'settings.js' });
  vm.runInContext(scriptMatch[1], context, { filename: 'workspace.html' });
  return { context, document, facets, requests, window, windowListeners };
}

async function run(harness, expression) {
  return await vm.runInContext(expression, harness.context);
}

async function settle() {
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
}

async function click(element) {
  element.dispatchEvent(event('click', { bubbles: true }));
  await settle();
}

let cases = 0;
async function testCase(name, fn) {
  try {
    await fn();
    cases += 1;
  } catch (error) {
    error.message = name + ': ' + error.message;
    throw error;
  }
}

(async () => {
  await testCase('AC1 unified list and empty state', async () => {
    const harness = createHarness();
    await run(harness, 'loadFacetsList()');
    const rows = harness.document.querySelectorAll('#facetsList .facet-list-row');
    assert.strictEqual(rows.length, 3);
    for (const [slug, config] of harness.facets) {
      const row = rows.find((candidate) => candidate.getAttribute('href') === '/app/settings/facets/' + slug);
      assert.ok(row, 'row exists for ' + slug);
      assert.strictEqual(row.querySelector('.facet-list-swatch').style.backgroundColor, config.color);
      assert.strictEqual(Boolean(row.querySelector('.facet-list-muted')), config.muted);
      assert.ok(row.querySelector('.facet-list-chevron'));
    }
    harness.facets.clear();
    await run(harness, 'loadFacetsList()');
    const empty = harness.document.querySelector('#facetsList .facets-list-empty');
    assert.ok(empty);
    assert.ok(empty.textContent.includes('without needing two journals'));
    assert.strictEqual(harness.document.querySelectorAll('#facetsList .facets-list-empty button').length, 1);
  });

  await testCase('settings tabs preserve browser history', async () => {
    const harness = createHarness();
    const guide = harness.document.querySelector('.settings-nav-item[data-section="guide"]');
    const facets = harness.document.querySelector('.settings-nav-item[data-section="facets"]');
    await click(facets);
    assert.strictEqual(harness.window.location.hash, '#facets');
    assert.strictEqual(facets.getAttribute('aria-selected'), 'true');
    await click(facets);
    harness.window.history.back();
    await settle();
    assert.strictEqual(harness.window.location.hash, '');
    assert.strictEqual(guide.getAttribute('aria-selected'), 'true');
    await click(facets);
    await click(guide);
    assert.strictEqual(harness.window.location.hash, '#guide');
    harness.window.history.back();
    await settle();
    assert.strictEqual(harness.window.location.hash, '#facets');
    assert.strictEqual(facets.getAttribute('aria-selected'), 'true');
    harness.window.history.back();
    await settle();
    assert.strictEqual(harness.window.location.hash, '');
    assert.strictEqual(guide.getAttribute('aria-selected'), 'true');
  });

  await testCase('AC2 rendered row drives detail bootstrap and lazy tabs', async () => {
    const harness = createHarness();
    await run(harness, 'loadFacetsList()');
    const route = harness.document.querySelector('#facetsList .facet-list-row').getAttribute('href');
    const slug = route.split('/').filter(Boolean).pop();
    harness.window.location.pathname = route;
    harness.window.location.href = route;
    await run(harness, 'initSettingsWorkspace()');
    assert.ok(harness.document.getElementById('facetDetailTabAppearance'));
    assert.ok(harness.document.getElementById('facetDetailTabActivities'));
    assert.ok(harness.document.getElementById('facetDetailTabActivity'));
    assert.ok(harness.document.querySelector('label[for="facetTitleInput"]').textContent);
    assert.ok(harness.document.querySelector('label[for="facetDescInput"]').textContent);
    harness.document.getElementById('facetDetailTabAppearance').dispatchEvent(event('keydown', { key: 'ArrowRight' }));
    await settle();
    assert.strictEqual(harness.document.getElementById('facetDetailTabActivities').getAttribute('aria-selected'), 'true');
    assert.strictEqual(harness.document.activeElement.id, 'facetDetailTabActivities');
    await click(harness.document.getElementById('facetDetailTabActivity'));
    assert.ok(harness.requests.some((request) => request.url === '/app/settings/api/facet/' + slug + '/activities'));
    assert.ok(harness.requests.some((request) => request.url === '/app/settings/api/facet/' + slug + '/logs'));
  });

  await testCase('AC3 sidebar persists while retired facet-gated markup stays absent', async () => {
    const harness = createHarness();
    assert.strictEqual(harness.document.querySelectorAll('#facetNavGroup').length, 0);
    assert.strictEqual(harness.document.querySelectorAll('#navSelectFacetGroup').length, 0);
    assert.strictEqual(harness.document.querySelectorAll('[data-requires-facet]').length, 0);
    await run(harness, 'loadFacetsList()');
    assert.strictEqual(harness.document.querySelectorAll('#facetNavGroup, #navSelectFacetGroup, [data-requires-facet]').length, 0);
    const mobileBack = harness.document.querySelector('#section-facets .facet-mobile-back');
    assert.ok(mobileBack);
    assert.strictEqual(mobileBack.getAttribute('href'), '/app/settings#guide');
    harness.window.location.pathname = '/app/settings/facets/work-life';
    await run(harness, 'initSettingsWorkspace()');
    const settings = harness.document.getElementById('settings-index-view');
    assert.strictEqual(settings.hidden, false);
    assert.strictEqual(harness.document.getElementById('settings-index-content').hidden, true);
    assert.strictEqual(harness.document.getElementById('settings-facet-detail-view').hidden, false);
    assert.ok(settings.querySelector('#settingsNav'));
    assert.ok(settings.querySelector('#settings-facet-detail-view'));
  });

  await testCase('AC4 add flow preserves typed title and selected color', async () => {
    const harness = createHarness();
    await run(harness, 'loadFacetsList()');
    await click(harness.document.querySelector('#facetsList .facets-list-action'));
    assert.strictEqual(harness.document.activeElement.id, 'facetCreateName');
    assert.ok(harness.document.getElementById('facetCreateForm'));
    assert.ok(harness.document.getElementById('facetCreateCancel'));
    await click(harness.document.getElementById('facetCreateCancel'));
    assert.strictEqual(harness.document.getElementById('facetCreateModal').style.display, 'none');
    await run(harness, 'openFacetCreateModal()');
    const title = 'Reading Retreat';
    const name = harness.document.getElementById('facetCreateName');
    const swatch = harness.document.querySelectorAll('#facetCreateColors .color-swatch')[4];
    name.value = title;
    await click(swatch);
    const color = swatch.dataset.color;
    harness.document.getElementById('facetCreateForm').dispatchEvent(event('submit', { bubbles: true }));
    await settle();
    const request = harness.requests.find((candidate) => candidate.method === 'POST' && candidate.url === '/app/settings/api/facet');
    assert.deepStrictEqual(request.body, { title, color });
    assert.strictEqual(harness.facets.get('reading-retreat').title, title);
    assert.strictEqual(harness.facets.get('reading-retreat').color, color);
  });

  await testCase('AC5 no selected-facet cookie write on exercised paths', async () => {
    const harness = createHarness();
    await run(harness, 'loadFacetsList()');
    const route = harness.document.querySelector('#facetsList .facet-list-row').getAttribute('href');
    harness.window.location.pathname = route;
    await run(harness, 'initSettingsWorkspace()');
    await run(harness, 'openFacetCreateModal()');
    harness.document.getElementById('facetCreateName').value = 'Cookie Check';
    await click(harness.document.querySelector('#facetCreateColors .color-swatch'));
    harness.document.getElementById('facetCreateForm').dispatchEvent(event('submit', { bubbles: true }));
    await settle();
    await click(harness.document.getElementById('facetMuteAction'));
    assert.ok(!harness.document.cookieWrites.some((value) => value.includes('selectedFacet=')));
  });

  await testCase('AC6 no shell facet event or globals are required', async () => {
    const harness = createHarness();
    assert.strictEqual(harness.window.selectedFacet, undefined);
    assert.strictEqual(harness.window.facetsData, undefined);
    assert.strictEqual((harness.windowListeners['facet.switch'] || []).length, 0);
    assert.strictEqual((harness.document.listeners['facet.switch'] || []).length, 0);
    await run(harness, 'loadFacetsList()');
    const route = harness.document.querySelector('#facetsList .facet-list-row').getAttribute('href');
    harness.window.location.pathname = route;
    await run(harness, 'initSettingsWorkspace()');
    assert.strictEqual(harness.window.selectedFacet, undefined);
    assert.strictEqual(harness.window.facetsData, undefined);
  });

  await testCase('AC7 retired setup is absent for populated and empty lists', async () => {
    const harness = createHarness();
    await run(harness, 'loadFacetsList()');
    assert.strictEqual(Boolean(harness.document.getElementById('setupSection')), false);
    assert.strictEqual(Boolean(harness.document.getElementById('createPersonalBtn')), false);
    harness.facets.clear();
    await run(harness, 'loadFacetsList()');
    assert.strictEqual(Boolean(harness.document.getElementById('setupSection')), false);
    assert.strictEqual(Boolean(harness.document.getElementById('createPersonalBtn')), false);
  });

  await testCase('AC8 mute round trip refreshes detail and list state', async () => {
    const harness = createHarness();
    await run(harness, 'loadFacetsList()');
    const route = harness.document.querySelector('#facetsList .facet-list-row').getAttribute('href');
    harness.window.location.pathname = route;
    await run(harness, 'initSettingsWorkspace()');
    const action = harness.document.getElementById('facetMuteAction');
    assert.strictEqual(action.textContent, 'mute');
    await click(action);
    assert.strictEqual(action.textContent, 'unmute');
    assert.strictEqual(harness.facets.get('work-life').muted, true);
    await click(action);
    assert.strictEqual(action.textContent, 'mute');
    assert.strictEqual(harness.facets.get('work-life').muted, false);
    await run(harness, 'loadFacetsList()');
    const workRow = harness.document.querySelector(
      '#facetsList .facet-list-row[href="/app/settings/facets/work-life"]'
    );
    assert.ok(workRow, 'work-life row remains present after unmuting');
    assert.strictEqual(Boolean(workRow.querySelector('.facet-list-muted')), false);
  });

  await testCase('G3-01 unconditional locality footer is removed', async () => {
    const harness = createHarness();
    assert.ok(
      !harness.document.body.textContent.includes('nothing leaves unless you send it'),
      'settings no longer renders the static custody claim — that belongs to backup/network, not settings'
    );
  });

  await testCase('G3-19 notifications guide row opens health, facets moves out of help', async () => {
    const harness = createHarness();
    const notifRow = harness.document
      .querySelectorAll('.sapp')
      .find((row) => row.querySelector('.sapp-title')?.textContent === 'notifications');
    assert.ok(notifRow, 'notifications guide row exists');
    assert.strictEqual(notifRow.tagName, 'A', 'row is a link like its sibling guide rows');
    assert.strictEqual(notifRow.getAttribute('href'), '/app/health/#quiet-notifs-section');
    assert.ok(notifRow.querySelector('.sapp-open'), 'row carries the same "open ›" affordance as its siblings');
    assert.strictEqual(
      harness.document.querySelectorAll('.sapp-muted, .sapp-tag').length,
      0,
      'the dead "built in" tag/muted row is gone'
    );

    const facetsLabel = harness.document
      .getElementById('tab-facets')
      .closest('.settings-nav-group')
      .querySelector('.settings-nav-label');
    assert.strictEqual(facetsLabel.textContent, 'data', 'desktop nav files facets under data, not help');

    const facetsOption = harness.document.querySelector('option[value="facets"]');
    assert.strictEqual(
      facetsOption.parentElement.getAttribute('label'),
      'data',
      'mobile nav select files facets under data, not help'
    );
  });

  await testCase('G3-107 notifications card describes the log it links to, outside the "set it up" promise', async () => {
    const harness = createHarness();
    const guideSection = harness.document.getElementById('section-guide');
    const notifRow = guideSection
      .querySelectorAll('.sapp')
      .find((row) => row.querySelector('.sapp-title')?.textContent === 'notifications');
    assert.ok(notifRow, 'notifications guide row still exists');
    assert.strictEqual(
      notifRow.getAttribute('href'),
      '/app/health/#quiet-notifs-section',
      'the anchor is unchanged'
    );
    const notifDesc = notifRow.querySelector('.sapp-desc').textContent;
    assert.ok(
      !notifDesc.includes('how and when notifications reach you on any device'),
      'the old "set it up" promise text is gone from the card'
    );
    assert.ok(
      notifDesc.includes("errors from background services that weren't shown as notifications"),
      'the card describes exactly what the health destination holds — service errors, not notification preferences'
    );

    const sectionDescs = guideSection.querySelectorAll('.settings-section-desc');
    assert.strictEqual(sectionDescs.length, 2, 'a second intro paragraph separates the notifications row from the setup list');
    assert.strictEqual(
      sectionDescs[0].textContent,
      'apps that have their own settings. open one to set it up or change how it works.',
      'the original setup promise still covers thinking/network/backup'
    );
    assert.ok(
      !sectionDescs[1].textContent.includes('set it up'),
      'the notifications row sits under its own intro, not the setup promise'
    );
  });

  await testCase('G3-20 sync retires "observations", API-key hint is a step list', async () => {
    const harness = createHarness();
    const syncText = harness.document.getElementById('section-sync').textContent;
    assert.ok(!syncText.includes('observations'), 'retired vocabulary must not survive in sync copy');
    assert.ok(syncText.includes('material'), 'sync copy uses the replacement noun');

    const apiKeyField = harness.document
      .getElementById('field-env-plaud')
      .closest('.settings-field');
    const apiKeyHint = apiKeyField.querySelector('small');
    assert.ok(
      !apiKeyHint.textContent.includes('log into the web portal and extract token'),
      'the run-on console instruction is gone'
    );
    assert.ok(/1\).*2\).*3\)/.test(apiKeyField.textContent), 'the mechanics survive as a numbered step list');
  });

  await testCase('G3-115 API-keys pane stops instructing the owner to open devtools up front', async () => {
    const harness = createHarness();
    const apiKeyField = harness.document
      .getElementById('field-env-plaud')
      .closest('.settings-field');
    const visibleHint = apiKeyField.querySelector('small').textContent;
    assert.ok(
      !visibleHint.includes("open your browser's console"),
      'the devtools instruction is no longer in the always-visible hint'
    );
    const disclosure = apiKeyField.querySelector('details');
    assert.ok(disclosure, 'the step-by-step how-to is behind a disclosure');
    assert.ok(!disclosure.hasAttribute('open'), 'the disclosure is closed by default');
    assert.ok(
      disclosure.querySelector('summary').textContent.length > 0,
      'the disclosure has a visible summary label'
    );
    assert.ok(
      disclosure.textContent.includes("open your browser's console"),
      'the full mechanics are still reachable inside the disclosure'
    );
  });

  await testCase('G3-21 transcription/observer/vision/sync show an explicit loading state', async () => {
    const harness = createHarness();
    for (const id of ['transcriptionLoadState', 'observerLoadState', 'visionLoadState', 'syncLoadState']) {
      assert.strictEqual(
        harness.document.getElementById(id).textContent,
        'loading settings…',
        id + ' must not be a blank pane while its read is in flight'
      );
    }
    // Isolate the vision load-state wiring from the shared shell's Drawer
    // helper (not loaded by this harness) by stubbing the renderer.
    await run(harness, 'populateVision = () => {}');
    harness.context.fetch = async () => response({ max_extractions: 20 });
    await run(harness, 'loadVision()');
    assert.strictEqual(
      harness.document.getElementById('visionLoadState').textContent,
      '',
      'a resolved read clears the loading state'
    );
  });

  await testCase('G3-116 storage/facets loading strings follow the same "loading <what>…" pattern', async () => {
    const harness = createHarness();
    assert.strictEqual(
      harness.document.getElementById('storageLoadState').textContent,
      'loading storage settings…',
      'storage names itself, already matching the shared pattern'
    );
    assert.strictEqual(
      harness.document.querySelector('#facetsList .text-muted').textContent,
      'loading your facets…',
      'facets loads a list, not settings, so its loading string names that instead of copying the settings wording'
    );
  });

  await testCase('G3-118 storage retention pane states the retention choice', async () => {
    const harness = createHarness();
    const retentionText = harness.document.getElementById('retentionModeField').textContent;
    assert.ok(
      retentionText.includes('choose whether to keep your original audio, video, and screen frames'),
      'the actual retention choice copy survives'
    );
  });

  process.stdout.write('DOM CASES: ' + cases + ' passed\n', () => process.exit(0));
})().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
