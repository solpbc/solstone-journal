// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const nodeAssert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

let executedCases = 0;
let passedCases = 0;
function recordCase(assertion) {
  executedCases += 1;
  const result = assertion();
  passedCases += 1;
  return result;
}
const assert = new Proxy(nodeAssert, {
  apply(target, thisArg, args) {
    return recordCase(() => Reflect.apply(target, thisArg, args));
  },
  get(target, property) {
    const value = Reflect.get(target, property);
    if (typeof value !== 'function') return value;
    return (...args) => recordCase(() => Reflect.apply(value, target, args));
  },
});

class ClassList {
  constructor(element) {
    this.element = element;
    this.values = new Set();
  }

  add(...names) {
    names.forEach((name) => this.values.add(name));
    this.element._syncClassString();
  }

  remove(...names) {
    names.forEach((name) => this.values.delete(name));
    this.element._syncClassString();
  }

  toggle(name, force) {
    const present = force === undefined ? !this.values.has(name) : force;
    if (present) this.values.add(name);
    else this.values.delete(name);
    this.element._syncClassString();
    return present;
  }

  contains(name) {
    return this.values.has(name);
  }

  toString() {
    return Array.from(this.values).join(' ');
  }
}

function parseAttributes(source, element) {
  if (!source) return;
  const attributePattern = /([a-zA-Z0-9_:-]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
  let match;
  while ((match = attributePattern.exec(source))) {
    const name = match[1];
    const value = match[2] ?? match[3] ?? match[4] ?? '';
    element.setAttribute(name, value);
  }
}

function parseHtml(html, root, doc) {
  root.replaceChildren();
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
    const child = doc.createElement(parts[1]);
    parseAttributes(parts[2], child);
    stack[stack.length - 1].appendChild(child);
    if (!selfClosing && !['BR', 'IMG', 'INPUT', 'META', 'LINK', 'RECT', 'PATH'].includes(child.tagName)) {
      stack.push(child);
    }
  }
}

function createDatasetProxy(element) {
  const store = {};
  return new Proxy(store, {
    set(target, prop, value) {
      target[prop] = String(value);
      const kebab = 'data-' + String(prop).replace(/[A-Z]/g, (m) => '-' + m.toLowerCase());
      element.attributes[kebab] = String(value);
      return true;
    },
    get(target, prop) {
      return target[prop];
    },
    deleteProperty(target, prop) {
      delete target[prop];
      const kebab = 'data-' + String(prop).replace(/[A-Z]/g, (m) => '-' + m.toLowerCase());
      delete element.attributes[kebab];
      return true;
    },
  });
}

class MockElement {
  constructor(tagName, ownerDoc) {
    this.tagName = (tagName || 'DIV').toUpperCase();
    this.ownerDocument = ownerDoc;
    this.nodeType = this.tagName === '#TEXT' ? 3 : 1;
    this.children = [];
    this.parentNode = null;
    this.nextSibling = null;
    this.attributes = {};
    this._textContent = '';
    this._nodeValue = '';
    this._className = '';
    this.classList = new ClassList(this);
    this.dataset = createDatasetProxy(this);
    this.disabled = false;
    this.hidden = false;
    this.tabIndex = 0;
    this.type = 'button';
    this.title = '';
    this.style = {
      _props: {},
      setProperty: (k, v) => {
        this.style._props[k] = String(v);
      },
      getPropertyValue: (k) => this.style._props[k] || '',
    };
    this._listeners = new Map();
  }

  get nodeValue() {
    return this.nodeType === 3 ? this._nodeValue : null;
  }

  set nodeValue(val) {
    this._nodeValue = String(val);
    this._textContent = String(val);
  }

  get childNodes() {
    return this.children;
  }

  get firstChild() {
    return this.children[0] || null;
  }

  get lastChild() {
    return this.children[this.children.length - 1] || null;
  }

  get className() {
    return this._className;
  }

  set className(val) {
    this._className = String(val || '');
    this.classList.values = new Set(this._className.split(/\s+/).filter(Boolean));
  }

