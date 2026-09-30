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
    const present = force === undefined ? !this.values.has(name) : Boolean(force);
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

function createDatasetProxy(element) {
  return new Proxy({}, {
    get(target, prop) {
      if (typeof prop === 'symbol') return target[prop];
      const attrName = 'data-' + prop.replace(/[A-Z]/g, (letter) => '-' + letter.toLowerCase());
      return element.hasAttribute(attrName) ? element.getAttribute(attrName) : undefined;
    },
    set(target, prop, value) {
      if (typeof prop === 'symbol') {
        target[prop] = value;
        return true;
      }
      const attrName = 'data-' + prop.replace(/[A-Z]/g, (letter) => '-' + letter.toLowerCase());
      element.setAttribute(attrName, String(value));
      return true;
    },
    deleteProperty(target, prop) {
      if (typeof prop === 'symbol') {
        delete target[prop];
        return true;
      }
      const attrName = 'data-' + prop.replace(/[A-Z]/g, (letter) => '-' + letter.toLowerCase());
      element.removeAttribute(attrName);
      return true;
    },
  });
}

function parseMockHtmlForWorkspace(html, doc) {
  const frag = new Element('div', doc);
  const tagRegex = /<(\/)?([a-zA-Z0-9]+)([^>]*)>|([^<]+)/g;
  let match;
  let currentParent = frag;
  const stack = [frag];

  while ((match = tagRegex.exec(html)) !== null) {
    const isClosing = match[1] === '/';
    const tagName = match[2];
    const rawAttrs = match[3];
    const textContent = match[4];

    if (textContent) {
      currentParent._textContent += textContent;
    } else if (isClosing) {
      if (stack.length > 1) {
        stack.pop();
        currentParent = stack[stack.length - 1];
      }
    } else {
      const elem = new Element(tagName, doc);
      if (rawAttrs) {
        const attrRegex = /([a-zA-Z0-9_\-]+)(?:=(?:"([^"]*)"|'([^']*)'|([^>\s]+)))?/g;
        let attrMatch;
        while ((attrMatch = attrRegex.exec(rawAttrs)) !== null) {
          const k = attrMatch[1];
          const v = attrMatch[2] ?? attrMatch[3] ?? attrMatch[4] ?? '';
          elem.setAttribute(k, v);
        }
      }
      currentParent.appendChild(elem);
      if (!['img', 'br', 'hr', 'input', 'source'].includes(tagName.toLowerCase())) {
        stack.push(elem);
        currentParent = elem;
      }
    }
  }
  return frag.children;
}

class Element {
  constructor(tagName, ownerDocument) {
    this.tagName = tagName.toUpperCase();
    this.ownerDocument = ownerDocument;
    this.attributes = {};
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.dataset = createDatasetProxy(this);
    this.classList = new ClassList(this);
    this.style = {};
    this.value = '';
    this.disabled = false;
    this.isContentEditable = false;
    this._textContent = '';
    this.clientHeight = 0;
    this.clientWidth = 0;
  }

  get id() { return this.getAttribute('id') || ''; }
  set id(value) { this.setAttribute('id', value); }
  get href() { return this.getAttribute('href') || ''; }
  set href(value) { this.setAttribute('href', value); }
  get className() { return this.getAttribute('class') || ''; }
  set className(value) { this.setAttribute('class', value); }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(value) { if (value) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  get firstChild() { return this.children[0] || null; }

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

  get innerHTML() {
    return this._innerHTML || this.textContent;
  }
  set innerHTML(value) {
    this._innerHTML = String(value);
    this.children = [];
    this._textContent = '';
    if (value && this.ownerDocument) {
      const parsedChildren = parseMockHtmlForWorkspace(value, this.ownerDocument);
      parsedChildren.forEach((child) => this.appendChild(child));
    }
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'class') this.classList.setFromString(value);
  }

  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  hasAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attributes, name); }

  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'class') this.classList.setFromString('');
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  insertBefore(newNode, referenceNode) {
    newNode.parentElement = this;
    const idx = this.children.indexOf(referenceNode);
    if (idx >= 0) this.children.splice(idx, 0, newNode);
    else this.children.push(newNode);
    return newNode;
  }

  append(...children) { children.forEach((child) => this.appendChild(child)); }

  remove() {
    if (this.parentElement) {
      const idx = this.parentElement.children.indexOf(this);
      if (idx >= 0) this.parentElement.children.splice(idx, 1);
      this.parentElement = null;
    }
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

  contains(other) {
    let current = other;
    while (current) {
      if (current === this) return true;
      current = current.parentElement;
    }
    return false;
  }

  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
  removeEventListener(type, listener) {
    if (this.listeners[type]) {
      this.listeners[type] = this.listeners[type].filter((l) => l !== listener);
    }
  }

  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    for (const listener of [...(this.listeners[event.type] || [])]) listener.call(this, event);
    if (event.bubbles && !event.cancelBubble && this.parentElement) this.parentElement.dispatchEvent(event);
    return !event.defaultPrevented;
  }

  focus() { this.ownerDocument.activeElement = this; }
  getBoundingClientRect() {
    return { top: 0, left: 0, bottom: this.clientHeight, right: this.clientWidth, width: this.clientWidth, height: this.clientHeight };
  }
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
  removeEventListener(type, listener) {
    if (this.listeners[type]) {
      this.listeners[type] = this.listeners[type].filter((l) => l !== listener);
    }
  }
  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    for (const listener of [...(this.listeners[event.type] || [])]) listener.call(this, event);
    return !event.defaultPrevented;
  }
  contains(node) {
    return this.documentElement.contains(node);
  }
}

