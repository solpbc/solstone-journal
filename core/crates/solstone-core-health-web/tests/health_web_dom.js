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
  return attribute.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function selectorMatches(element, selector) {
  const attributes = [...selector.matchAll(/\[([^\]=]+)(?:=["']?([^\]"']+)["']?)?\]/g)];
  const simple = selector.replace(/\[[^\]]+\]/g, '');
  const tag = simple.match(/^[a-zA-Z][\w-]*/);
  const id = simple.match(/#([\w-]+)/);
  const classes = [...simple.matchAll(/\.([\w-]+)/g)];
  if (tag && element.tagName !== tag[0].toUpperCase()) return false;
  if (id && element.id !== id[1]) return false;
  if (classes.some((match) => !element.classList.contains(match[1]))) return false;
  return attributes.every((match) => {
    if (!element.hasAttribute(match[1])) return false;
    return match[2] === undefined || element.getAttribute(match[1]) === match[2];
  });
}

function matchesSelector(element, selector) {
  return selector.split(',').some((part) => selectorMatches(element, part.trim()));
}

function queryAll(root, selector) {
  const pieces = selector.trim().split(/\s+/);
  let candidates = [root];
  for (const piece of pieces) {
    const next = [];
    for (const candidate of candidates) {
      const visit = (element) => {
        for (const child of element.children) {
          if (matchesSelector(child, piece)) next.push(child);
          visit(child);
        }
      };
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
    this.dataset = {};
    this.classList = new ClassList(this);
    this.style = {};
    this.value = '';
    this.disabled = false;
    this._textContent = '';
  }

  get id() { return this.getAttribute('id') || ''; }
  set id(value) { this.setAttribute('id', value); }
  get href() { return this.getAttribute('href') || ''; }
  set href(value) { this.setAttribute('href', value); }
  get className() { return this.getAttribute('class') || ''; }
  set className(value) { this.setAttribute('class', value); }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(value) { if (value) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  get textContent() {
    if (this.children.length > 0) {
      return this.children.map((c) => c.textContent).join('');
    }
    return this._textContent;
  }
  set textContent(value) {
    this.children = [];
    this._textContent = String(value);
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'class') this.classList.setFromString(value);
    if (name.startsWith('data-')) this.dataset[dataKey(name)] = String(value);
  }

  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  hasAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attributes, name); }

  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'class') this.classList.setFromString('');
    if (name.startsWith('data-')) delete this.dataset[dataKey(name)];
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  append(...children) { children.forEach((child) => this.appendChild(child)); }

  replaceChildren(...children) {
    this.children.forEach((child) => { child.parentElement = null; });
    this.children = [];
    this.append(...children);
  }

  querySelector(selector) { return queryAll(this, selector)[0] || null; }
  querySelectorAll(selector) { return queryAll(this, selector); }
  matches(selector) { return matchesSelector(this, selector); }

  closest(selector) {
    let current = this;
    while (current) {
      if (current.matches(selector)) return current;
      current = current.parentElement;
    }
    return null;
  }

  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }

  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    if (event.bubbles && !event.cancelBubble && this.parentElement) this.parentElement.dispatchEvent(event);
    return !event.defaultPrevented;
  }

  focus() { this.ownerDocument.activeElement = this; }
}

class Document {
  constructor() {
    this.readyState = 'complete';
    this.documentElement = new Element('html', this);
    this.body = new Element('body', this);
    this.documentElement.appendChild(this.body);
    this.activeElement = this.body;
    this.listeners = {};
  }

  createElement(tagName) { return new Element(tagName, this); }
  getElementById(id) { return this.querySelector('#' + id); }
  querySelector(selector) {
    if (selector === 'body') return this.body;
    return this.documentElement.querySelector(selector);
  }
  querySelectorAll(selector) { return this.documentElement.querySelectorAll(selector); }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
}

function loadHealthSource() {
  return fs.readFileSync(path.join(crateDir, 'assets/static/health.js'), 'utf8');
}

function createHealthDOM() {
  const doc = new Document();
  const backlogVerdict = doc.createElement('div');
  backlogVerdict.id = 'backlogVerdict';

  const verdictLine = doc.createElement('p');
  verdictLine.setAttribute('class', 'backlog-verdict-line');
  backlogVerdict.appendChild(verdictLine);

  const unfinishedLine = doc.createElement('p');
  unfinishedLine.setAttribute('class', 'backlog-unfinished-line');
  unfinishedLine.hidden = true;
  backlogVerdict.appendChild(unfinishedLine);

  const searchLine = doc.createElement('p');
  searchLine.setAttribute('class', 'backlog-search-line');
  searchLine.hidden = true;
  backlogVerdict.appendChild(searchLine);

  doc.body.appendChild(backlogVerdict);

  const stuckRowsContainer = doc.createElement('div');
  stuckRowsContainer.setAttribute('data-backlog-stuck-rows', '');
  const rowsHost = doc.createElement('div');
  rowsHost.setAttribute('data-backlog-rows', '');
  stuckRowsContainer.appendChild(rowsHost);
  doc.body.appendChild(stuckRowsContainer);

  return doc;
}