  _syncClassString() {
    this._className = Array.from(this.classList.values).join(' ');
  }

  get textContent() {
    if (this.nodeType === 3) return this._nodeValue;
    return this.children.length > 0
      ? this.children.map((c) => c.textContent).join('')
      : this._textContent;
  }

  set textContent(val) {
    if (this.nodeType === 3) {
      this._nodeValue = String(val);
      this._textContent = String(val);
    } else {
      this.replaceChildren();
      this._textContent = String(val);
      if (val) {
        const textNode = this.ownerDocument.createTextNode(val);
        this.appendChild(textNode);
      }
    }
  }

  get innerHTML() {
    return '';
  }

  set innerHTML(html) {
    parseHtml(html, this, this.ownerDocument);
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'class') {
      this.className = String(value);
    }
    if (name === 'hidden') {
      this.hidden = true;
    }
    if (name.startsWith('data-')) {
      const camel = name
        .slice(5)
        .replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
      this.dataset[camel] = String(value);
    }
  }

  getAttribute(name) {
    if (name === 'class') return this._className;
    return this.attributes[name] !== undefined ? this.attributes[name] : null;
  }

  hasAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name);
  }

  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'class') this.className = '';
    if (name === 'hidden') this.hidden = false;
    if (name.startsWith('data-')) {
      const camel = name
        .slice(5)
        .replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
      delete this.dataset[camel];
    }
  }

  appendChild(child) {
    if (child.parentNode) {
      child.parentNode.removeChild(child);
    }
    child.parentNode = this;
    const last = this.children[this.children.length - 1];
    if (last) last.nextSibling = child;
    child.nextSibling = null;
    this.children.push(child);
    return child;
  }

  append(...items) {
    for (const item of items) {
      if (typeof item === 'string') {
        const textNode = this.ownerDocument.createTextNode(item);
        this.appendChild(textNode);
      } else if (item) {
        this.appendChild(item);
      }
    }
  }

  insertBefore(newChild, refChild) {
    if (newChild.parentNode) {
      newChild.parentNode.removeChild(newChild);
    }
    newChild.parentNode = this;
    if (!refChild) {
      this.appendChild(newChild);
      return newChild;
    }
    const idx = this.children.indexOf(refChild);
    if (idx === -1) {
      this.appendChild(newChild);
      return newChild;
    }
    const prev = idx > 0 ? this.children[idx - 1] : null;
    if (prev) prev.nextSibling = newChild;
    newChild.nextSibling = refChild;
    this.children.splice(idx, 0, newChild);
    return newChild;
  }

  removeChild(child) {
    const idx = this.children.indexOf(child);
    if (idx === -1) return child;
    const prev = idx > 0 ? this.children[idx - 1] : null;
    if (prev) {
      prev.nextSibling = child.nextSibling;
    }
    child.parentNode = null;
    child.nextSibling = null;
    this.children.splice(idx, 1);
    return child;
  }

  replaceChildren(...newChildren) {
    while (this.children.length > 0) {
      this.removeChild(this.children[0]);
    }
    for (const c of newChildren) {
      if (c) this.appendChild(c);
    }
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }

  querySelectorAll(selector) {
    const results = [];
    const test = (el) => {
      if (el.nodeType === 1 && matchesSelector(el, selector)) {
        results.push(el);
      }
      for (const child of el.children) {
        test(child);
      }
    };
    for (const child of this.children) {
      test(child);
    }
    return results;
  }

  closest(selector) {
    let curr = this;
    while (curr) {
      if (curr.nodeType === 1 && matchesSelector(curr, selector)) return curr;
      curr = curr.parentNode;
    }
    return null;
  }

  contains(node) {
    let curr = node;
    while (curr) {
      if (curr === this) return true;
      curr = curr.parentNode;
    }
    return false;
  }

  matches(selector) {
    return matchesSelector(this, selector);
  }

  addEventListener(type, listener, options) {
    if (!this._listeners.has(type)) this._listeners.set(type, new Set());
    this._listeners.get(type).add(listener);
    if (options && options.signal) {
      options.signal.addEventListener('abort', () => {
        this.removeEventListener(type, listener);
      });
    }
  }

  removeEventListener(type, listener) {
    const set = this._listeners.get(type);
    if (set) set.delete(listener);
  }

  dispatchEvent(event) {
    if (!event.target) {
      event.target = this;
    }
    event.currentTarget = this;
    const set = this._listeners.get(event.type);
    if (set) {
      for (const listener of Array.from(set)) {
        listener(event);
      }
    }
    return true;
  }

  click() {
    const event = {
      type: 'click',
      target: this,
      currentTarget: this,
      preventDefault: () => {},
      stopPropagation: () => {},
    };
    this.dispatchEvent(event);
    let p = this.parentNode;
    while (p) {
      p.dispatchEvent(event);
      p = p.parentNode;
    }
    if (this.ownerDocument) {
      this.ownerDocument.dispatchEvent(event);
    }
  }

  focus() {}
  blur() {}
}