class Location {
  constructor(initialUrl) {
    this._url = new URL(initialUrl);
  }
  get href() { return this._url.href; }
  set href(v) { this._url = new URL(v, this._url.origin); }
  get pathname() { return this._url.pathname; }
  set pathname(v) { this._url.pathname = v; }
  get search() { return this._url.search; }
  set search(v) { this._url.search = v; }
  get hash() { return this._url.hash; }
  set hash(v) { this._url.hash = v; }
  get searchParams() { return this._url.searchParams; }
  get origin() { return this._url.origin; }
  toString() { return this.href; }
}

class History {
  constructor(initialUrl, windowRef) {
    this.stack = [initialUrl];
    this.index = 0;
    this.window = windowRef;
    this.popstateCount = 0;
    this.hashchangeCount = 0;
    this._syncLocation();
  }

  get length() { return this.stack.length; }

  _syncLocation() {
    this.window.location = new Location(this.stack[this.index]);
  }

  pushState(state, title, url) {
    const resolved = url ? new URL(url, this.window.location.origin).href : this.stack[this.index];
    this.stack = this.stack.slice(0, this.index + 1);
    this.stack.push(resolved);
    this.index = this.stack.length - 1;
    this._syncLocation();
  }

  replaceState(state, title, url) {
    const resolved = url ? new URL(url, this.window.location.origin).href : this.stack[this.index];
    this.stack[this.index] = resolved;
    this._syncLocation();
  }

  back() {
    if (this.index > 0) {
      const oldHash = new URL(this.stack[this.index]).hash;
      this.index--;
      const newUrl = this.stack[this.index];
      const newHash = new URL(newUrl).hash;
      this._syncLocation();
      this.popstateCount++;
      this.window.dispatchEvent({ type: 'popstate' });
      if (oldHash !== newHash) {
        setTimeout(() => {
          this.hashchangeCount++;
          this.window.dispatchEvent({ type: 'hashchange' });
        }, 0);
      }
    }
  }

  forward() {
    if (this.index < this.stack.length - 1) {
      const oldHash = new URL(this.stack[this.index]).hash;
      this.index++;
      const newUrl = this.stack[this.index];
      const newHash = new URL(newUrl).hash;
      this._syncLocation();
      this.popstateCount++;
      this.window.dispatchEvent({ type: 'popstate' });
      if (oldHash !== newHash) {
        setTimeout(() => {
          this.hashchangeCount++;
          this.window.dispatchEvent({ type: 'hashchange' });
        }, 0);
      }
    }
  }
}

function loadTranscriptsScript() {
  const html = fs.readFileSync(path.join(crateDir, 'assets/transcripts/workspace.html'), 'utf8');
  const startTag = '<script>';
  const endTag = '</script>';
  const startIdx = html.indexOf(startTag);
  assert.ok(startIdx >= 0, '<script> tag found in workspace.html');
  const endIdx = html.indexOf(endTag, startIdx);
  assert.ok(endIdx > startIdx, '</script> tag found in workspace.html');
  return html.slice(startIdx + startTag.length, endIdx);
}

const scriptSource = loadTranscriptsScript();
const apiSource = fs.readFileSync(path.join(crateDir, '../solstone-core-convey-shell/assets/static/api.js'), 'utf8');

