// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

let cases = 0;
function check(condition, message) {
  cases += 1;
  assert.ok(condition, message);
}

class ClassList {
  constructor() {
    this.values = new Set();
  }

  add(...names) {
    names.forEach((name) => this.values.add(name));
  }

  remove(...names) {
    names.forEach((name) => this.values.delete(name));
  }

  contains(name) {
    return this.values.has(name);
  }
}

class Style {
  constructor() {
    this.values = new Map();
  }

  setProperty(name, value) {
    this.values.set(name, value);
  }

  removeProperty(name) {
    this.values.delete(name);
  }

  getPropertyValue(name) {
    return this.values.get(name) || '';
  }
}

class Element {
  constructor(document, id = '') {
    this.ownerDocument = document;
    this.id = id;
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.attributes = {};
    this.classList = new ClassList();
    this.style = new Style();
    this.hidden = false;
    this.tabIndex = 0;
    this.textContent = '';
    this.className = '';
    this.type = '';
  }

  append(...children) {
    children.forEach((child) => this.appendChild(child));
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  remove() {
    if (!this.parentElement) return;
    this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
    this.parentElement = null;
  }

  contains(node) {
    return node === this || this.children.some((child) => child.contains(node));
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }

  getAttribute(name) {
    return Object.hasOwn(this.attributes, name) ? this.attributes[name] : null;
  }

  addEventListener(name, listener) {
    (this.listeners[name] ||= []).push(listener);
  }

  emit(name, event = {}) {
    for (const listener of this.listeners[name] || []) {
      listener({target: this, preventDefault() {}, stopPropagation() {}, ...event});
    }
  }

  focus() {
    this.ownerDocument.activeElement = this;
  }

  matches(selector) {
    if (selector.startsWith('#')) return this.id === selector.slice(1);
    if (selector === '[role="option"]') return this.attributes.role === 'option';
    return false;
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }

  querySelectorAll(selector) {
    const matches = [];
    const visit = (node) => {
      node.children.forEach((child) => {
        if (child.matches(selector)) matches.push(child);
        visit(child);
      });
    };
    visit(this);
    return matches;
  }
}

function setLocation(window, value) {
  const [beforeHash, hash = ''] = String(value).split('#', 2);
  const [pathname, search = ''] = beforeHash.split('?', 2);
  window.location.pathname = pathname || '/app/entities';
  window.location.search = search ? `?${search}` : '';
  window.location.hash = hash ? `#${hash}` : '';
}

function findById(root, id) {
  if (root.id === id) return root;
  for (const child of root.children) {
    const found = findById(child, id);
    if (found) return found;
  }
  return null;
}

function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) throw new Error('manifest directory required');
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/entities/workspace.html'), 'utf8');
  check(!workspace.includes('window.selectedFacet'), 'Entities does not read the shared selected facet');
  check(!workspace.includes('window.facetsData'), 'Entities does not read the shared facet roster');
  check(!workspace.includes('facet.switch'), 'Entities does not register the retired facet event');

  const scriptStart = workspace.lastIndexOf('<script>') + '<script>'.length;
  const scriptEnd = workspace.lastIndexOf('</script>');
  new vm.Script(workspace.slice(scriptStart, scriptEnd), {filename: 'entities-workspace.js'});
  check(scriptStart >= '<script>'.length && scriptEnd > scriptStart, 'the complete Entities workspace script parses');

  const start = workspace.indexOf('let ENT_COPY = {};');
  const scopeEnd = workspace.indexOf('function showFacetDetailView', start);
  const metadataStart = workspace.indexOf('function adoptJournalSummary', start);
  const metadataEnd = workspace.indexOf('// Edit form submission', metadataStart);
  check(start !== -1 && scopeEnd !== -1 && metadataStart !== -1 && metadataEnd !== -1, 'workspace exposes scope implementation boundaries');

  const documentListeners = {};
  const document = {
    activeElement: null,
    body: null,
    createElement() { return new Element(document); },
    getElementById(id) { return findById(document.body, id); },
    querySelector(selector) { return document.body.querySelector(selector); },
    addEventListener(name, listener) { (documentListeners[name] ||= []).push(listener); },
    removeEventListener(name, listener) {
      documentListeners[name] = (documentListeners[name] || []).filter((candidate) => candidate !== listener);
    },
  };
  document.body = new Element(document, 'body');
  const appendRoot = (id) => {
    const node = new Element(document, id);
    document.body.appendChild(node);
    return node;
  };
  appendRoot('entities-scope-anchor');
  appendRoot('entities-list-region');
  appendRoot('journal-entities-view');
  appendRoot('journal-entity-detail-view');
  appendRoot('entities-list-view');
  appendRoot('entity-detail-view');
  const emptyView = appendRoot('facet-entities-empty-view');
  emptyView.append(
    new Element(document, 'facet-entities-empty-title'),
    new Element(document, 'facet-entities-empty-body'),
    new Element(document, 'facet-entities-empty-action'),
  );

  const windowListeners = {};
  const window = {
    location: {pathname: '/app/entities', search: '', hash: ''},
    history: {
      pushed: [],
      replaced: [],
      pushState(_state, _title, value) {
        this.pushed.push(value);
        setLocation(window, value);
      },
      replaceState(_state, _title, value) {
        this.replaced.push(value);
        setLocation(window, value);
      },
    },
    addEventListener(name, listener) { (windowListeners[name] ||= []).push(listener); },
    emit(name) { (windowListeners[name] || []).forEach((listener) => listener()); },
    logError() {},
    SurfaceState: {replaceLoading() {}, error() {}},
    CONVEY_COPY: {RELOAD_HINT: ''},
  };
  window.window = window;
  const context = {
    window,
    document,
    URLSearchParams,
    history: window.history,
    location: window.location,
    console,
  };
  const scopeSource = `${workspace.slice(start, scopeEnd)}\nfunction loadEntities() { window.__scopeLoads = (window.__scopeLoads || 0) + 1; showListView(); }`;
  vm.runInNewContext(scopeSource, context, {filename: 'entities-scope.js'});
  // The list-state key builder lives in the paged-list region, outside both slices; the scope
  // harness only needs adoptJournalSummary to record a key, not to compute one.
  context.journalListStateKey = () => 'harness';
  vm.runInNewContext(workspace.slice(metadataStart, metadataEnd), context, {filename: 'entities-metadata.js'});
  vm.runInNewContext(`
    window.__entities = {
      setRoster(value) { facetRoster = value; },
      setJournal(summary) { adoptJournalSummary(summary); },
      restore: restoreEntityScopeFromLocation,
      render: renderScopeControl,
      showList: showListView,
      currentFacet: () => currentFacet,
      count: facetCount,
    };
  `, context, {filename: 'entities-exports.js'});
  const entities = window.__entities;
  const copy = {
    ENT_SCOPE_SHOWING: 'showing',
    ENT_SCOPE_WHOLE_JOURNAL: 'your whole journal',
    ENT_SCOPE_EMPTY_TITLE: 'nothing in {facet} yet.',
    ENT_SCOPE_EMPTY_BODY: 'entities show up here as your journal holds people, places and projects in this facet.',
    ENT_SCOPE_EMPTY_ACTION: 'show your whole journal',
    ENT_SCOPE_FACET_MISSING: "that facet isn't in your journal any more, so this is your whole journal.",
  };
  vm.runInNewContext(`ENT_COPY = ${JSON.stringify(copy)};`, context, {filename: 'entities-copy.js'});

  entities.setRoster([
    {name: 'work', title: 'Work', color: '#2f6fdd'},
    {name: 'personal', title: 'Personal', color: '#a23f7b'},
  ]);
  // Counts come from the paged summary read; the route excludes blocked and
  // detached memberships server-side (see solstone-core-entities router tests).
  entities.setJournal({facet_counts: {work: 1}});
  entities.restore();
  const control = document.getElementById('entities-scope-control');
  check(control !== null, 'scope control is present when the roster is non-empty');
  const options = control.querySelectorAll('[role="option"]');
  check(options.length === 3, 'scope menu includes whole journal and every roster facet');
  check(
    JSON.stringify(options.map((option) => [option.children[0].textContent, option.children[1]?.textContent || '']))
      === JSON.stringify([['your whole journal', ''], ['Work', '1'], ['Personal', '0']]),
    'scope control shows counts from the journal summary and includes zero-entity facets',
  );

  setLocation(window, '/app/entities?facet=work');
  entities.restore();
  entities.showList();
  check(entities.currentFacet() === 'work' && document.getElementById('entities-list-view').style.display === 'block', 'facet query selects the facet-scoped list');
  check(document.getElementById('entities-list-region').style.getPropertyValue('--entities-facet-color') === '#2f6fdd', 'facet tint stays on the local list region');

  setLocation(window, '/app/entities');
  entities.restore();
  entities.showList();
  check(entities.currentFacet() === null && document.getElementById('journal-entities-view').style.display === 'block', 'plain Entities URL selects the journal-wide list');

  setLocation(window, '/app/entities?facet=personal');
  entities.restore();
  entities.showList();
  check(document.getElementById('facet-entities-empty-view').style.display === 'block', 'zero-entity facets use the page-level empty state');
  check(
    document.getElementById('facet-entities-empty-title').textContent === 'nothing in Personal yet.'
      && document.getElementById('facet-entities-empty-body').textContent === copy.ENT_SCOPE_EMPTY_BODY
      && document.getElementById('facet-entities-empty-action').textContent === copy.ENT_SCOPE_EMPTY_ACTION,
    'page-level empty state uses its own scoped copy',
  );

  setLocation(window, '/app/entities?facet=work');
  entities.restore();
  const journalOption = document.getElementById('entities-scope-control').querySelectorAll('[role="option"]')[0];
  journalOption.emit('click');
  check(window.history.pushed.at(-1) === '/app/entities' && window.location.search === '', 'scope selection pushes the journal-wide query state');
  setLocation(window, '/app/entities?facet=work');
  window.emit('popstate');
  check(entities.currentFacet() === 'work' && document.getElementById('entities-list-view').style.display === 'block', 'popstate restores the prior scoped list');
  check((window.__scopeLoads || 0) >= 2, 'scope selections and popstate rerender through the page loader');

  setLocation(window, '/app/entities?facet=missing');
  entities.restore();
  check(window.location.search === '' && entities.currentFacet() === null, 'unknown facets canonicalize to the journal-wide URL');
  check(
    document.getElementById('entities-scope-notice').textContent === copy.ENT_SCOPE_FACET_MISSING,
    'unknown facets show the stale-facet notice',
  );

  entities.setRoster([]);
  entities.setJournal({facet_counts: {}});
  entities.render();
  check(document.getElementById('entities-scope-control') === null, 'scope control and popover are removed when the roster is empty');

  console.log(`DOM CASES: ${cases} passed`);
}

try {
  main();
} catch (error) {
  console.error(error.stack || error);
  process.exitCode = 1;
}