function matchesSelector(element, selector) {
  if (!element || element.nodeType !== 1) return false;
  selector = selector.trim();
  if (selector === '*') return true;
  if (selector.includes(',')) {
    return selector.split(',').some((part) => matchesSelector(element, part));
  }
  if (selector.includes(':not(:disabled)')) {
    if (element.disabled) return false;
    selector = selector.replace(':not(:disabled)', '');
  }
  if (selector.includes(':disabled')) {
    if (!element.disabled) return false;
    selector = selector.replace(':disabled', '');
  }
  selector = selector.trim();
  if (!selector) return true;
  const attributes = [...selector.matchAll(/\[([^\]=]+)(?:=["']?([^\]"']+)["']?)?\]/g)];
  let baseSelector = selector.replace(/\[[^\]]+\]/g, '');
  const idMatch = baseSelector.match(/#([\w-]+)/);
  const classMatches = [...baseSelector.matchAll(/\.([\w-]+)/g)];
  const tagMatch = baseSelector.match(/^[a-zA-Z][\w-]*/);
  if (tagMatch && element.tagName !== tagMatch[0].toUpperCase()) return false;
  if (idMatch && element.getAttribute('id') !== idMatch[1]) return false;
  if (classMatches.some((match) => !element.classList.contains(match[1]))) return false;
  return attributes.every((match) => {
    if (!element.hasAttribute(match[1])) return false;
    return match[2] === undefined || element.getAttribute(match[1]) === match[2];
  });
}

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');

const copySrc = fs.readFileSync(path.join(crateDir, 'assets', 'static', 'convey_copy.js'), 'utf8');
const clockSrc = fs.readFileSync(path.join(crateDir, 'assets', 'static', 'journal_clock.js'), 'utf8');
const dateFormatSrc = fs.readFileSync(path.join(crateDir, 'assets', 'static', 'date_format.js'), 'utf8');
const dayGridSrc = fs.readFileSync(path.join(crateDir, 'assets', 'static', 'day-grid.js'), 'utf8');
const dateNavSrc = fs.readFileSync(path.join(crateDir, 'assets', 'static', 'date-nav.js'), 'utf8');