function createTranscriptsDOM() {
  const doc = new Document();

  const timeline = doc.createElement('div');
  timeline.id = 'trTimeline';
  timeline.clientHeight = 800;
  timeline.clientWidth = 1000;
  doc.body.appendChild(timeline);

  const grid = doc.createElement('div');
  grid.id = 'trGrid';
  timeline.appendChild(grid);

  const labels = doc.createElement('div');
  labels.id = 'trLabels';
  timeline.appendChild(labels);

  const segmentsLane = doc.createElement('div');
  segmentsLane.id = 'trSegments';
  timeline.appendChild(segmentsLane);

  const bodyEventsLane = doc.createElement('div');
  bodyEventsLane.id = 'trBodyEvents';
  timeline.appendChild(bodyEventsLane);

  const selWrap = doc.createElement('div');
  selWrap.id = 'trSelWrap';
  const sel = doc.createElement('div');
  sel.className = 'tr-sel';
  const handleStart = doc.createElement('div');
  handleStart.setAttribute('data-handle', 'start');
  const handleEnd = doc.createElement('div');
  handleEnd.setAttribute('data-handle', 'end');
  sel.appendChild(handleStart);
  sel.appendChild(handleEnd);
  selWrap.appendChild(sel);
  timeline.appendChild(selWrap);

  const zoom = doc.createElement('div');
  zoom.id = 'trZoom';
  zoom.clientHeight = 800;
  zoom.clientWidth = 1000;
  doc.body.appendChild(zoom);

  const zoomGrid = doc.createElement('div');
  zoomGrid.id = 'trZoomGrid';
  zoom.appendChild(zoomGrid);

  const zoomLabels = doc.createElement('div');
  zoomLabels.id = 'trZoomLabels';
  zoom.appendChild(zoomLabels);

  const zoomSegments = doc.createElement('div');
  zoomSegments.id = 'trZoomSegments';
  zoom.appendChild(zoomSegments);

  const titleEl = doc.createElement('div');
  titleEl.className = 'tr-title';
  doc.body.appendChild(titleEl);

  const rangeText = doc.createElement('div');
  rangeText.id = 'trRangeText';
  doc.body.appendChild(rangeText);

  const tabsContainer = doc.createElement('div');
  tabsContainer.id = 'trTabs';
  doc.body.appendChild(tabsContainer);

  const panel = doc.createElement('div');
  panel.id = 'trPanel';
  doc.body.appendChild(panel);

  const warningNotice = doc.createElement('div');
  warningNotice.id = 'trWarningNotice';
  const warningSummary = doc.createElement('div');
  warningSummary.className = 'tr-warning-summary';
  const warningText = doc.createElement('span');
  warningText.id = 'trWarningText';
  const warningCaret = doc.createElement('span');
  warningCaret.id = 'trWarningCaret';
  warningSummary.appendChild(warningText);
  warningSummary.appendChild(warningCaret);
  const warningDetails = doc.createElement('ul');
  warningDetails.id = 'trWarningDetails';
  warningNotice.appendChild(warningSummary);
  warningNotice.appendChild(warningDetails);
  doc.body.appendChild(warningNotice);

  const deleteBtn = doc.createElement('button');
  deleteBtn.id = 'trDeleteBtn';
  doc.body.appendChild(deleteBtn);

  const bodyPanel = doc.createElement('div');
  bodyPanel.id = 'trBodyPanel';
  const bodyPanelTitle = doc.createElement('div');
  bodyPanelTitle.id = 'trBodyPanelTitle';
  const bodyPanelRange = doc.createElement('div');
  bodyPanelRange.id = 'trBodyPanelRange';
  const bodyPanelBody = doc.createElement('div');
  bodyPanelBody.id = 'trBodyPanelBody';
  const bodyPanelClose = doc.createElement('button');
  bodyPanelClose.id = 'trBodyPanelClose';
  bodyPanel.appendChild(bodyPanelTitle);
  bodyPanel.appendChild(bodyPanelRange);
  bodyPanel.appendChild(bodyPanelBody);
  bodyPanel.appendChild(bodyPanelClose);
  doc.body.appendChild(bodyPanel);

  const deleteSegmentModal = doc.createElement('div');
  deleteSegmentModal.id = 'trDeleteSegmentModal';
  deleteSegmentModal.style.display = 'none';
  const deleteSegmentModalBody = doc.createElement('div');
  deleteSegmentModalBody.id = 'trDeleteSegmentModalBody';
  const deleteSegmentModalConfirm = doc.createElement('button');
  deleteSegmentModalConfirm.id = 'trDeleteSegmentModalConfirm';
  deleteSegmentModal.appendChild(deleteSegmentModalBody);
  deleteSegmentModal.appendChild(deleteSegmentModalConfirm);
  doc.body.appendChild(deleteSegmentModal);

  const initialEmptyIcon = doc.createElement('div');
  initialEmptyIcon.id = 'trInitialEmptyIcon';
  doc.body.appendChild(initialEmptyIcon);

  return doc;
}

class MockStorage {
  constructor() { this.store = {}; }
  getItem(k) { return Object.prototype.hasOwnProperty.call(this.store, k) ? this.store[k] : null; }
  setItem(k, v) { this.store[k] = String(v); }
  removeItem(k) { delete this.store[k]; }
}

