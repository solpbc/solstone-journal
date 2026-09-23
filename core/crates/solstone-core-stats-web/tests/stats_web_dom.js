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

  insertBefore(newNode, referenceNode) {
    newNode.parentElement = this;
    const index = referenceNode ? this.children.indexOf(referenceNode) : -1;
    if (index >= 0) {
      this.children.splice(index, 0, newNode);
    } else {
      this.children.push(newNode);
    }
    return newNode;
  }

  remove() {
    if (this.parentElement) {
      const idx = this.parentElement.children.indexOf(this);
      if (idx >= 0) this.parentElement.children.splice(idx, 1);
      this.parentElement = null;
    }
  }

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
  createTextNode(text) {
    const el = new Element('#text', this);
    el.textContent = text;
    return el;
  }
  getElementById(id) { return this.querySelector('#' + id); }
  querySelector(selector) {
    if (selector === 'body') return this.body;
    return this.documentElement.querySelector(selector);
  }
  querySelectorAll(selector) { return this.documentElement.querySelectorAll(selector); }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
}

function loadDashboardSource() {
  return fs.readFileSync(path.join(crateDir, 'assets/static/dashboard.js'), 'utf8');
}

function createStatsDOM() {
  const doc = new Document();
  const main = doc.createElement('main');
  main.id = 'mainContent';

  const statsGrid = doc.createElement('div');
  statsGrid.id = 'statsGridBlock';
  main.appendChild(statsGrid);

  doc.body.appendChild(main);
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

function extractRenderBacklog(source) {
  const start = source.indexOf('function renderBacklog(stats) {');
  assert.ok(start >= 0, 'renderBacklog found in dashboard.js');
  const end = source.indexOf('function clearDashboardSections() {', start);
  assert.ok(end > start, 'end of renderBacklog found in dashboard.js');
  return source.slice(start, end);
}

const dashboardBacklogCode = extractRenderBacklog(loadDashboardSource());

function createEnvironment() {
  const doc = createStatsDOM();
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
    Math,
  });

  const glue = `
    function el(tag, attrs = {}, children = []) {
      const elem = document.createElement(tag);
      Object.entries(attrs).forEach(([k, v]) => {
        if (k === 'className') elem.className = v;
        else if (k === 'innerHTML') elem.innerHTML = v;
        else if (k === 'style' && typeof v === 'object') {
          Object.assign(elem.style, v);
        } else elem.setAttribute(k, v);
      });
      children.forEach(child => {
        if (typeof child === 'string') elem.appendChild(document.createTextNode(child));
        else if (child) elem.appendChild(child);
      });
      return elem;
    }
    function count(val) { return Number(val) || 0; }
    function backlogCopy() {
      return {
        VERDICT_CANT_TELL: "it's unclear whether your journal is caught up."
      };
    }
    function backlogCounts(stats) {
      const bl = stats.backlog || {};
      return {
        pending: count(bl.pending_days),
        stuck: count(bl.stuck_days)
      };
    }
    function stuckBucket(bl, C) { return null; }
    function backlogList(bl, counts, C) { return null; }
    ${dashboardBacklogCode}
  `;
  vm.runInContext(glue, context);
  return { doc, context };
}

test('stats hero renders verdict directly from journal_status.verdict', async () => {
  const { doc, context } = createEnvironment();
  const stats = {
    backlog: { pending_days: 0, stuck_days: 0 },
    journal_status: {
      verdict: "your journal's caught up.",
      freshness: "fresh",
      pending_days: 0,
      unfinished_activities: { activities: 1, day_count: 1, oldest_day: "20260403" },
      copy: {
        unfinished_template_one: "an activity from {day} couldn't finish processing",
      },
    },
  };
  context.stats = stats;
  vm.runInContext('renderBacklog(stats);', context);

  const heroLine = doc.querySelector('.backlog-hero-line');
  assert.ok(heroLine, 'hero line is rendered');
  assert.strictEqual(heroLine.textContent, "your journal's caught up.");

  const unfinishedLine = doc.querySelector('.backlog-unfinished-line');
  assert.ok(unfinishedLine, 'unfinished line is rendered');
  assert.strictEqual(unfinishedLine.textContent, "an activity from Apr 3, 2026 couldn't finish processing →");
});

test('stats hero renders verdict without rewrite for single pending day', async () => {
  const { doc, context } = createEnvironment();
  const stats = {
    backlog: { pending_days: 1, stuck_days: 0, oldest_pending_day: "20260403" },
    journal_status: {
      verdict: "1 day is still catching up.",
      freshness: "fresh",
      pending_days: 1,
      oldest_pending_day: "20260403",
    },
  };
  context.stats = stats;
  vm.runInContext('renderBacklog(stats);', context);

  const heroLine = doc.querySelector('.backlog-hero-line');
  assert.strictEqual(heroLine.textContent, "1 day is still catching up.");
});

test('stats hero renders nothing for unfinished line when template is missing', async () => {
  const { doc, context } = createEnvironment();
  const stats = {
    backlog: { pending_days: 0, stuck_days: 0 },
    journal_status: {
      verdict: "your journal's caught up.",
      freshness: "fresh",
      unfinished_activities: { activities: 1, day_count: 1, oldest_day: "20260403" },
      copy: {}, // Missing template
    },
  };
  context.stats = stats;
  vm.runInContext('renderBacklog(stats);', context);

  const unfinishedLine = doc.querySelector('.backlog-unfinished-line');
  assert.strictEqual(unfinishedLine, null);
});

async function runAsyncCases() {
  for (const runCase of asyncCases) await runCase();
  console.log('DOM CASES: ' + cases + ' passed');
}

runAsyncCases().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