async function run() {
  const fixedInstant = Date.parse('2026-09-30T18:00:00.000Z');

  const docListeners = new Map();
  const documentObj = {
    createElement: (tag) => new MockElement(tag, documentObj),
    createTextNode: (text) => {
      const el = new MockElement('#text', documentObj);
      el.nodeValue = text;
      return el;
    },
    querySelector: (sel) => documentElement.querySelector(sel),
    querySelectorAll: (sel) => documentElement.querySelectorAll(sel),
    addEventListener: (type, fn, opts) => {
      if (!docListeners.has(type)) docListeners.set(type, new Set());
      docListeners.get(type).add(fn);
      if (opts?.signal) {
        opts.signal.addEventListener('abort', () => docListeners.get(type)?.delete(fn));
      }
    },
    removeEventListener: (type, fn) => {
      docListeners.get(type)?.delete(fn);
    },
    dispatchEvent: (event) => {
      const set = docListeners.get(event.type);
      if (set) {
        for (const fn of Array.from(set)) {
          fn(event);
        }
      }
    },
  };

  const documentElement = new MockElement('html', documentObj);
  const body = new MockElement('body', documentObj);
  documentElement.appendChild(body);
  documentObj.body = body;

  const navHost = new MockElement('div', documentObj);
  navHost.setAttribute('data-date-nav', '');
  body.appendChild(navHost);

  const headingEl = new MockElement('h1', documentObj);
  headingEl.setAttribute('data-date-nav-heading', '');
  body.appendChild(headingEl);

  const locationObj = {
    pathname: '/app/records/20260901',
    search: '',
    href: '',
  };

  const windowObj = {
    location: locationObj,
    document: documentObj,
    addEventListener: documentObj.addEventListener,
    removeEventListener: documentObj.removeEventListener,
    dispatchEvent: documentObj.dispatchEvent,
    requestAnimationFrame: (cb) => setTimeout(cb, 0),
    cancelAnimationFrame: (id) => clearTimeout(id),
    solShellData: {
      apps: [
        {
          id: 'records',
          name: 'records',
          date_nav: { allow_future: true, unit: 'record' },
        },
      ],
    },
    apiJson: async (url) => {
      if (url.includes('api/index')) {
        return {
          coverage: { start: '20260901', end: '20261031' },
          months: { '202609': 1, '202610': 1 },
        };
      }
      if (url.includes('api/stats/202610')) {
        return { '20261001': 1, '20261002': 0 };
      }
      if (url.includes('api/stats/202609')) {
        return { '20260901': 1 };
      }
      return {};
    },
  };

  const context = {
    window: windowObj,
    document: documentObj,
    Intl,
    Date,
    console,
    AbortController,
    requestAnimationFrame: (cb) => setTimeout(cb, 0),
    cancelAnimationFrame: (id) => clearTimeout(id),
  };

  vm.createContext(context);
  vm.runInContext(copySrc, context, { filename: 'convey_copy.js' });
  vm.runInContext(clockSrc, context, { filename: 'journal_clock.js' });
  vm.runInContext(dateFormatSrc, context, { filename: 'date_format.js' });
  vm.runInContext(dayGridSrc, context, { filename: 'day-grid.js' });

  // Seed Tokyo before date-nav load
  context.window.JournalClock.setNow(() => fixedInstant);
  context.window.JournalClock.seed({ tz: 'Asia/Tokyo', label: 'Tokyo' });

  vm.runInContext(dateNavSrc, context, { filename: 'date-nav.js' });

  // Mount real nav via workspace:mounted event
  documentObj.dispatchEvent({
    type: 'workspace:mounted',
    detail: { appName: 'records' },
  });

  // Wait for index promise
  await new Promise((resolve) => setTimeout(resolve, 50));

  // 1. Click [data-date-nav-today]. navigateTo writes window.location.href ending with 20261001
  const todayBtn = documentObj.querySelector('[data-date-nav-today]');
  assert.ok(todayBtn, 'today button exists');
  todayBtn.click();
  assert.ok(
    locationObj.href.endsWith('20261001'),
    `today navigation href must end with 20261001: ${locationObj.href}`
  );

  // 2. Simulate navigation to that day (location.pathname = '/app/records/20261001', dispatch workspace:mounted)
  locationObj.pathname = '/app/records/20261001';
  documentObj.dispatchEvent({
    type: 'workspace:mounted',
    detail: { appName: 'records' },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));

  const triggerBtn = documentObj.querySelector('[data-date-nav-trigger]');
  assert.ok(triggerBtn, 'trigger button exists');
  triggerBtn.click();
  await new Promise((resolve) => setTimeout(resolve, 50));

  // Read cells in October grid
  const cells = documentObj.querySelectorAll('[data-date-nav-cell="day"]');
  const cell20261001 = cells.find((c) => c.dataset.value === '20261001');
  assert.ok(cell20261001, 'cell 20261001 exists');
  assert.strictEqual(cell20261001.disabled, false);

  const cell20261002 = cells.find((c) => c.dataset.value === '20261002');
  assert.ok(cell20261002, 'cell 20261002 exists');
  assert.strictEqual(cell20261002.disabled, false, 'future cell 20261002 is not disabled under allow_future');

  // The 1st sits under its own weekday, Sunday-first: October 1, 2026 is a Thursday.
  const dayGrid = documentObj.querySelector('[data-date-nav-grid]');
  const [weekdayRow, firstWeek, secondWeek] = dayGrid.children;
  assert.strictEqual(weekdayRow.querySelectorAll('[role="columnheader"]').length, 7, 'day grid has seven weekday headers');
  assert.strictEqual(firstWeek.children.length, 7, 'first week row fills seven columns');
  assert.strictEqual(firstWeek.children.indexOf(cell20261001), 4, 'October 1, 2026 sits in the Thursday column');
  assert.strictEqual(secondWeek.children[0].dataset.value, '20261004', 'second week starts on Sunday, October 4');

  // Down keeps the column: from the 1st to the 8th.
  documentObj.querySelector('[data-date-nav-root]').dispatchEvent({
    type: 'keydown',
    key: 'ArrowDown',
    target: cell20261001,
    preventDefault: () => {},
    stopPropagation: () => {},
  });
  const cell20261008 = cells.find((c) => c.dataset.value === '20261008');
  assert.strictEqual(cell20261008.tabIndex, 0, 'ArrowDown from October 1 moves to October 8');

  // 3. Heading element contains one [data-zone-note]
  const headingNote = headingEl.querySelector('[data-zone-note]');
  assert.ok(headingNote, 'heading contains one [data-zone-note]');

  // 4. Seed Denver on same instant, call label update again -> note absent
  context.window.JournalClock.seed({ tz: 'America/Denver', label: 'Denver' });
  documentObj.dispatchEvent({
    type: 'workspace:mounted',
    detail: { appName: 'records' },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  const headingNoteAfter = headingEl.querySelector('[data-zone-note]');
  assert.strictEqual(headingNoteAfter, null, 'heading note is absent after switching to Denver');

  // 5. Second app, allow_future: false, coverage end: '20260929', pathname day 20260929
  windowObj.solShellData.apps.push({
    id: 'pastapp',
    name: 'pastapp',
    date_nav: { allow_future: false, unit: 'item' },
  });
  locationObj.pathname = '/app/pastapp/20260929';
  windowObj.apiJson = async (url) => {
    if (url.includes('api/index')) {
      return {
        coverage: { start: '20260901', end: '20260929' },
        months: { '202609': 1 },
      };
    }
    return {};
  };

  documentObj.dispatchEvent({
    type: 'workspace:mounted',
    detail: { appName: 'pastapp' },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));

  const nextBtn = documentObj.querySelector('[data-date-nav-next]');
  assert.ok(nextBtn, 'next button exists');
  assert.strictEqual(nextBtn.disabled, true, 'next button is disabled when at coverage end and allow_future is false');

  // Pathname day 20260928 with same coverage: forward control not disabled
  locationObj.pathname = '/app/pastapp/20260928';
  documentObj.dispatchEvent({
    type: 'workspace:mounted',
    detail: { appName: 'pastapp' },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));

  const nextBtn28 = documentObj.querySelector('[data-date-nav-next]');
  assert.strictEqual(nextBtn28.disabled, false, 'next button is enabled when not at coverage end');

  // 6. DayGrid.mount
  context.window.JournalClock.seed({ tz: 'Asia/Tokyo', label: 'Tokyo' });
  const gridHost = new MockElement('div', documentObj);
  body.appendChild(gridHost);

  context.window.DayGrid.mount(gridHost, {
    data: {
      coverage: { start: '20260901', end: '20261031' },
      days: { '20261001': 5 },
    },
    appPath: '/app/records',
  });

  const todayGridCell = gridHost.querySelector('.daygrid-cell--today');
  assert.ok(todayGridCell, 'daygrid today cell exists');
  assert.strictEqual(
    todayGridCell.getAttribute('data-daygrid-date'),
    '20261001',
    'daygrid today cell is 20261001'
  );

  // 7. Date nav keep-search, generation-tracked refresh, and defaultPrevented keydown guard
  windowObj.solShellData.apps.push({
    id: 'searchapp',
    name: 'searchapp',
    date_nav: { allow_future: true, unit: 'item' },
  });
  locationObj.pathname = '/app/searchapp/20261001';
  locationObj.search = '?facet=work';
  locationObj.href = '/app/searchapp/20261001?facet=work';

  navHost.setAttribute('data-date-nav-keep-search', '');

  const requestedUrls = [];
  windowObj.apiJson = async (url) => {
    requestedUrls.push(url);
    if (url.includes('api/index')) {
      return {
        coverage: { start: '20261001', end: '20261031' },
        months: { '202610': 3 },
      };
    }
    if (url.includes('api/stats')) {
      return { '20261001': 3 };
    }
    return {};
  };

  documentObj.dispatchEvent({
    type: 'workspace:mounted',
    detail: { appName: 'searchapp' },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));

  assert.strictEqual(typeof navHost.refresh, 'function', 'host.refresh is attached when data-date-nav-keep-search is present');
  assert.ok(requestedUrls.some((u) => u.includes('api/index?facet=work')), 'index fetch includes location.search');

  // navigateTo preserves location.search
  const todayBtnSearch = documentObj.querySelector('[data-date-nav-today]');
  todayBtnSearch.click();
  assert.ok(locationObj.href.endsWith('20261001?facet=work'), `href preserves search: ${locationObj.href}`);

  // Test defaultPrevented guard on keydown
  const prevHref = locationObj.href;
  documentObj.dispatchEvent({
    type: 'keydown',
    key: 'ArrowLeft',
    defaultPrevented: true,
    target: navHost,
    preventDefault: () => {},
  });
  assert.strictEqual(locationObj.href, prevHref, 'keydown with defaultPrevented does not navigate');

  // Test refresh() increments generation and refetches both index and shown month with search while panel closed
  requestedUrls.length = 0;
  await navHost.refresh();
  assert.ok(requestedUrls.some((u) => u.includes('api/index?facet=work')), 'refresh triggers index refetch with search');
  assert.ok(requestedUrls.some((u) => u.includes('api/stats/202610?facet=work')), 'refresh triggers stats refetch for shown month with search while panel closed');

  // Refresh while in-flight issues a second fetch; second response renders, earlier discarded
  let resolveFirstIndex, resolveSecondIndex;
  const firstIndexPromise = new Promise((res) => { resolveFirstIndex = res; });
  const secondIndexPromise = new Promise((res) => { resolveSecondIndex = res; });
  let indexCalls = 0;
  windowObj.apiJson = async (url) => {
    if (url.includes('api/index')) {
      indexCalls += 1;
      if (indexCalls === 1) {
        await firstIndexPromise;
        return {
          coverage: { start: '20261001', end: '20261031' },
          months: { '202610': 1 },
        };
      } else {
        await secondIndexPromise;
        return {
          coverage: { start: '20261001', end: '20261031' },
          months: { '202610': 99 },
        };
      }
    }
    if (url.includes('api/stats')) {
      return { '20261001': indexCalls === 1 ? 1 : 99 };
    }
    return {};
  };

  const refresh1 = navHost.refresh();
  const refresh2 = navHost.refresh();
  resolveSecondIndex();
  await refresh2;
  resolveFirstIndex();
  await refresh1;

  const triggerForCheck = documentObj.querySelector('[data-date-nav-trigger]');
  triggerForCheck.click();
  await new Promise((resolve) => setTimeout(resolve, 50));
  const cellOct1 = documentObj.querySelector('[data-date-nav-cell="day"][data-value="20261001"]');
  assert.ok(cellOct1, 'October 1 cell exists');
  assert.ok(cellOct1.getAttribute('aria-label').includes('99'), 'second response rendered, earlier response discarded');
  triggerForCheck.click(); // close panel

  // Stale fetch that fails after current generation succeeded shows no warning
  let rejectStaleIndex;
  const staleIndexPromise = new Promise((_, rej) => { rejectStaleIndex = rej; });
  indexCalls = 0;
  windowObj.apiJson = async (url) => {
    if (url.includes('api/index')) {
      indexCalls += 1;
      if (indexCalls === 1) {
        await staleIndexPromise;
      }
      return {
        coverage: { start: '20261001', end: '20261031' },
        months: { '202610': 3 },
      };
    }
    return { '20261001': 3 };
  };
  const refreshStale = navHost.refresh();
  const refreshCurrent = navHost.refresh();
  await refreshCurrent;
  try {
    rejectStaleIndex(new Error('stale network error'));
    await refreshStale;
  } catch (_) {}
  const warningNode = documentObj.querySelector('[data-date-nav-warning]');
  assert.strictEqual(warningNode.hidden, true, 'stale fetch failure shows no warning');

  // Host without the attribute still drops location.search on day change
  navHost.removeAttribute('data-date-nav-keep-search');
  todayBtnSearch.click();
  assert.ok(locationObj.href.endsWith('20261001'), 'without keep-search attribute, day change drops search');
  assert.strictEqual(locationObj.href.includes('?'), false, 'no query string in href');

  // Unprevented ArrowRight steps the day (the fixture's coverage starts on this day)
  const hrefBeforeStep = locationObj.href;
  documentObj.dispatchEvent({
    type: 'keydown',
    key: 'ArrowRight',
    defaultPrevented: false,
    target: navHost,
    preventDefault: () => {},
  });
  assert.notStrictEqual(locationObj.href, hrefBeforeStep, 'unprevented ArrowRight steps the day');

  // An index that reports freshness: bounded re-reads, change envelopes, reconnects.
  const timers = [];
  windowObj.setTimeout = (fn, ms) => {
    const timer = { fn, ms, live: true };
    timers.push(timer);
    return timer;
  };
  windowObj.clearTimeout = (timer) => {
    if (timer) timer.live = false;
  };
  const liveTimers = () => timers.filter((timer) => timer.live);
  const fireTimers = async () => {
    liveTimers().forEach((timer) => {
      timer.live = false;
      timer.fn();
    });
    await new Promise((resolve) => setTimeout(resolve, 10));
  };
  const appListeners = { change: new Set(), connection: new Set() };
  windowObj.appEvents = {
    listen: (tract, fn) => {
      appListeners[tract].add(fn);
      return () => appListeners[tract].delete(fn);
    },
    onConnectionState: (fn) => {
      appListeners.connection.add(fn);
      fn({ connected: true, state: 'connected' });
      return () => appListeners.connection.delete(fn);
    },
  };
  const sendChange = (msg) => appListeners.change.forEach((fn) => fn(msg));
  let indexReads = 0;
  let nextIndex = null;
  windowObj.apiJson = async (url) => {
    if (url.includes('api/index')) {
      indexReads += 1;
      return nextIndex;
    }
    return { '20261001': 1 };
  };
  const mountFreshness = async (index) => {
    nextIndex = index;
    indexReads = 0;
    documentObj.dispatchEvent({ type: 'workspace:mounted', detail: { appName: 'records' } });
    await new Promise((resolve) => setTimeout(resolve, 20));
  };
  const coverage = { start: '20261001', end: '20261031' };
  const noteNode = () => documentObj.querySelector('[data-date-nav-freshness]');

  // No freshness field: one read, and nothing reacts to a change or a reconnect.
  await mountFreshness({ coverage, months: { '202610': 1 } });
  assert.strictEqual(indexReads, 1, 'an index without freshness is read once');
  assert.strictEqual(liveTimers().length, 0, 'an index without freshness schedules nothing');
  sendChange({ tract: 'change', event: 'changed', kind: 'day', key: '20261001' });
  appListeners.connection.forEach((fn) => fn({ connected: true }));
  assert.strictEqual(liveTimers().length, 0, 'a view whose index has no freshness ignores change and reconnect');
  assert.strictEqual(noteNode().hidden, true, 'an index without freshness shows no note');

  // Behind: labelled, re-read on a timer until complete, then quiet.
  await mountFreshness({
    coverage,
    months: { '202610': 1 },
    freshness: { complete: false, behind: 3, failed: 0, oldest: '2026-11-05T09:00:00Z' },
  });
  assert.strictEqual(noteNode().hidden, false, 'a behind index shows a note');
  assert.strictEqual(liveTimers().length, 1, 'a behind index schedules one re-read');
  nextIndex = { coverage, months: { '202610': 7 }, freshness: { complete: true } };
  await fireTimers();
  assert.strictEqual(indexReads, 2, 'the timer re-reads the index');
  assert.strictEqual(noteNode().hidden, true, 'a complete answer clears the note');
  assert.strictEqual(liveTimers().length, 0, 'a complete answer stops the re-reads');

  // Behind forever: the re-reads end.
  nextIndex = { coverage, months: { '202610': 1 }, freshness: { complete: false, behind: 1, failed: 0, oldest: 'x' } };
  await mountFreshness(nextIndex);
  let rounds = 0;
  while (liveTimers().length > 0 && rounds < 50) {
    rounds += 1;
    await fireTimers();
  }
  assert.ok(rounds > 0 && rounds < 50, 'a stuck index stops being re-read');
  assert.strictEqual(noteNode().hidden, false, 'a stuck index keeps its note');

  // Failed days only: labelled, with no timer, because the server's own retry announces itself.
  await mountFreshness({ coverage, months: {}, freshness: { complete: false, behind: 0, failed: 2, oldest: 'x' } });
  assert.strictEqual(noteNode().hidden, false, 'failed days show a note');
  assert.strictEqual(liveTimers().length, 0, 'failed days alone schedule no re-read');

  // A burst of envelopes settles into one read; other kinds and keys don't count.
  await mountFreshness({ coverage, months: { '202610': 1 }, freshness: { complete: true } });
  sendChange({ tract: 'change', event: 'changed', kind: 'entity', key: 'x' });
  sendChange({ tract: 'change', event: 'changed', kind: 'system', key: 'other' });
  assert.strictEqual(liveTimers().length, 0, 'an unknown kind or system key is ignored');
  sendChange({ tract: 'change', event: 'changed', kind: 'day', key: '20261001' });
  sendChange({ tract: 'change', event: 'changed', kind: 'day', key: '20261002' });
  sendChange({ tract: 'change', event: 'changed', kind: 'system', key: 'calendar' });
  assert.strictEqual(liveTimers().length, 1, 'a burst of envelopes holds one pending read');
  await fireTimers();
  assert.strictEqual(indexReads, 2, 'a burst of envelopes costs one read');
  appListeners.connection.forEach((fn) => fn({ connected: true }));
  await fireTimers();
  assert.strictEqual(indexReads, 3, 'a reconnect re-reads an index with freshness');
  appListeners.connection.forEach((fn) => fn({ connected: false }));
  assert.strictEqual(liveTimers().length, 0, 'a disconnect schedules nothing');

  // Leaving the view stops everything.
  sendChange({ tract: 'change', event: 'changed', kind: 'day', key: '20261001' });
  assert.strictEqual(liveTimers().length, 1, 'a change is pending before unmount');
  documentObj.dispatchEvent({ type: 'workspace:mounted', detail: { appName: null } });
  assert.strictEqual(liveTimers().length, 0, 'unmount cancels pending reads');
  assert.strictEqual(appListeners.change.size, 0, 'unmount removes the change listener');
  assert.strictEqual(appListeners.connection.size, 0, 'unmount removes the connection listener');
}

run()
  .then(() => {
    console.log(`NAV CASES: ${passedCases} passed`);
  })
  .catch((err) => {
    console.error(err);
    process.exit(1);
  });