async function createEnvironment(
  initialUrl = 'https://journal.example/app/transcripts/20200115?ref=keep',
  initialAudioState = 'analyzed',
  ignoreSegmentAbort = false,
  segments = null
) {
  const doc = createTranscriptsDOM();
  const storage = new MockStorage();
  const notifications = [];
  const intervals = new Map();
  let nextIntervalId = 1;

  const windowListeners = {};
  const windowObj = {
    addEventListener(type, listener) { (windowListeners[type] ||= []).push(listener); },
    removeEventListener(type, listener) {
      if (windowListeners[type]) {
        windowListeners[type] = windowListeners[type].filter((l) => l !== listener);
      }
    },
    dispatchEvent(event) {
      event.target ||= windowObj;
      event.currentTarget = windowObj;
      for (const listener of [...(windowListeners[event.type] || [])]) listener.call(windowObj, event);
      return !event.defaultPrevented;
    },
    solPathContext: () => ({ day: '20200115' }),
    AppServices: {
      escapeHtml: (s) => String(s ?? ''),
      renderMarkdown: (s) => String(s ?? ''),
      notifications: {
        show: (notice) => { notifications.push(notice); return notifications.length; },
        dismiss: () => {},
      },
    },
    SurfaceState: {
      loading: () => '<div class="surface-state surface-state--loading"></div>',
      empty: () => '<div class="surface-state surface-state--empty"></div>',
      error: () => '<div class="surface-state surface-state--error"></div>',
    },
    JournalFormat: {
      stream: (s) => String(s ?? ''),
    },
    ApiError: class ApiError extends Error {
      constructor(opts = {}) {
        super(opts.serverMessage || 'ApiError');
        Object.assign(this, opts);
      }
    },
    ConveyIcons: {
      svg: (name) => `<svg data-icon="${name}"></svg>`,
    },
    CONVEY_COPY: {},
    localStorage: storage,
    location: null,
    history: null,
  };

  const history = new History(initialUrl, windowObj);
  windowObj.history = history;

  const segmentGetRequests = [];

  const dayPayload = {
    audio: [{ start: '00:30', end: '23:30', streams: ['room', 'desk'], state: 'analyzed' }],
    screen: [],
    segments: segments || [
      { key: '114500_300', stream: 'room', start: '11:45', end: '11:50', types: ['audio'], data_state: { audio: 'analyzed' } },
      { key: '115500_300', stream: 'room', start: '11:55', end: '12:00', types: ['audio'], data_state: { audio: 'analyzed' } },
      { key: '114500_300', stream: 'desk', start: '11:45', end: '11:50', types: ['audio'], data_state: { audio: 'analyzed' } },
    ],
  };

  const mockFetch = (url, options = {}) => {
    const urlStr = String(url);
    const signal = options.signal;

    if (options.method === 'DELETE') {
      return Promise.resolve({
        ok: true,
        status: 200,
        json: async () => ({ pending: 'ok', ttl_seconds: 0 }),
      });
    }

    if (urlStr.includes('/app/transcripts/api/delete-status/')) {
      return Promise.resolve({
        ok: true,
        status: 200,
        json: async () => ({ state: 'deleted' }),
      });
    }

    if (urlStr.includes('/app/transcripts/api/delete-outcomes')) {
      return Promise.resolve({
        ok: true,
        status: 200,
        json: async () => ({ outcomes: [] }),
      });
    }

    if (urlStr.includes('/app/transcripts/api/day/')) {
      return Promise.resolve({
        ok: true,
        status: 200,
        json: async () => dayPayload,
      });
    }

    if (urlStr.includes('/app/transcripts/api/ranges/')) {
      return Promise.resolve({
        ok: true,
        status: 200,
        json: async () => ({ audio: dayPayload.audio, screen: [] }),
      });
    }

    if (urlStr.includes('/app/transcripts/api/segment/') && urlStr.includes('/body-window')) {
      return Promise.resolve({
        ok: true,
        status: 200,
        json: async () => ({ has_data: false }),
      });
    }

    if (urlStr.includes('/app/transcripts/api/day/') && urlStr.includes('/body-window')) {
      return Promise.resolve({
        ok: true,
        status: 200,
        json: async () => ({ has_data: false }),
      });
    }

    if (urlStr.includes('/app/transcripts/api/segment/')) {
      let resolveDeferred;
      let rejectDeferred;
      const promise = new Promise((resolve, reject) => {
        resolveDeferred = resolve;
        rejectDeferred = reject;
      });

      const reqRecord = {
        url: urlStr,
        fulfilled: false,
        aborted: false,
        resolve: (audioState = initialAudioState, payloadOverride) => {
          if (!reqRecord.fulfilled && !reqRecord.aborted) {
            reqRecord.fulfilled = true;
            resolveDeferred({
              ok: true,
              status: 200,
              json: async () => ({
                chunks: payloadOverride?.chunks ?? [],
                data_state: { audio: audioState },
                signals: { events: [] },
                speaker_labels: payloadOverride?.speaker_labels,
                transcripts_copy: {},
              }),
            });
          }
        },
      };

      // A completed transport can deliver after selection changes.
      if (signal && !ignoreSegmentAbort) {
        if (signal.aborted) {
          reqRecord.aborted = true;
          const err = new Error('The user aborted a request.');
          err.name = 'AbortError';
          rejectDeferred(err);
        } else {
          signal.addEventListener('abort', () => {
            if (!reqRecord.fulfilled && !reqRecord.aborted) {
              reqRecord.aborted = true;
              const err = new Error('The user aborted a request.');
              err.name = 'AbortError';
              rejectDeferred(err);
            }
          });
        }
      }

      segmentGetRequests.push(reqRecord);
      return promise;
    }

    return Promise.resolve({
      ok: true,
      status: 200,
      json: async () => ({}),
    });
  };

  class MockResizeObserver {
    constructor(cb) { this.cb = cb; }
    observe() {}
    unobserve() {}
    disconnect() {}
  }

  class MockMutationObserver {
    constructor(cb) { this.cb = cb; }
    observe() {}
    disconnect() {}
  }

  const silentConsole = {
    log: () => {},
    warn: () => {},
    error: () => {},
    info: () => {},
  };

  const sandbox = {
    window: windowObj,
    document: doc,
    history,
    location: windowObj.location,
    fetch: async (...args) => {
      const response = await mockFetch(...args);
      return new Response(JSON.stringify(await response.json()), { status: response.status || 200 });
    },
    URL,
    URLSearchParams,
    AbortController,
    ResizeObserver: MockResizeObserver,
    MutationObserver: MockMutationObserver,
    setTimeout,
    clearTimeout,
    setInterval: (callback) => {
      const id = nextIntervalId++;
      intervals.set(id, callback);
      return id;
    },
    clearInterval: (id) => intervals.delete(id),
    Date,
    Math,
    Number,
    String,
    Array,
    Object,
    Set,
    Map,
    JSON,
    Promise,
    TypeError,
    Error,
    Infinity,
    console: silentConsole,
    encodeURIComponent,
  };

  const context = vm.createContext(sandbox);
  vm.runInContext(apiSource, context);
  vm.runInContext(scriptSource, context);

  // Flush microtasks for day load
  await new Promise((r) => setTimeout(r, 10));

  return {
    doc,
    window: windowObj,
    history,
    notifications,
    intervals,
    segmentGetRequests,
    flushHashchanges: async () => {
      await new Promise((r) => setTimeout(r, 0));
    },
    releasePendingSegmentGets: async (audioState, payloadOverride) => {
      segmentGetRequests.forEach((req) => req.resolve(audioState, payloadOverride));
      await new Promise((r) => setTimeout(r, 10));
    },
  };
}