let cases = 0;
const asyncCases = [];
function test(name, fn) {
  cases += 1;
  asyncCases.push(async () => {
    try {
      await fn();
    } catch (err) {
      console.error(`FAILED: ${name}`);
      throw err;
    }
  });
}

function extractRenderFunctions(source) {
  // Extract renderBacklogState and renderBacklogVerdict
  const start = source.indexOf('function renderBacklogVerdict(');
  const end = source.indexOf('function renderMediaBacklog(');
  assert.ok(start >= 0 && end > start, 'renderBacklog functions found in health.js');
  return source.slice(start, end);
}

const healthCode = extractRenderFunctions(loadHealthSource());

function createEnvironment() {
  const doc = createHealthDOM();
  const context = vm.createContext({
    document: doc,
    window: {
      JournalFormat: { day: (d) => `Apr 3, 2026` },
      location: { href: '' },
    },
    console,
    encodeURIComponent,
    Number,
    String,
    Array,
    Object,
  });

  const glue = `
    let backlogCopy = {};
    function clearHealthStateError() {}
    ${healthCode}
  `;
  vm.runInContext(glue, context);
  return { doc, context };
}

test('unfinished activities renders single day with template_one', async () => {
  const { doc, context } = createEnvironment();
  const payload = {
    verdict: "your journal's caught up.",
    unfinished_activities: { activities: 1, day_count: 1, oldest_day: "20260403" },
    copy: {
      unfinished_template_one: "1 unfinished activity on {day}",
      unfinished_template_many_one_day: "{n} unfinished activities on {day}",
      unfinished_template_many_days: "{n} unfinished activities on {days} completed days, oldest {day}",
    },
  };
  context.payload = payload;
  vm.runInContext('renderBacklogState(payload, null);', context);

  const unfinishedLine = doc.querySelector('#backlogVerdict .backlog-unfinished-line');
  assert.strictEqual(unfinishedLine.hidden, false);
  const link = unfinishedLine.querySelector('a');
  assert.ok(link, 'link is rendered');
  assert.strictEqual(link.textContent, '1 unfinished activity on Apr 3, 2026 →');
  assert.strictEqual(link.getAttribute('href'), '/app/transcripts/20260403');
});

test('unfinished activities with missing template renders nothing', async () => {
  const { doc, context } = createEnvironment();
  const payload = {
    verdict: "your journal's caught up.",
    unfinished_activities: { activities: 1, day_count: 1, oldest_day: "20260403" },
    copy: {}, // No templates
  };
  context.payload = payload;
  vm.runInContext('renderBacklogState(payload, null);', context);

  const unfinishedLine = doc.querySelector('#backlogVerdict .backlog-unfinished-line');
  assert.strictEqual(unfinishedLine.hidden, true);
  assert.strictEqual(unfinishedLine.children.length, 0);
});

test('unfinished activities renders multiple activities across multiple days', async () => {
  const { doc, context } = createEnvironment();
  const payload = {
    verdict: "your journal's caught up.",
    unfinished_activities: { activities: 3, day_count: 2, oldest_day: "20260401" },
    copy: {
      unfinished_template_many_days: "{n} unfinished activities on {days} completed days, oldest {day}",
    },
  };
  context.payload = payload;
  vm.runInContext('renderBacklogState(payload, null);', context);

  const unfinishedLine = doc.querySelector('#backlogVerdict .backlog-unfinished-line');
  assert.strictEqual(unfinishedLine.hidden, false);
  const link = unfinishedLine.querySelector('a');
  assert.ok(link);
  assert.strictEqual(link.textContent, '3 unfinished activities on 2 completed days, oldest Apr 3, 2026 →');
});

test('search index line renders when present and hides when absent', async () => {
  const { doc, context } = createEnvironment();
  const searchIndex = { state: "fresh", text: "search is current." };
  context.payload = { verdict: "your journal's caught up." };
  context.searchIndex = searchIndex;
  vm.runInContext('renderBacklogState(payload, searchIndex);', context);

  const searchLine = doc.querySelector('#backlogVerdict .backlog-search-line');
  assert.strictEqual(searchLine.hidden, false);
  assert.strictEqual(searchLine.textContent, 'search is current.');

  vm.runInContext('renderBacklogState(payload, null);', context);
  assert.strictEqual(searchLine.hidden, true);
});

async function runAsyncCases() {
  for (const runCase of asyncCases) await runCase();
  console.log('DOM CASES: ' + cases + ' passed');
}

runAsyncCases().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