function getZoomPills(doc) {
  return doc.querySelectorAll('#trZoomSegments .tr-zoom-pill');
}

function getActiveTab(doc) {
  const activeTabBtn = doc.querySelector('#trTabs .tr-tab.active');
  return activeTabBtn ? activeTabBtn.dataset.tab : null;
}

function isDeleteBtnVisible(doc) {
  return doc.querySelector('#trDeleteBtn').classList.contains('visible');
}

const tests = [];
function test(name, fn) {
  tests.push({ name, fn });
}

// 1. Post-load length is the baseline. Click A's pill (first 114500_300), release the segment GET.
// Click B's pill (115500_300), release. Length is baseline+2. back, flush hashchange, release:
// A selected (visible, hash key 114500_300, search contains stream=room). back again, flush hashchange:
// not visible, no aria-selected="true" tab, location is /app/transcripts/20200115?ref=keep with empty hash.
// forward, flush hashchange, release: A selected again.
test('case 1: selection pushes history and navigating back reaches day view', async () => {
  const env = await createEnvironment();
  const baseline = env.history.length;

  const pills = getZoomPills(env.doc);
  assert.strictEqual(pills.length, 3);
  const pillA = pills[0]; // first 114500_300 (room)
  const pillB = pills[1]; // 115500_300 (room)

  pillA.dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  assert.strictEqual(env.history.length, baseline + 1);

  pillB.dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  assert.strictEqual(env.history.length, baseline + 2);

  env.history.back();
  await env.flushHashchanges();
  await env.releasePendingSegmentGets();
  assert.strictEqual(isDeleteBtnVisible(env.doc), true);
  assert.ok(env.window.location.hash.startsWith('#114500_300'));
  assert.ok(env.window.location.search.includes('stream=room'));

  env.history.back();
  await env.flushHashchanges();
  assert.strictEqual(isDeleteBtnVisible(env.doc), false);
  const selectedTab = env.doc.querySelector('#trTabs .tr-tab[aria-selected="true"]');
  assert.strictEqual(selectedTab, null);
  assert.strictEqual(env.window.location.pathname, '/app/transcripts/20200115');
  assert.strictEqual(env.window.location.search, '?ref=keep');
  assert.strictEqual(env.window.location.hash, '');

  env.history.forward();
  await env.flushHashchanges();
  await env.releasePendingSegmentGets();
  assert.strictEqual(isDeleteBtnVisible(env.doc), true);
  assert.ok(env.window.location.hash.startsWith('#114500_300'));
});

// 2. Click A, release. Click the audio tab. Click B, release. back, flush hashchange while
// the segment GET is still pending (exactly one new segment GET so far), then release.
// Active tab is audio, not transcript.
test('case 2: back navigation preserves previously active tab', async () => {
  const env = await createEnvironment();
  const pills = getZoomPills(env.doc);

  pills[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();

  const audioTabBtn = env.doc.querySelector('#trTabs .tr-tab[data-tab="audio"]');
  assert.ok(audioTabBtn);
  audioTabBtn.dispatchEvent({ type: 'click' });
  assert.strictEqual(getActiveTab(env.doc), 'audio');

  pills[1].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();

  const getCountBeforeBack = env.segmentGetRequests.length;
  env.history.back();
  await env.flushHashchanges();

  assert.strictEqual(env.segmentGetRequests.length, getCountBeforeBack + 1);
  await env.releasePendingSegmentGets();
  assert.strictEqual(getActiveTab(env.doc), 'audio');
});

// 3. Click A, release. Click B, release. Note length. Click B's audio tab. Length unchanged.
// back, flush hashchange, release: A selected.
test('case 3: tab switches replace history and do not add entries', async () => {
  const env = await createEnvironment();
  const pills = getZoomPills(env.doc);

  pills[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  pills[1].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();

  const lengthBeforeTab = env.history.length;
  const audioTabBtn = env.doc.querySelector('#trTabs .tr-tab[data-tab="audio"]');
  assert.ok(audioTabBtn);
  audioTabBtn.dispatchEvent({ type: 'click' });
  assert.strictEqual(env.history.length, lengthBeforeTab);

  env.history.back();
  await env.flushHashchanges();
  await env.releasePendingSegmentGets();
  assert.ok(env.window.location.hash.startsWith('#114500_300'));
});

// 4. Click the first 114500_300 pill, release. Click that same pill again, release. Length unchanged
// from after the first click. Click the later 114500_300 pill, release. Length grew by 1.
// Search contains stream=desk. back: fragment unchanged, so hashchange count for this back is 0.
// Exactly one new segment GET and its URL contains /room/. Release. Delete button visible and
// search contains stream=room.
test('case 4: same key across different streams disambiguates and fires popstate without hashchange', async () => {
  const env = await createEnvironment();
  const pills = getZoomPills(env.doc);
  const pillA = pills[0]; // 114500_300 room
  const pillTwin = pills[2]; // 114500_300 desk

  pillA.dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  const lengthAfterFirst = env.history.length;

  pillA.dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  assert.strictEqual(env.history.length, lengthAfterFirst);

  pillTwin.dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  assert.strictEqual(env.history.length, lengthAfterFirst + 1);
  assert.ok(env.window.location.search.includes('stream=desk'));

  const hashchangesBeforeBack = env.history.hashchangeCount;
  const getsBeforeBack = env.segmentGetRequests.length;
  env.history.back();
  await env.flushHashchanges();

  assert.strictEqual(env.history.hashchangeCount, hashchangesBeforeBack);
  assert.strictEqual(env.segmentGetRequests.length, getsBeforeBack + 1);
  assert.ok(env.segmentGetRequests[getsBeforeBack].url.includes('/room/'));

  await env.releasePendingSegmentGets();
  assert.strictEqual(isDeleteBtnVisible(env.doc), true);
  assert.ok(env.window.location.search.includes('stream=room'));
});

// 5. Boot with https://journal.example/app/transcripts/20200115?ref=keep&stream=room#114500_300/audio.
// After the day settles, release the one segment GET. Delete button visible, search contains stream=room,
// hash is #114500_300/audio, active tab audio. history.length still 1.
test('case 5: deep link boot selects segment without adding history entry', async () => {
  const env = await createEnvironment(
    'https://journal.example/app/transcripts/20200115?ref=keep&stream=room#114500_300/audio'
  );

  await env.releasePendingSegmentGets();
  assert.strictEqual(isDeleteBtnVisible(env.doc), true);
  assert.ok(env.window.location.search.includes('stream=room'));
  assert.strictEqual(env.window.location.hash, '#114500_300/audio');
  assert.strictEqual(getActiveTab(env.doc), 'audio');
  assert.strictEqual(env.history.length, 1);
});

// 6. Real delete path. Click A, release. Click B, release. Click A again, release. Length is baseline+3.
// Click #trDeleteBtn, then await window.confirmDeleteSegment(). Flush the real 500ms wait and the
// delete-status fetch before backing up. back, flush hashchange, release: B selected (key 115500_300).
// back again, flush hashchange: not visible, no selected tab, and the location still has hash
// #114500_300/ and search stream=room (the earlier A entry was not rewritten).
test('case 6: delete flow navigation preserves past entries and leaves unselected on deleted segment', async () => {
  const env = await createEnvironment();
  const baseline = env.history.length;
  const pills = getZoomPills(env.doc);

  pills[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  pills[1].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  pills[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  assert.strictEqual(env.history.length, baseline + 3);

  const deleteBtn = env.doc.querySelector('#trDeleteBtn');
  deleteBtn.dispatchEvent({ type: 'click' });
  await env.window.confirmDeleteSegment();

  // Wait for 500ms delete outcome polling
  await new Promise((r) => setTimeout(r, 600));

  env.history.back();
  await env.flushHashchanges();
  await env.releasePendingSegmentGets();
  assert.strictEqual(isDeleteBtnVisible(env.doc), true);
  assert.ok(env.window.location.hash.startsWith('#115500_300'));

  env.history.back();
  await env.flushHashchanges();
  assert.strictEqual(isDeleteBtnVisible(env.doc), false);
  const selectedTab = env.doc.querySelector('#trTabs .tr-tab[aria-selected="true"]');
  assert.strictEqual(selectedTab, null);
  assert.ok(env.window.location.hash.startsWith('#114500_300'));
  assert.ok(env.window.location.search.includes('stream=room'));
});

// 7. Click A, release. Click B, release. Record segment-GET count. back, flush the hashchange macrotask
// BEFORE releasing the segment promise. The new segment-GET count is exactly 1. Hashchange dispatched once
// for this back, and popstate already ran. Then release. Active tab is transcript (the hash tab).
test('case 7: hashchange macrotask does not duplicate in-flight fetch', async () => {
  const env = await createEnvironment();
  const pills = getZoomPills(env.doc);

  pills[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  pills[1].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();

  const getCount = env.segmentGetRequests.length;
  const popstateBefore = env.history.popstateCount;
  const hashchangeBefore = env.history.hashchangeCount;

  env.history.back();
  await env.flushHashchanges(); // Flush hashchange macrotask before releasing promise

  assert.strictEqual(env.history.popstateCount, popstateBefore + 1);
  assert.strictEqual(env.history.hashchangeCount, hashchangeBefore + 1);
  assert.strictEqual(env.segmentGetRequests.length, getCount + 1);

  await env.releasePendingSegmentGets();
  assert.strictEqual(getActiveTab(env.doc), 'transcript');
});

// 8. Click A, release. Note length. Dispatch ] on document with target.tagName BODY
// (not INPUT or TEXTAREA), isContentEditable false, preventDefault. Length grew by 1.
// back, flush hashchange, release: A selected (key 114500_300, stream=room).
test('case 8: bracket keyboard navigation pushes history and restores on back', async () => {
  const env = await createEnvironment();
  const pills = getZoomPills(env.doc);

  pills[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  const lengthBefore = env.history.length;

  env.doc.dispatchEvent({
    type: 'keydown',
    key: ']',
    target: env.doc.body,
    preventDefault: () => {},
  });
  await env.releasePendingSegmentGets();
  assert.strictEqual(env.history.length, lengthBefore + 1);

  env.history.back();
  await env.flushHashchanges();
  await env.releasePendingSegmentGets();
  assert.ok(env.window.location.hash.startsWith('#114500_300'));
  assert.ok(env.window.location.search.includes('stream=room'));
});

// 9. Click A, release. Note length and popstate count. Set the location to B without
// going through history.back or pushState: search ?ref=keep&stream=room, hash #115500_300/transcript,
// href kept absolute. Do not fire popstate. Deliver hashchange via setTimeout(0). Flush that macrotask,
// release the segment GET. B selected (key 115500_300). Length unchanged. Popstate count unchanged.
test('case 9: standalone hashchange selects segment without popstate or stack growth', async () => {
  const env = await createEnvironment();
  const pills = getZoomPills(env.doc);

  pills[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();

  const lengthBefore = env.history.length;
  const popstateBefore = env.history.popstateCount;

  env.window.location.search = '?ref=keep&stream=room';
  env.window.location.hash = '#115500_300/transcript';

  setTimeout(() => {
    env.window.dispatchEvent({ type: 'hashchange' });
  }, 0);

  await env.flushHashchanges();
  await env.releasePendingSegmentGets();

  assert.strictEqual(isDeleteBtnVisible(env.doc), true);
  assert.ok(env.window.location.hash.startsWith('#115500_300'));
  assert.strictEqual(env.history.length, lengthBefore);
  assert.strictEqual(env.history.popstateCount, popstateBefore);
});

test('deleting one stream keeps its same-key sibling selectable', async () => {
  const env = await createEnvironment();
  getZoomPills(env.doc)[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  env.doc.querySelector('#trDeleteBtn').dispatchEvent({ type: 'click' });
  await env.window.confirmDeleteSegment();
  const remaining = getZoomPills(env.doc);
  assert.strictEqual(remaining.length, 2);
  remaining[1].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  assert.ok(env.window.location.search.includes('stream=desk'));
  assert.ok(env.segmentGetRequests.at(-1).url.includes('/desk/114500_300'));
});

test('cancelling restores the selected stream alongside its same-key sibling', async () => {
  const env = await createEnvironment();
  getZoomPills(env.doc)[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  env.doc.querySelector('#trDeleteBtn').dispatchEvent({ type: 'click' });
  await env.window.confirmDeleteSegment();
  const cancel = env.notifications.find(notice => notice.buttons?.length)?.buttons[0];
  assert.ok(cancel);
  await cancel.onClick();
  assert.strictEqual(getZoomPills(env.doc).length, 3);
});

test('no-speech analysis finishes polling and renders the terminal segment without navigation', async () => {
  const env = await createEnvironment(undefined, 'analyzing');
  getZoomPills(env.doc)[0].dispatchEvent({ type: 'click' });
  await env.releasePendingSegmentGets();
  assert.strictEqual(env.intervals.size, 1);
  assert.ok(env.doc.querySelector('#tr-tabpanel-transcript').innerHTML.includes('tr-analyzing-state'));
  const historyLength = env.history.length;
  const poll = [...env.intervals.values()][0]();
  env.segmentGetRequests.at(-1).resolve('empty');
  await poll;
  assert.strictEqual(env.intervals.size, 0);
  assert.ok(!env.doc.querySelector('#tr-tabpanel-transcript').innerHTML.includes('tr-analyzing-state'));
  assert.strictEqual(getActiveTab(env.doc), 'transcript');
  assert.strictEqual(env.history.length, historyLength);
  assert.strictEqual(isDeleteBtnVisible(env.doc), true);
});

test('a late same-key response from another stream cannot replace the current segment', async () => {
  const env = await createEnvironment(undefined, 'analyzed', true);
  getZoomPills(env.doc)[0].dispatchEvent({ type: 'click' });
  const oldRequest = env.segmentGetRequests.at(-1);
  getZoomPills(env.doc)[2].dispatchEvent({ type: 'click' });
  env.segmentGetRequests.at(-1).resolve();
  await new Promise(resolve => setTimeout(resolve, 10));
  oldRequest.resolve('analyzing');
  await new Promise(resolve => setTimeout(resolve, 10));
  assert.ok(env.window.location.search.includes('stream=desk'));
  assert.strictEqual(env.intervals.size, 0);
  assert.ok(!env.doc.querySelector('#tr-tabpanel-transcript').innerHTML.includes('tr-analyzing-state'));
});

test('source link corpus named segment redirect boots and selects segment', async () => {
  const corpusPath = path.join(__dirname, '../../solstone-core-convey-shell/tests/source_link_corpus.json');
  const corpus = JSON.parse(fs.readFileSync(corpusPath, 'utf8'));
  const namedCase = corpus.segment_cases.find((c) => c.id === 'segment-named-valid');
  assert.ok(namedCase, 'namedCase found in corpus');

  const fullUrl = `https://journal.example${namedCase.location}`;
  const expectedStream = new URL(fullUrl).searchParams.get('stream');
  const env = await createEnvironment(fullUrl);
  await env.releasePendingSegmentGets('analyzed', {
    speaker_labels: { present: true, loaded: true },
    chunks: [
      {
        type: 'audio',
        has_embedding: true,
        speaker_actionable: true,
        markdown: 'hi',
        time: '11:45',
        timestamp: 0,
      },
    ],
  });
  assert.strictEqual(isDeleteBtnVisible(env.doc), true);
  assert.strictEqual(env.doc.querySelector('[data-stream]')?.getAttribute('data-stream'), expectedStream);
});

test('source link direct segment redirect boots and selects direct segment over named segment', async () => {
  const corpusPath = path.join(__dirname, '../../solstone-core-convey-shell/tests/source_link_corpus.json');
  const corpus = JSON.parse(fs.readFileSync(corpusPath, 'utf8'));
  const directCase = corpus.segment_cases.find((c) => c.id === 'segment-direct-valid');
  assert.ok(directCase, 'directCase found in corpus');

  const customSegments = [
    { key: '114500_300', stream: '_default', start: '11:45', end: '11:50', types: ['audio'], data_state: { audio: 'analyzed' } },
    { key: '114500_300', stream: 'room', start: '11:45', end: '11:50', types: ['audio'], data_state: { audio: 'analyzed' } },
  ];
  const fullUrl = `https://journal.example${directCase.location}`;
  const expectedStream = new URL(fullUrl).searchParams.get('stream');
  const env = await createEnvironment(fullUrl, 'analyzed', false, customSegments);
  await env.releasePendingSegmentGets('analyzed', {
    speaker_labels: { present: true, loaded: true },
    chunks: [
      {
        type: 'audio',
        has_embedding: true,
        speaker_actionable: true,
        markdown: 'hi',
        time: '11:45',
        timestamp: 0,
      },
    ],
  });
  assert.strictEqual(isDeleteBtnVisible(env.doc), true);
  assert.strictEqual(env.doc.querySelector('[data-stream]')?.getAttribute('data-stream'), expectedStream);
});

async function run() {
  for (const t of tests) {
    try {
      await t.fn();
    } catch (err) {
      console.error(`FAILED: ${t.name}`);
      console.error(err);
      process.exit(1);
    }
  }
  console.log(`DOM CASES: ${tests.length} passed`);
}

run();
