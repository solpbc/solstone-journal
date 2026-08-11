# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
import shutil
import subprocess
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[4]
WHO_IS_THIS_JS = (
    REPO_ROOT
    / "core"
    / "crates"
    / "solstone-core-convey-shell"
    / "assets"
    / "speakers"
    / "who_is_this.js"
)
WORKSPACE_HTML = (
    REPO_ROOT
    / "core"
    / "crates"
    / "solstone-core-convey-shell"
    / "assets"
    / "speakers"
    / "workspace.html"
)
APP_CSS = REPO_ROOT / "solstone" / "convey" / "static" / "app.css"
SPK_OVERVIEW_SELECT_CLASS = re.compile(r"\.spk-overview-select(?![A-Za-z0-9_-])")
CSS_DISPLAY_DECLARATION = re.compile(r"(^|;)\s*display\s*:", re.IGNORECASE)
CSS_DISPLAY_NONE_DECLARATION = re.compile(
    r"(^|;)\s*display\s*:\s*none\b",
    re.IGNORECASE,
)


def _node_or_skip() -> str:
    node = shutil.which("node")
    if node is None:
        import pytest

        pytest.skip("node is not available")
    return node


DOM_STUB = r"""
const assert = require('assert');
const who = require(process.argv[1]);

class FakeEvent {
  constructor(type, props = {}) {
    this.type = type;
    this.key = props.key || '';
    this.shiftKey = Boolean(props.shiftKey);
    this.target = props.target || null;
    this.defaultPrevented = false;
  }
  preventDefault() {
    this.defaultPrevented = true;
  }
}

class FakeElement {
  constructor(tagName, ownerDocument) {
    this.tagName = String(tagName || '').toUpperCase();
    this.ownerDocument = ownerDocument;
    this.parentNode = null;
    this.children = [];
    this.attributes = {};
    this.dataset = {};
    this.listeners = {};
    this.className = '';
    this.hidden = false;
    this.disabled = false;
    this.value = '';
    this.type = '';
    this.id = '';
    this._text = '';
    this.scrollTop = 0;
  }
  get firstChild() {
    return this.children[0] || null;
  }
  get textContent() {
    return this._text + this.children.map((child) => child.textContent).join('');
  }
  set textContent(value) {
    this._text = String(value ?? '');
    this.children = [];
  }
  setAttribute(name, value) {
    const text = String(value);
    this.attributes[name] = text;
    if (name === 'class') this.className = text;
    if (name === 'id') this.id = text;
    if (name.startsWith('data-')) {
      const key = name.slice(5).replace(/-([a-z])/g, (_match, char) => char.toUpperCase());
      this.dataset[key] = text;
    }
  }
  getAttribute(name) {
    if (name === 'class') return this.className;
    if (name === 'id') return this.id;
    return Object.prototype.hasOwnProperty.call(this.attributes, name)
      ? this.attributes[name]
      : null;
  }
  appendChild(child) {
    child.parentNode = this;
    this.children.push(child);
    return child;
  }
  removeChild(child) {
    this.children = this.children.filter((item) => item !== child);
    child.parentNode = null;
    return child;
  }
  replaceChildren(...nodes) {
    this.children.forEach((child) => { child.parentNode = null; });
    this.children = [];
    this._text = '';
    nodes.forEach((node) => this.appendChild(node));
  }
  remove() {
    if (this.parentNode) this.parentNode.removeChild(this);
  }
  addEventListener(type, handler) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push(handler);
  }
  dispatchEvent(event) {
    if (!event.target) event.target = this;
    (this.listeners[event.type] || []).forEach((handler) => handler(event));
    return !event.defaultPrevented;
  }
  focus() {
    this.ownerDocument.activeElement = this;
  }
  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }
  querySelectorAll(selector) {
    const selectors = selector.split(',').map((part) => part.trim()).filter(Boolean);
    const found = [];
    const visit = (node) => {
      node.children.forEach((child) => {
        if (selectors.some((part) => child.matches(part))) found.push(child);
        visit(child);
      });
    };
    visit(this);
    return found;
  }
  matches(selector) {
    if (selector === 'button:not([disabled])') {
      return this.tagName === 'BUTTON' && !this.disabled;
    }
    if (selector === 'input:not([disabled])') {
      return this.tagName === 'INPUT' && !this.disabled;
    }
    if (selector === 'a[href]') {
      return this.tagName === 'A' && this.getAttribute('href') !== null;
    }
    if (selector === '[tabindex]:not([tabindex="-1"])') {
      const tabindex = this.getAttribute('tabindex');
      return tabindex !== null && tabindex !== '-1';
    }
    if (selector.startsWith('.')) {
      const classes = selector.slice(1).split('.');
      const present = new Set(String(this.className || '').split(/\s+/).filter(Boolean));
      return classes.every((name) => present.has(name));
    }
    if (selector.startsWith('#')) return this.id === selector.slice(1);
    return this.tagName.toLowerCase() === selector.toLowerCase();
  }
}

class FakeDocument {
  constructor() {
    this.body = new FakeElement('body', this);
    this.activeElement = null;
  }
  createElement(tagName) {
    return new FakeElement(tagName, this);
  }
}

function allByTag(root, tagName) {
  return root.querySelectorAll(tagName);
}

function text(root) {
  return root.textContent;
}

function click(node) {
  node.dispatchEvent(new FakeEvent('click'));
}

function input(node, value) {
  node.value = value;
  node.dispatchEvent(new FakeEvent('input'));
}

function makeCopy() {
  return {
    SPK_SHEET_TITLE: 'title',
    SPK_SHEET_LEDE_MANY: 'many {count}',
    SPK_SHEET_LEDE_ONE: 'one',
    SPK_SHELF_CANDIDATES: 'candidates',
    SPK_SHELF_NO_EVIDENCE: 'no evidence',
    SPK_EVIDENCE_SCREEN_MANY: 'screen {count}',
    SPK_EVIDENCE_SCREEN_ONE: 'screen one',
    SPK_EVIDENCE_MEETING_MANY: 'meeting {count}',
    SPK_EVIDENCE_MEETING_ONE: 'meeting one',
    SPK_SHELF_MENTIONS: 'mentions',
    SPK_ANCHOR: 'anchor',
    SPK_ANCHOR_HAS_VOICE: 'anchor voice',
    SPK_SEARCH_LABEL: 'search label',
    SPK_SEARCH_PLACEHOLDER: 'placeholder',
    SPK_THIS_IS_ME: 'me action',
    SPK_THIS_IS_ME_GUIDANCE: 'guidance',
    SPK_SEARCH_NO_RESULTS: 'missing {query}',
    SPK_CREATE_ROW: 'create {query}',
    SPK_NEAR_MATCH_BAND: 'near band',
    SPK_KEEP_SEPARATE_TITLE: 'different {name}',
    SPK_KEEP_SEPARATE_BODY: 'body {name}',
    SPK_KEEP_SEPARATE_CONFIRM: 'confirm new',
    SPK_KEEP_SEPARATE_DECLINE: 'decline {name}',
    SPK_PREVIEW_TITLE: 'preview {name}',
    SPK_PREVIEW_BODY_FRESH: 'fresh',
    SPK_PREVIEW_BODY_HAS_VOICE: 'has voice {name}',
    SPK_PREVIEW_FACTS: 'facts {statements} {conversations}',
    SPK_PREVIEW_CONFIRM: 'confirm {first_name}',
    SPK_PREVIEW_BACK: 'return action',
    SPK_RECEIPT_TITLE: 'receipt {name}',
    SPK_RECEIPT_BODY: 'receipt body {name}',
    SPK_RECEIPT_UNDO: 'undo action',
    SPK_UNDO_DONE: 'undo done',
    SPK_UNDO_PARTIAL: 'partial {restored} {skipped}',
    SPK_EXIT_NOT_PERSON: 'exit person',
    SPK_EXIT_NOT_NOW: 'exit later',
    SPK_NOT_PERSON_DONE: 'done person',
    SPK_NOT_NOW_DONE: 'done later',
    SPK_ACTION_WHO_IS_THIS: 'trigger',
    SPK_LOAD_ERROR: 'load error',
    SPK_SEARCH_ERROR: 'search error',
    SPK_CHECK_NAME_ERROR: 'check error',
    SPK_SAMPLE_UNAVAILABLE: 'unavailable',
    SPK_ACTION_RETRY: 'retry action',
  };
}

function presence(overrides = {}) {
  return {
    cluster_id: 7,
    facts: {
      statement_count: 9,
      conversation_count: 2,
      samples: [
        { day: '20260701', stream: 'stream-a', setting: 'room one', audio_url: null },
        { day: '20260702', stream: 'stream-b', setting: null, audio_url: '/audio/sample.flac' },
      ],
    },
    evidence_complete: true,
    candidates: {
      co_presence: [
        {
          entity_id: 'alice',
          name: 'Alice',
          has_voice: true,
          screen_conversations: 2,
          meeting_days: 1,
        },
      ],
      mention: [
        {
          entity_id: 'bob',
          name: 'Bob',
          has_voice: false,
          setting_conversations: 1,
          speaker_conversations: 0,
        },
      ],
    },
    ...overrides,
  };
}

function fullUndoResult(overrides = {}) {
  return {
    status: 'undone',
    undo_report: {
      labels: { restored_count: 1, skipped_count: 0 },
      corrections: { restored_count: 1, skipped_count: 0 },
      voiceprints: { restored_count: 1, skipped_count: 0 },
      tracker: { restored_count: 1, skipped_count: 0 },
      sentinel: { restored_count: 1, skipped_count: 0 },
      entity: {
        restored_count: 1,
        skipped_count: 0,
        blocked_categories: [],
      },
    },
    ...overrides,
  };
}

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

function makeHarness(options = {}) {
  const doc = new FakeDocument();
  const calls = [];
  const logs = [];
  let requestIndex = 0;
  const apiJson = options.apiJson || ((url, request) => {
    calls.push({
      url,
      request,
      body: request?.body ? JSON.parse(request.body) : null,
    });
    if (url.includes('/presence')) return Promise.resolve(options.presence || presence());
    return Promise.resolve({});
  });
  const controller = who.init({
    mount: doc.body,
    copy: makeCopy(),
    context: { isDay: true },
    apiJson,
    logError: (error, meta) => logs.push({ error, meta }),
    formatDateShort: (day) => `weekday-${day}`,
    debounceMs: 0,
    requestIdFactory: () => `req-${++requestIndex}`,
    onThisIsMe: options.onThisIsMe,
    onIdentified: options.onIdentified,
    onDismissed: options.onDismissed,
    onFullyRestoredUndo: options.onFullyRestoredUndo,
  });
  const trigger = doc.createElement('button');
  doc.body.appendChild(trigger);
  return { doc, controller, trigger, calls, logs };
}
"""


WORKSPACE_DOM_STUB = r"""
const assert = require('assert');
const fs = require('fs');
const vm = require('vm');
const workspaceHtml = fs.readFileSync(process.argv[1], 'utf8');
const workspaceScripts = [...workspaceHtml.matchAll(/<script>([\s\S]*?)<\/script>/g)]
  .map((match) => match[1]);

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function response(payload) {
  return {
    ok: true,
    json: () => Promise.resolve(payload),
  };
}

function apiError(payload, status = 503) {
  const error = new Error(payload?.error || payload?.message || `Request failed (HTTP ${status})`);
  error.name = 'ApiError';
  error.status = status;
  error.statusText = '';
  error.serverMessage = payload?.error || payload?.message || error.message;
  error.reasonCode = payload?.reason_code || null;
  error.rawDetail = payload?.detail ?? null;
  error.payload = payload;
  return error;
}

function escapeHtml(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

function datasetKey(name) {
  return name.slice(5).replace(/-([a-z])/g, (_match, char) => char.toUpperCase());
}

function parseAttrs(raw) {
  const attrs = {};
  String(raw || '').replace(/([a-zA-Z0-9_-]+)="([^"]*)"/g, (_match, name, value) => {
    attrs[name] = value;
    return '';
  });
  return attrs;
}

class FakeClassList {
  constructor(node) {
    this.node = node;
  }
  _set() {
    return new Set(String(this.node.className || '').split(/\s+/).filter(Boolean));
  }
  add(...names) {
    const classes = this._set();
    names.forEach((name) => classes.add(name));
    this.node.className = [...classes].join(' ');
  }
  remove(...names) {
    const classes = this._set();
    names.forEach((name) => classes.delete(name));
    this.node.className = [...classes].join(' ');
  }
  contains(name) {
    return this._set().has(name);
  }
}

class FakeElement {
  constructor(tagName, ownerDocument) {
    this.tagName = String(tagName || 'div').toUpperCase();
    this.ownerDocument = ownerDocument;
    this.parentNode = null;
    this.children = [];
    this.attributes = {};
    this.dataset = {};
    this.listeners = {};
    this.style = {};
    this.className = '';
    this.classList = new FakeClassList(this);
    this.hidden = false;
    this.disabled = false;
    this.value = '';
    this.href = '';
    this.id = '';
    this._html = '';
    this._text = '';
    this.scrollIntoViewCalls = [];
  }
  get firstChild() {
    return this.children[0] || null;
  }
  get textContent() {
    if (this.children.length) {
      const ownText = this._text + this._html.replace(/<[^>]*>/g, '');
      const childText = this.children.map((child) => child.textContent).join('');
      return ownText + childText;
    }
    return this._text || this._html.replace(/<[^>]*>/g, '');
  }
  set textContent(value) {
    this.replaceChildren();
    this._text = String(value ?? '');
    this._html = '';
  }
  get innerHTML() {
    return this._html;
  }
  set innerHTML(value) {
    this._html = String(value ?? '');
    this._text = '';
    this.children.forEach((child) => { child.parentNode = null; });
    this.children = [];
    parseKnownHtml(this, this._html);
  }
  setAttribute(name, value) {
    const text = String(value);
    this.attributes[name] = text;
    if (name === 'class') this.className = text;
    if (name === 'id') {
      this.id = text;
      this.ownerDocument.register(this);
    }
    if (name.startsWith('data-')) this.dataset[datasetKey(name)] = text;
    if (name === 'href') this.href = text;
  }
  getAttribute(name) {
    if (name === 'class') return this.className;
    if (name === 'id') return this.id;
    if (name === 'href') return this.href || null;
    return Object.prototype.hasOwnProperty.call(this.attributes, name)
      ? this.attributes[name]
      : null;
  }
  appendChild(child) {
    child.parentNode = this;
    this.children.push(child);
    if (child.id) this.ownerDocument.register(child);
    return child;
  }
  insertBefore(child, reference) {
    child.parentNode = this;
    const index = this.children.indexOf(reference);
    if (index === -1) this.children.push(child);
    else this.children.splice(index, 0, child);
    if (child.id) this.ownerDocument.register(child);
    return child;
  }
  removeChild(child) {
    this.children = this.children.filter((item) => item !== child);
    child.parentNode = null;
    return child;
  }
  replaceChildren(...nodes) {
    this.children.forEach((child) => { child.parentNode = null; });
    this.children = [];
    this._html = '';
    this._text = '';
    nodes.forEach((node) => this.appendChild(node));
  }
  remove() {
    if (this.parentNode) this.parentNode.removeChild(this);
  }
  addEventListener(type, handler) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push(handler);
  }
  dispatchEvent(event) {
    event.target = event.target || this;
    (this.listeners[event.type] || []).forEach((handler) => handler(event));
  }
  click() {
    this.dispatchEvent({ type: 'click', target: this });
  }
  focus() {
    this.ownerDocument.activeElement = this;
  }
  scrollIntoView(options) {
    this.scrollIntoViewCalls.push(options || {});
  }
  contains(node) {
    if (node === this) return true;
    return this.children.some((child) => child.contains(node));
  }
  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }
  querySelectorAll(selector) {
    const selectors = selector.split(',').map((part) => part.trim()).filter(Boolean);
    const found = [];
    const visit = (node) => {
      node.children.forEach((child) => {
        if (selectors.some((part) => child.matches(part))) found.push(child);
        visit(child);
      });
    };
    visit(this);
    return found;
  }
  closest(selector) {
    let node = this;
    while (node) {
      if (node.matches(selector)) return node;
      node = node.parentNode;
    }
    return null;
  }
  matches(selector) {
    if (selector.startsWith('.')) {
      const classes = selector.slice(1).split('.');
      const present = new Set(String(this.className || '').split(/\s+/).filter(Boolean));
      return classes.every((name) => present.has(name));
    }
    if (selector.startsWith('#')) return this.id === selector.slice(1);
    const dataMatch = selector.match(/^\[data-([a-z0-9-]+)="([^"]*)"\]$/);
    if (dataMatch) {
      return this.dataset[datasetKey(`data-${dataMatch[1]}`)] === dataMatch[2];
    }
    return this.tagName.toLowerCase() === selector.toLowerCase();
  }
}

function applyAttrs(node, attrs) {
  Object.entries(attrs).forEach(([name, value]) => node.setAttribute(name, value));
}

function appendParsedNode(parent, tagName, attrs) {
  const node = parent.ownerDocument.createElement(tagName);
  applyAttrs(node, attrs);
  parent.appendChild(node);
  return node;
}

function parseKnownHtml(parent, html) {
  const discoveryPattern = /<div class="([^"]*(?:spk-discovery-card|spk-discovery-cluster)[^"]*)" data-cluster-id="([^"]+)"[\s\S]*?<button[^>]*class="([^"]*spk-who-trigger[^"]*)"[^>]*>/g;
  let match;
  while ((match = discoveryPattern.exec(html)) !== null) {
    const card = appendParsedNode(parent, 'div', {
      class: match[1],
      'data-cluster-id': match[2],
    });
    appendParsedNode(card, 'button', { class: match[3] });
  }

  const segmentPattern = /<li class="([^"]*spk-segment[^"]*)"[\s\S]*?data-key="([^"]+)"[\s\S]*?>/g;
  while ((match = segmentPattern.exec(html)) !== null) {
    appendParsedNode(parent, 'li', {
      class: match[1],
      'data-key': match[2],
    });
  }

  const tagPattern = /<([a-zA-Z0-9]+)([^>]*)>/g;
  while ((match = tagPattern.exec(html)) !== null) {
    const attrs = parseAttrs(match[2]);
    if (!attrs.id) continue;
    if (parent.querySelector(`#${attrs.id}`)) continue;
    appendParsedNode(parent, match[1], attrs);
  }

  const sourceTabPattern = /<button class="([^"]*spk-source-tab[^"]*)" data-source="([^"]+)"/g;
  while ((match = sourceTabPattern.exec(html)) !== null) {
    appendParsedNode(parent, 'button', {
      class: match[1],
      'data-source': match[2],
    });
  }

  const filterPattern = /<button class="([^"]*spk-filter-btn[^"]*)" data-filter="([^"]+)"/g;
  while ((match = filterPattern.exec(html)) !== null) {
    appendParsedNode(parent, 'button', {
      class: match[1],
      'data-filter': match[2],
    });
  }
}

class FakeDocument {
  constructor(ids) {
    this.body = new FakeElement('body', this);
    this.activeElement = null;
    this.readyState = 'complete';
    this.byId = {};
    ids.forEach((id) => this.createRoot(id));
  }
  createRoot(id) {
    const node = this.createElement('div');
    node.setAttribute('id', id);
    this.body.appendChild(node);
    return node;
  }
  register(node) {
    if (node.id) this.byId[node.id] = node;
  }
  createElement(tagName) {
    return new FakeElement(tagName, this);
  }
  getElementById(id) {
    return this.byId[id] || null;
  }
  querySelector(selector) {
    return this.body.querySelector(selector);
  }
  querySelectorAll(selector) {
    if (selector === '[data-copy]') return [];
    return this.body.querySelectorAll(selector);
  }
  addEventListener() {}
}

function queueResponse(queues, name, url, payloadForImmediate) {
  if (payloadForImmediate) return Promise.resolve(response(payloadForImmediate));
  const item = deferred();
  item.url = url;
  queues[name].push(item);
  return item.promise;
}

function resolveFetch(item, payload) {
  item.resolve(response(payload));
}

function resolveApi(item, payload) {
  item.resolve(payload);
}

function rejectApi(item, error) {
  item.reject(error);
}

function queueApiResponse(queues, name, url) {
  const item = deferred();
  item.url = url;
  queues[name].push(item);
  return item.promise;
}

function dispatchHashChange(window) {
  window.dispatchEvent({ type: 'hashchange', target: window });
}

function assertOwnerBannerVisible(document, expectedText = '') {
  const target = document.getElementById('spkOwnerBanner');
  assert.notStrictEqual(target.style.display, 'none');
  assert.strictEqual(target.hidden, false);
  assert(target.textContent.trim().length > 0);
  if (expectedText) assert(target.textContent.includes(expectedText));
  assert.strictEqual(document.activeElement, target);
  return target;
}

function assertOwnerBannerHidden(document) {
  const target = document.getElementById('spkOwnerBanner');
  assert.strictEqual(target.style.display, 'none');
  assert.strictEqual(target.innerHTML, '');
  return target;
}

function speakerCopy() {
  return {
    SPK_ACTION_WHO_IS_THIS: 'who',
    SPK_ACTION_RETRY: 'try again',
    SPK_DISCOVERY_ERROR: 'discovery error',
    SPK_DISCOVERY_DEGRADED_TEMPLATE: 'degraded {count}',
    SPK_GRID_BODY: 'grid',
    SPK_OVERVIEW_TODAY_LINK_LABEL: 'today',
    SPK_OVERVIEW_KNOWN_VOICES_SORTS: ['recent'],
    SPK_OVERVIEW_KNOWN_VOICES_EMPTY: 'known empty',
    SPK_OVERVIEW_CARD_SAMPLES_LABEL: 'samples',
    SPK_OVERVIEW_CARD_SEGMENTS_LABEL: 'segments',
    SPK_OVERVIEW_YOUR_VOICE_HEADER: 'your voice',
    SPK_OVERVIEW_CARD_LAST_HEARD_PREFIX: 'last',
    SPK_OVERVIEW_CARD_STREAMS_PREFIX: 'streams',
    SPK_OVERVIEW_COHESION_LABELS: [
      'learning',
      'early',
      'improving',
      'good',
      'strong',
      'settled',
    ],
    SPK_OVERVIEW_QUALITY_READY: 'quality ready',
    SPK_OVERVIEW_QUALITY_ERROR_HEADING: 'quality failed',
    SPK_OVERVIEW_QUALITY_TEACHING_ZERO: 'teaching zero',
    SPK_THIS_IS_ME_GUIDANCE: 'guidance',
    SPK_OWNER_TEACH_TITLE: 'teach title',
    SPK_OWNER_TEACH_BODY: 'teach body',
    SPK_OWNER_TEACH_LOADING: 'teach loading',
    SPK_OWNER_TEACH_PROGRESS_TEMPLATE: '{count}/{minimum}',
    SPK_OWNER_TEACH_START_LABEL: 'start',
    SPK_OWNER_TEACH_PAUSE_LABEL: 'pause',
  };
}

function segment(key = 'seg-a') {
  return {
    key,
    stream: 'test',
    sources: ['audio'],
    start: '10:00',
    end: '10:05',
    duration: 300,
    speaker_count: 0,
    attribution_total: 1,
    attribution_needs_review: 1,
    attribution_non_owner_total: 1,
    attribution_null: 1,
  };
}

function reviewPayload(name = '') {
  return {
    day: '20240101',
    source: 'audio',
    segment: segment(),
    audio_file: '',
    audio_mimetype: '',
    has_labels: true,
    all_entities: [],
    summary: {},
    sentences: [
      {
        id: 1,
        offset: 0,
        text: 'hello',
        speaker_name: name,
        speaker_entity_id: name ? 'undone_person' : '',
        confidence: name ? 'high' : '',
        method: name ? 'user_identified' : '',
        needs_review: !name,
      },
    ],
  };
}

function discoveryCluster(clusterId, name = `Cluster ${clusterId}`) {
  return {
    cluster_id: clusterId,
    suggested_name: name,
    size: 1,
    segment_count: 1,
    samples: [{ text: name }],
  };
}

function discoveryOk(clusters) {
  return { status: 'ok', clusters, issues: [] };
}

function discoveryDegraded(clusters, count = 2) {
  return {
    status: 'degraded',
    clusters,
    issues: [{
      reason_code: 'speaker_discovery_invalid_embeddings',
      message: 'backend degraded message',
      count,
    }],
  };
}

function discoveryFailure(message = 'scan failed', retryable = true) {
  return apiError({
    error: message,
    reason_code: 'speaker_discovery_failed',
    detail: '',
    retryable,
  }, retryable ? 503 : 500);
}

function discoverySurface(document, kind) {
  return kind === 'overview'
    ? document.getElementById('spkNewVoicesSection')
    : document.getElementById('spkDiscoveryBanner');
}

function discoveryClusterHost(document, kind) {
  return kind === 'overview'
    ? document.getElementById('spkDiscoveryClusters')
    : document.getElementById('spkDiscoveryBanner');
}

function discoveryNoticeHost(document, kind) {
  return kind === 'overview'
    ? document.getElementById('spkOverviewDiscoveryNotice')
    : document.getElementById('spkDiscoveryBanner');
}

function discoveryRetryButton(document, kind) {
  const host = discoveryNoticeHost(document, kind);
  return host.querySelector(`#${kind === 'overview' ? 'spkOverviewDiscoveryRetry' : 'spkDayDiscoveryRetry'}`);
}

function assertDiscoveryCluster(document, kind, clusterId) {
  assert(discoveryClusterHost(document, kind).querySelector(`[data-cluster-id="${clusterId}"]`));
}

function assertNoDiscoveryCluster(document, kind, clusterId) {
  assert.strictEqual(discoveryClusterHost(document, kind).querySelector(`[data-cluster-id="${clusterId}"]`), null);
}

function assertDiscoveryNotice(document, kind, expectedText, expectsRetry = false) {
  const host = discoveryNoticeHost(document, kind);
  if (kind === 'overview') assert.strictEqual(host.hidden, false);
  assert(host.textContent.includes(expectedText));
  if (expectsRetry) assert(discoveryRetryButton(document, kind));
}

function assertNoDiscoveryNotice(document, kind, textValue) {
  const host = discoveryNoticeHost(document, kind);
  assert(!host.textContent.includes(textValue));
  assert.strictEqual(discoveryRetryButton(document, kind), null);
}

function matchedSpeakers(name = '') {
  return {
    matched: name ? [{ entity_name: name, detected_name: name }] : [],
    unmatched: [],
  };
}

function makeLocation(hash = '') {
  const location = { href: '' };
  let currentHash = '';
  Object.defineProperty(location, 'hash', {
    get() {
      return currentHash;
    },
    set(value) {
      const text = String(value || '');
      currentHash = text && !text.startsWith('#') ? `#${text}` : text;
    },
  });
  location.hash = hash;
  return location;
}

function makeWorkspaceContext(kind, options = {}) {
  const config = options || {};
  const day = config.day || '20240101';
  const today = config.today || '20240101';
  const hash = config.hash ?? (kind === 'day' ? '#seg-a' : '');
  const ownerStatus = config.ownerStatus || { status: 'confirmed', centroid_metadata: {} };
  const ids = kind === 'overview'
    ? [
        'speakersOverviewView',
        'spkOverviewYourVoiceSection',
        'spkOverviewOwner',
        'spkOverviewQuality',
        'spkKnownVoices',
        'spkKnownSort',
        'spkNewVoicesSection',
        'spkStatementHandoffNotice',
        'spkOverviewDiscoveryNotice',
        'spkDiscoveryClusters',
        'spkTodayReview',
        'spkDayGridCard',
        'spkDayGridCopy',
        'spkDayGridHost',
        'spkDayGridLegend',
      ]
    : [
        'speakersDayView',
        'spkSegmentList',
        'spkDetail',
        'spkOwnerBanner',
        'spkDiscoveryBanner',
        'spkSegmentsStatus',
        'spkFilterIndicator',
      ];
  const document = new FakeDocument(ids);
  const overviewYourVoiceSection = document.getElementById('spkOverviewYourVoiceSection');
  if (overviewYourVoiceSection) {
    overviewYourVoiceSection.tagName = 'SECTION';
    overviewYourVoiceSection.setAttribute('data-section', 'your-voice');
    overviewYourVoiceSection.setAttribute('tabindex', '-1');
    overviewYourVoiceSection.setAttribute('aria-labelledby', 'spkOverviewYourVoiceHeader');
    const ownerPanel = document.getElementById('spkOverviewOwner');
    if (ownerPanel?.parentNode) ownerPanel.parentNode.removeChild(ownerPanel);
    const header = document.createElement('h2');
    header.setAttribute('id', 'spkOverviewYourVoiceHeader');
    header.setAttribute('data-copy', 'SPK_OVERVIEW_YOUR_VOICE_HEADER');
    overviewYourVoiceSection.appendChild(header);
    if (ownerPanel) overviewYourVoiceSection.appendChild(ownerPanel);
  }
  const queues = {
    discovery: [],
    scan: [],
    known: [],
    quality: [],
    segments: [],
    speakers: [],
    review: [],
    ownerStatus: [],
  };
  const sheets = [];
  const fetchCalls = [];
  const apiCalls = [];
  const windowListeners = {};
  const window = {
    SPEAKERS_CONTEXT: kind === 'overview'
      ? { isDay: false }
      : { isDay: true, day },
    SPEAKERS_STATE_PROMISE: Promise.resolve({
      speaker_copy: speakerCopy(),
      owner_status_routing_tokens: { candidate: 'candidate', confirmed: 'confirmed' },
      not_in_new_voices_copy: 'not in new voices',
      today,
      owner_min_statements: 3,
    }),
    AppServices: { escapeHtml },
    ConveyIcons: {
      svg: (name) => `<svg data-icon="${escapeHtml(name)}"></svg>`,
    },
    SurfaceState: {
      loading: ({ text }) => `<div>${escapeHtml(text)}</div>`,
      error: ({ heading }) => `<div class="surface-state-retry">${escapeHtml(heading)}</div>`,
      empty: ({ heading }) => `<div>${escapeHtml(heading)}</div>`,
    },
    CONVEY_COPY: { RELOAD_HINT: 'reload' },
    RelativeTime: { formatTimestamp: () => 'recently' },
    DayGrid: null,
    Drawer: { preserveOpen(_node, render) { render(); } },
    GateDrawer: { render: () => '' },
    location: makeLocation(hash),
    addEventListener(type, handler) {
      if (!windowListeners[type]) windowListeners[type] = [];
      windowListeners[type].push(handler);
    },
    dispatchEvent(event) {
      event.target = event.target || window;
      (windowListeners[event.type] || []).forEach((handler) => handler(event));
    },
    logError() {},
    formatDateShort: (day) => day,
    SpeakersWhoIsThis: {
      init(options) {
        sheets.push(options);
        return {
          setCopy(copy) { this.copy = copy; },
          open(args) { this.openArgs = args; },
        };
      },
    },
  };
  window.fetch = (url, requestOptions) => {
    fetchCalls.push({ url, options: requestOptions });
    if (url.includes('/api/grid')) return Promise.resolve(response(null));
    if (url.includes('/api/owner/status')) {
      if (config.ownerStatusError) return Promise.reject(config.ownerStatusError);
      return Promise.resolve(response(ownerStatus));
    }
    if (url.includes('/api/quality')) return queueResponse(queues, 'quality', url);
    if (url.includes('/api/speakers/known')) return queueResponse(queues, 'known', url);
    if (url.includes('/api/discovery/cache')) return queueResponse(queues, 'discovery', url);
    if (url.includes('/api/discovery/scan')) return queueResponse(queues, 'scan', url);
    if (url.includes('/api/segments/')) return queueResponse(queues, 'segments', url);
    if (url.includes(`/api/speakers/${day}/`)) return queueResponse(queues, 'speakers', url);
    throw new Error(`unexpected fetch ${url}`);
  };
  window.apiJson = (url, options = {}) => {
    apiCalls.push({ url, options });
    if (url.includes('/api/owner/status')) {
      if (config.deferOwnerStatus) return queueApiResponse(queues, 'ownerStatus', url);
      if (config.ownerStatusError) return Promise.reject(config.ownerStatusError);
      return Promise.resolve(ownerStatus);
    }
    if (url.includes('/api/owner/detect')) return Promise.resolve({});
    if (url.includes('/api/discovery/cache')) return queueResponse(queues, 'discovery', url).then((r) => r.json());
    if (url.includes('/api/discovery/scan')) return queueResponse(queues, 'scan', url).then((r) => r.json());
    if (url.includes('/api/review/')) return queueResponse(queues, 'review', url).then((r) => r.json());
    if (url.includes('/api/segments/')) return queueResponse(queues, 'segments', url).then((r) => r.json());
    throw new Error(`unexpected api ${url}`);
  };
  const context = {
    console,
    document,
    window,
    fetch: window.fetch,
    URLSearchParams,
    Date,
    setTimeout,
    clearTimeout,
    setImmediate,
  };
  vm.createContext(context);
  vm.runInContext(kind === 'overview' ? workspaceScripts[2] : workspaceScripts[1], context);
  return { context, document, window, queues, sheets, fetchCalls, apiCalls };
}
"""


def _run_node(body: str) -> None:
    node = _node_or_skip()
    script = DOM_STUB + "\n" + textwrap.dedent(body)
    result = subprocess.run(
        [node, "-e", script, str(WHO_IS_THIS_JS)],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr


def _run_workspace_node(body: str) -> None:
    node = _node_or_skip()
    script = WORKSPACE_DOM_STUB + "\n" + textwrap.dedent(body)
    result = subprocess.run(
        [node, "-e", script, str(WORKSPACE_HTML)],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr


def _strip_css_comments(css: str) -> str:
    return re.sub(r"/\*.*?\*/", "", css, flags=re.DOTALL)


def _iter_css_rules(css: str):
    source = _strip_css_comments(css)
    index = 0
    while index < len(source):
        brace = source.find("{", index)
        if brace == -1:
            break
        prelude = source[index:brace].strip()
        prelude = prelude.rsplit(";", 1)[-1].strip()
        depth = 1
        cursor = brace + 1
        in_string: str | None = None
        escaped = False
        while cursor < len(source) and depth:
            char = source[cursor]
            if in_string:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == in_string:
                    in_string = None
            elif char in {"'", '"'}:
                in_string = char
            elif char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
            cursor += 1
        if depth != 0:
            raise AssertionError(f"unterminated CSS rule after {prelude!r}")
        declarations = source[brace + 1 : cursor - 1]
        if prelude:
            if prelude.startswith("@"):
                yield from _iter_css_rules(declarations)
            else:
                yield prelude, declarations
        index = cursor


def _split_selector_prelude(prelude: str) -> list[str]:
    selectors: list[str] = []
    start = 0
    paren_depth = 0
    bracket_depth = 0
    in_string: str | None = None
    escaped = False
    for index, char in enumerate(prelude):
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == in_string:
                in_string = None
            continue
        if char in {"'", '"'}:
            in_string = char
        elif char == "(":
            paren_depth += 1
        elif char == ")" and paren_depth:
            paren_depth -= 1
        elif char == "[":
            bracket_depth += 1
        elif char == "]" and bracket_depth:
            bracket_depth -= 1
        elif char == "," and paren_depth == 0 and bracket_depth == 0:
            selectors.append(prelude[start:index].strip())
            start = index + 1
    selectors.append(prelude[start:].strip())
    return [selector for selector in selectors if selector]


def _selector_applies_to_spk_overview_select(selector: str) -> bool:
    return SPK_OVERVIEW_SELECT_CLASS.search(selector) is not None


def _style_blocks(html: str) -> list[str]:
    return [
        match.group(1)
        for match in re.finditer(r"<style[^>]*>(.*?)</style>", html, flags=re.DOTALL)
    ]


def test_spk_overview_select_css_detector_handles_parser_edges() -> None:
    css = """
    @charset "utf-8";
    @import "x.css";
    .spk-overview-btn,
    .spk-overview-link,
    .spk-overview-select {
      color: black;
    }

    .spk-overview-selection {
      display: flex;
    }

    @media (min-width: 40rem) {
      .spk-overview-select:hover {
        color: blue;
      }

      .x .spk-overview-select {
        color: red;
      }
    }
    """
    matched_selectors: list[str] = []
    display_selectors: list[str] = []
    for prelude, declarations in _iter_css_rules(css):
        applying_selectors = [
            selector
            for selector in _split_selector_prelude(prelude)
            if _selector_applies_to_spk_overview_select(selector)
        ]
        matched_selectors.extend(applying_selectors)
        if CSS_DISPLAY_DECLARATION.search(declarations):
            display_selectors.extend(applying_selectors)

    assert ".spk-overview-select" in matched_selectors
    assert ".spk-overview-select:hover" in matched_selectors
    assert ".x .spk-overview-select" in matched_selectors
    assert ".spk-overview-selection" not in matched_selectors
    assert display_selectors == []


def test_spk_overview_select_display_rules_have_hidden_override() -> None:
    workspace_html = WORKSPACE_HTML.read_text(encoding="utf-8")
    assert 'class="spk-overview-select"' in workspace_html, (
        "the .spk-overview-select class was renamed; update this tripwire to follow it"
    )
    style_blocks = _style_blocks(workspace_html)
    assert style_blocks, "no <style> blocks found in speakers workspace"
    css_sources = [
        (f"speakers workspace style block {index}", block)
        for index, block in enumerate(style_blocks, start=1)
    ]
    css_sources.append(("convey app.css", APP_CSS.read_text(encoding="utf-8")))

    applying_rule_count = 0
    failures: list[str] = []
    for source_name, css in css_sources:
        display_selectors: list[str] = []
        has_hidden_display_none = False
        for prelude, declarations in _iter_css_rules(css):
            applying_selectors = [
                selector
                for selector in _split_selector_prelude(prelude)
                if _selector_applies_to_spk_overview_select(selector)
            ]
            if not applying_selectors:
                continue
            applying_rule_count += 1
            declares_display = CSS_DISPLAY_DECLARATION.search(declarations) is not None
            if (
                declares_display
                and CSS_DISPLAY_NONE_DECLARATION.search(declarations) is not None
                and any("[hidden]" in selector for selector in applying_selectors)
            ):
                has_hidden_display_none = True
            if declares_display:
                display_selectors.extend(applying_selectors)
        if display_selectors and not has_hidden_display_none:
            failures.append(f"{source_name}: {', '.join(display_selectors)}")

    assert applying_rule_count > 0, (
        "no CSS rules apply to .spk-overview-select; update this tripwire "
        "to follow the class if it was renamed"
    )
    assert not failures, (
        ".spk-overview-select display rules need a co-located "
        "[hidden] display:none override: " + "; ".join(failures)
    )


def test_who_is_this_accessibility_contract() -> None:
    _run_node(
        """
        (async () => {
          const { doc, controller, trigger } = makeHarness();
          await controller.open({ cluster: { cluster_id: 7 }, trigger });

          const dialog = doc.body.querySelector('.spk-who-dialog');
          assert.strictEqual(trigger.getAttribute('aria-haspopup'), 'dialog');
          assert.strictEqual(trigger.getAttribute('aria-expanded'), 'true');
          assert.strictEqual(dialog.getAttribute('role'), 'dialog');
          assert.strictEqual(dialog.getAttribute('aria-modal'), 'true');
          assert.strictEqual(dialog.getAttribute('aria-labelledby'), 'spkWhoTitle');
          assert.strictEqual(dialog.getAttribute('tabindex'), '-1');
          assert(doc.activeElement.matches('.spk-who-person-action'));

          const focusable = controller.focusableElements();
          controller.handleDialogKeydown(new FakeEvent('keydown', {
            key: 'Tab',
            target: focusable[focusable.length - 1],
          }));
          assert.strictEqual(doc.activeElement, focusable[0]);

          controller.handleDialogKeydown(new FakeEvent('keydown', {
            key: 'Tab',
            shiftKey: true,
            target: focusable[0],
          }));
          assert.strictEqual(doc.activeElement, focusable[focusable.length - 1]);

          controller.handleDialogKeydown(new FakeEvent('keydown', {
            key: 'Escape',
            target: dialog,
          }));
          assert.strictEqual(trigger.getAttribute('aria-expanded'), 'false');
          assert.strictEqual(doc.activeElement, trigger);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_ordinary_close_restores_and_deep_link_has_no_trigger() -> None:
    _run_node(
        """
        (async () => {
          const { doc, controller, trigger } = makeHarness();
          await controller.open({ cluster: { cluster_id: 7 }, trigger });

          controller.backdrop.dispatchEvent(new FakeEvent('click', { target: controller.backdrop }));
          assert.strictEqual(trigger.getAttribute('aria-expanded'), 'false');
          assert.strictEqual(doc.activeElement, trigger);

          const deep = makeHarness();
          await deep.controller.open({ cluster: { cluster_id: 8 }, trigger: null });
          assert.strictEqual(deep.controller.trigger, null);
          deep.controller.close();
          assert.strictEqual(deep.controller.trigger, null);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_main_lede_samples_and_evidence_states() -> None:
    _run_node(
        """
        (async () => {
          const { doc, controller, trigger } = makeHarness();
          await controller.open({ cluster: { cluster_id: 7 }, trigger });
          const bodyText = text(doc.body);
          assert(bodyText.includes('many 2'));
          assert(bodyText.includes('weekday-20260701 · room one'));
          assert(bodyText.includes('weekday-20260702 · stream-b'));
          assert(bodyText.includes('unavailable'));
          assert(bodyText.includes('candidates'));
          assert(bodyText.includes('screen 2'));
          assert(bodyText.includes('anchor voice'));
          assert(bodyText.includes('mentions'));
          assert(!doc.body.querySelector('.spk-who-mentions').textContent.includes('screen'));

          const emptyHarness = makeHarness({
            presence: presence({
              candidates: { co_presence: [], mention: [] },
              facts: { statement_count: 1, conversation_count: 1, samples: [] },
            }),
          });
          await emptyHarness.controller.open({
            cluster: { cluster_id: 8 },
            trigger: emptyHarness.trigger,
          });
          assert(text(emptyHarness.doc.body).includes('one'));
          assert(text(emptyHarness.doc.body).includes('no evidence'));
          assert(emptyHarness.doc.activeElement.matches('.spk-who-search-input'));

          const incompleteHarness = makeHarness({
            presence: presence({ evidence_complete: false }),
          });
          await incompleteHarness.controller.open({
            cluster: { cluster_id: 9 },
            trigger: incompleteHarness.trigger,
          });
          assert(text(incompleteHarness.doc.body).includes('load error'));
          assert(incompleteHarness.doc.activeElement.matches('.spk-who-retry'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_sample_unavailable_paths() -> None:
    _run_node(
        """
        (async () => {
          const { doc, controller, trigger } = makeHarness();
          await controller.open({ cluster: { cluster_id: 7 }, trigger });

          assert.strictEqual(doc.body.querySelectorAll('.spk-who-sample-unavailable').length, 1);
          const audio = doc.body.querySelector('audio');
          assert(audio);
          const sampleTextBefore = audio.parentNode.textContent;
          audio.dispatchEvent(new FakeEvent('error'));

          assert.strictEqual(doc.body.querySelectorAll('.spk-who-sample-unavailable').length, 2);
          assert.strictEqual(audio.hidden, true);
          assert(audio.parentNode.textContent.includes(sampleTextBefore));
          assert(audio.parentNode.textContent.includes('unavailable'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_search_latest_query_wins_and_text_safety() -> None:
    _run_node(
        """
        (async () => {
          const resolvers = {};
          const apiJson = (url, request) => {
            if (url.includes('/presence')) {
              return Promise.resolve(presence({
                candidates: { co_presence: [], mention: [] },
                facts: { statement_count: 1, conversation_count: 1, samples: [] },
              }));
            }
            if (url.includes('/people/search')) {
              return new Promise((resolve) => { resolvers[url] = resolve; });
            }
            return Promise.resolve({});
          };
          const { doc, controller, trigger } = makeHarness({ apiJson });
          await controller.open({ cluster: { cluster_id: 7 }, trigger });
          const search = doc.body.querySelector('.spk-who-search-input');
          input(search, 'old');
          input(search, '"><script>');
          const urls = Object.keys(resolvers);
          assert.strictEqual(urls.length, 2);

          resolvers[urls[1]]({
            query: '"><script>',
            people: [
              {
                entity_id: 'mal',
                name: '<img src=x onerror=alert(1)>',
                has_voice: false,
              },
            ],
          });
          await Promise.resolve();
          resolvers[urls[0]]({
            query: 'old',
            people: [{ entity_id: 'old', name: 'Old Result', has_voice: false }],
          });
          await Promise.resolve();

          const bodyText = text(doc.body);
          assert(bodyText.includes('<img src=x onerror=alert(1)>'));
          assert(bodyText.includes('create "><script>'));
          assert(!bodyText.includes('Old Result'));
          assert.strictEqual(allByTag(doc.body, 'script').length, 0);
          assert.strictEqual(allByTag(doc.body, 'img').length, 0);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_overview_full_undo_refreshes_known_quality_and_rediscovery() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('overview');
          const { document, queues, sheets } = harness;
          await flush();
          await flush();

          resolveFetch(queues.quality[0], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          resolveFetch(queues.known[0], { speakers: [] });
          resolveFetch(queues.discovery[0], {
            clusters: [
              { cluster_id: 7, suggested_name: 'Initial Voice', size: 1, samples: [] },
            ],
          });
          await flush();
          await flush();

          const discoveryContainer = document.getElementById('spkDiscoveryClusters');
          discoveryContainer.querySelector('.spk-who-trigger').click();
          assert.strictEqual(sheets.length, 1);
          const sheetOptions = sheets[0];

          sheetOptions.onIdentified({ clusterId: '7' });
          await flush();
          assert.strictEqual(queues.known.length, 2);
          assert.strictEqual(queues.quality.length, 2);
          assert.strictEqual(discoveryContainer.querySelector('[data-cluster-id="7"]'), null);

          const refresh = sheetOptions.onFullyRestoredUndo();
          await flush();
          assert.strictEqual(queues.discovery.length, 2);
          assert.strictEqual(queues.known.length, 3);
          assert.strictEqual(queues.quality.length, 3);

          resolveFetch(queues.discovery[1], {
            clusters: [
              { cluster_id: 7, suggested_name: 'Restored Voice', size: 1, samples: [] },
            ],
          });
          resolveFetch(queues.known[2], { speakers: [] });
          resolveFetch(queues.quality[2], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          await refresh;
          await flush();

          resolveFetch(queues.known[1], {
            speakers: [
              {
                entity_id: 'undone_person',
                name: 'Undone Person',
                streams: [],
                embedding_count: 1,
                segment_count: 1,
              },
            ],
          });
          resolveFetch(queues.quality[1], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          await flush();
          await flush();

          assert(discoveryContainer.querySelector('[data-cluster-id="7"]'));
          assert(document.getElementById('spkDiscoveryClusters').innerHTML.includes('Restored Voice'));
          assert(!document.getElementById('spkKnownVoices').innerHTML.includes('Undone Person'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_overview_known_sort_hidden_only_for_empty_results() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('overview');
          const { document, queues } = harness;
          await flush();
          await flush();

          resolveFetch(queues.quality[0], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          resolveFetch(queues.known[0], { speakers: [] });
          resolveFetch(queues.discovery[0], { clusters: [] });
          await flush();
          await flush();

          const sortSelect = document.getElementById('spkKnownSort');
          const knownContainer = document.getElementById('spkKnownVoices');
          assert.strictEqual(sortSelect.hidden, true);
          assert(knownContainer.innerHTML.includes('known empty'));

          sortSelect.dispatchEvent({ type: 'change', target: sortSelect });
          await flush();
          assert.strictEqual(queues.known.length, 2);
          resolveFetch(queues.known[1], {
            speakers: [
              {
                entity_id: 'known_person',
                name: 'Known Person',
                streams: ['daily'],
                embedding_count: 2,
                segment_count: 1,
                last_seen_ts: '2026-07-26T10:00:00Z',
                intra_cosine_p25: 0.8,
              },
            ],
          });
          await flush();
          await flush();

          assert.strictEqual(sortSelect.hidden, false);
          assert(knownContainer.innerHTML.includes('spk-known-grid'));
          assert(knownContainer.innerHTML.includes('Known Person'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_day_full_undo_refreshes_discovery_segments_and_active_review() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('day');
          const { document, queues, sheets } = harness;
          await flush();
          await flush();

          resolveFetch(queues.discovery[0], {
            clusters: [
              { cluster_id: 7, size: 1, segment_count: 1, samples: [] },
            ],
          });
          resolveFetch(queues.segments[0], { segments: [segment()], total: 1 });
          await flush();
          await flush();
          assert.strictEqual(queues.scan.length, 1);
          resolveFetch(queues.speakers[0], matchedSpeakers(''));
          resolveFetch(queues.review[0], reviewPayload(''));
          await flush();
          await flush();

          const discoveryBanner = document.getElementById('spkDiscoveryBanner');
          discoveryBanner.querySelector('.spk-who-trigger').click();
          assert.strictEqual(sheets.length, 1);
          const sheetOptions = sheets[0];

          sheetOptions.onIdentified({ clusterId: '7' });
          await flush();
          assert.strictEqual(queues.segments.length, 2);
          assert.strictEqual(queues.review.length, 2);
          assert.strictEqual(discoveryBanner.querySelector('[data-cluster-id="7"]'), null);

          const refresh = sheetOptions.onFullyRestoredUndo();
          await flush();
          assert.strictEqual(queues.discovery.length, 2);
          assert.strictEqual(queues.segments.length, 3);
          resolveFetch(queues.discovery[1], {
            clusters: [
              { cluster_id: 7, size: 1, segment_count: 1, samples: [] },
            ],
          });
          resolveFetch(queues.segments[2], { segments: [segment()], total: 1 });
          await flush();
          await flush();
          assert.strictEqual(queues.scan.length, 2);
          resolveFetch(queues.scan[1], {
            clusters: [
              { cluster_id: 7, size: 1, segment_count: 1, samples: [] },
            ],
          });
          await flush();
          await flush();

          const explicitSpeakers = queues.speakers[queues.speakers.length - 1];
          const explicitReview = queues.review[queues.review.length - 1];
          resolveFetch(explicitSpeakers, matchedSpeakers(''));
          resolveFetch(explicitReview, reviewPayload(''));
          await refresh;
          await flush();

          resolveFetch(queues.segments[1], {
            segments: [{ ...segment(), speaker_count: 1 }],
            total: 1,
          });
          resolveFetch(queues.review[1], reviewPayload('Undone Person'));
          if (queues.speakers.length > 1) {
            resolveFetch(queues.speakers[1], matchedSpeakers('Undone Person'));
          }
          if (queues.review.length > 2) {
            resolveFetch(queues.review[2], reviewPayload('Undone Person'));
          }
          resolveFetch(queues.scan[0], {
            clusters: [
              { cluster_id: 99, size: 1, segment_count: 1, samples: [] },
            ],
          });
          await flush();
          await flush();

          assert(discoveryBanner.querySelector('[data-cluster-id="7"]'));
          assert.strictEqual(discoveryBanner.querySelector('[data-cluster-id="99"]'), null);
          assert(!document.getElementById('spkSpeakers').innerHTML.includes('Undone Person'));
          assert(!document.getElementById('spkSentences').innerHTML.includes('Undone Person'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_day_discovery_failure_retry_preserves_cached_clusters() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('day');
          const { document, queues, sheets } = harness;
          await flush();
          await flush();

          resolveFetch(queues.discovery[0], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'day', '7');
          assert.strictEqual(queues.scan.length, 1);

          rejectApi(queues.scan[0], discoveryFailure('scan failed', true));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'day', '7');
          assertDiscoveryNotice(document, 'day', 'discovery error', true);

          document.getElementById('spkDiscoveryBanner').querySelector('.spk-who-trigger').click();
          sheets[0].onFullyRestoredUndo().catch(() => false);
          await flush();
          await flush();
          assert.strictEqual(queues.discovery.length, 2);
          resolveFetch(queues.discovery[1], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'day', '7');
          assertDiscoveryNotice(document, 'day', 'discovery error', true);

          discoveryRetryButton(document, 'day').click();
          await flush();
          await flush();
          resolveFetch(queues.scan[queues.scan.length - 1], discoveryOk([discoveryCluster(8, 'Cluster B')]));
          await flush();
          await flush();

          assertNoDiscoveryCluster(document, 'day', '7');
          assertDiscoveryCluster(document, 'day', '8');
          assertNoDiscoveryNotice(document, 'day', 'discovery error');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_overview_discovery_failure_retry_preserves_cached_clusters() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('overview');
          const { document, queues, sheets } = harness;
          await flush();
          await flush();

          resolveFetch(queues.quality[0], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          resolveFetch(queues.known[0], { speakers: [] });
          resolveFetch(queues.discovery[0], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'overview', '7');
          assert.strictEqual(queues.scan.length, 1);

          rejectApi(queues.scan[0], discoveryFailure('scan failed', true));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'overview', '7');
          assertDiscoveryNotice(document, 'overview', 'discovery error', true);

          document.getElementById('spkDiscoveryClusters').querySelector('.spk-who-trigger').click();
          sheets[0].onFullyRestoredUndo().catch(() => false);
          await flush();
          await flush();
          assert.strictEqual(queues.discovery.length, 2);
          resolveFetch(queues.discovery[1], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'overview', '7');
          assertDiscoveryNotice(document, 'overview', 'discovery error', true);

          discoveryRetryButton(document, 'overview').click();
          await flush();
          await flush();
          resolveFetch(queues.scan[queues.scan.length - 1], discoveryOk([discoveryCluster(8, 'Cluster B')]));
          await flush();
          await flush();

          assertNoDiscoveryCluster(document, 'overview', '7');
          assertDiscoveryCluster(document, 'overview', '8');
          assertNoDiscoveryNotice(document, 'overview', 'discovery error');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_day_discovery_stale_failure_does_not_hide_newer_success() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('day');
          const { document, queues, sheets } = harness;
          await flush();
          await flush();

          resolveFetch(queues.discovery[0], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          const staleScan = queues.scan[0];
          document.getElementById('spkDiscoveryBanner').querySelector('.spk-who-trigger').click();
          sheets[0].onFullyRestoredUndo().catch(() => false);
          await flush();
          await flush();

          resolveFetch(queues.discovery[1], discoveryOk([discoveryCluster(8, 'Cluster B')]));
          await flush();
          await flush();
          resolveFetch(queues.scan[1], discoveryOk([discoveryCluster(8, 'Cluster B')]));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'day', '8');
          assertNoDiscoveryNotice(document, 'day', 'discovery error');

          rejectApi(staleScan, discoveryFailure('scan failed', true));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'day', '8');
          assertNoDiscoveryNotice(document, 'day', 'discovery error');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_overview_discovery_stale_failure_does_not_hide_newer_success() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('overview');
          const { document, queues, sheets } = harness;
          await flush();
          await flush();

          resolveFetch(queues.quality[0], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          resolveFetch(queues.known[0], { speakers: [] });
          resolveFetch(queues.discovery[0], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          const staleScan = queues.scan[0];
          document.getElementById('spkDiscoveryClusters').querySelector('.spk-who-trigger').click();
          sheets[0].onFullyRestoredUndo().catch(() => false);
          await flush();
          await flush();

          rejectApi(queues.discovery[1], discoveryFailure('cache failed', true));
          await flush();
          await flush();
          assertDiscoveryNotice(document, 'overview', 'discovery error', true);
          discoveryRetryButton(document, 'overview').click();
          await flush();
          await flush();
          resolveFetch(queues.scan[1], discoveryOk([discoveryCluster(8, 'Cluster B')]));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'overview', '8');
          assertNoDiscoveryNotice(document, 'overview', 'discovery error');

          rejectApi(staleScan, discoveryFailure('scan failed', true));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'overview', '8');
          assertNoDiscoveryNotice(document, 'overview', 'discovery error');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_day_discovery_stale_success_does_not_erase_newer_error() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('day');
          const { document, queues, sheets } = harness;
          await flush();
          await flush();

          resolveFetch(queues.discovery[0], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          const staleScan = queues.scan[0];
          document.getElementById('spkDiscoveryBanner').querySelector('.spk-who-trigger').click();
          sheets[0].onFullyRestoredUndo().catch(() => false);
          await flush();
          await flush();

          resolveFetch(queues.discovery[1], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          rejectApi(queues.scan[1], discoveryFailure('scan failed', true));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'day', '7');
          assertDiscoveryNotice(document, 'day', 'discovery error', true);

          resolveFetch(staleScan, discoveryOk([discoveryCluster(8, 'Cluster B')]));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'day', '7');
          assertNoDiscoveryCluster(document, 'day', '8');
          assertDiscoveryNotice(document, 'day', 'discovery error', true);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_overview_discovery_stale_success_does_not_erase_newer_error() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('overview');
          const { document, queues, sheets } = harness;
          await flush();
          await flush();

          resolveFetch(queues.quality[0], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          resolveFetch(queues.known[0], { speakers: [] });
          resolveFetch(queues.discovery[0], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          const staleScan = queues.scan[0];
          document.getElementById('spkDiscoveryClusters').querySelector('.spk-who-trigger').click();
          sheets[0].onFullyRestoredUndo().catch(() => false);
          await flush();
          await flush();

          rejectApi(queues.discovery[1], discoveryFailure('cache failed', true));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'overview', '7');
          assertDiscoveryNotice(document, 'overview', 'discovery error', true);
          discoveryRetryButton(document, 'overview').click();
          await flush();
          await flush();
          rejectApi(queues.scan[1], discoveryFailure('scan failed', true));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'overview', '7');
          assertDiscoveryNotice(document, 'overview', 'discovery error', true);

          resolveFetch(staleScan, discoveryOk([discoveryCluster(8, 'Cluster B')]));
          await flush();
          await flush();
          assertDiscoveryCluster(document, 'overview', '7');
          assertNoDiscoveryCluster(document, 'overview', '8');
          assertDiscoveryNotice(document, 'overview', 'discovery error', true);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_day_discovery_degraded_notice_keeps_clusters_and_empty_scan() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          const withCluster = makeWorkspaceContext('day');
          await flush();
          await flush();
          resolveFetch(withCluster.queues.discovery[0], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          resolveFetch(withCluster.queues.scan[0], discoveryDegraded([], 2));
          await flush();
          await flush();
          assertDiscoveryCluster(withCluster.document, 'day', '7');
          assertDiscoveryNotice(withCluster.document, 'day', 'degraded 2', false);
          assert.strictEqual(discoveryRetryButton(withCluster.document, 'day'), null);

          const empty = makeWorkspaceContext('day');
          await flush();
          await flush();
          resolveFetch(empty.queues.discovery[0], discoveryOk([]));
          await flush();
          await flush();
          resolveFetch(empty.queues.scan[0], discoveryDegraded([], 3));
          await flush();
          await flush();
          assertDiscoveryNotice(empty.document, 'day', 'degraded 3', false);
          assert.strictEqual(empty.document.getElementById('spkDiscoveryBanner').style.display, 'block');
          assertNoDiscoveryCluster(empty.document, 'day', '7');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_overview_discovery_degraded_notice_keeps_clusters_and_empty_scan() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          const withCluster = makeWorkspaceContext('overview');
          await flush();
          await flush();
          resolveFetch(withCluster.queues.quality[0], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          resolveFetch(withCluster.queues.known[0], { speakers: [] });
          resolveFetch(withCluster.queues.discovery[0], discoveryOk([discoveryCluster(7, 'Cluster A')]));
          await flush();
          await flush();
          resolveFetch(withCluster.queues.scan[0], discoveryDegraded([], 2));
          await flush();
          await flush();
          assertDiscoveryCluster(withCluster.document, 'overview', '7');
          assertDiscoveryNotice(withCluster.document, 'overview', 'degraded 2', false);
          assert.strictEqual(discoveryRetryButton(withCluster.document, 'overview'), null);

          const empty = makeWorkspaceContext('overview');
          await flush();
          await flush();
          resolveFetch(empty.queues.quality[0], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          resolveFetch(empty.queues.known[0], { speakers: [] });
          resolveFetch(empty.queues.discovery[0], discoveryOk([]));
          await flush();
          await flush();
          resolveFetch(empty.queues.scan[0], discoveryDegraded([], 3));
          await flush();
          await flush();
          assertDiscoveryNotice(empty.document, 'overview', 'degraded 3', false);
          assert.strictEqual(empty.document.getElementById('spkNewVoicesSection').hidden, false);
          assertNoDiscoveryCluster(empty.document, 'overview', '7');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_overview_this_is_me_focuses_your_voice_section() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('overview');
          const { document, queues, sheets } = harness;
          await flush();
          await flush();

          resolveFetch(queues.quality[0], {
            owner_voice: { bootstrap_state: 'ready' },
            tier_histogram: {},
            corrections_window_count: 0,
          });
          resolveFetch(queues.known[0], { speakers: [] });
          resolveFetch(queues.discovery[0], {
            clusters: [
              { cluster_id: 7, suggested_name: 'Initial Voice', size: 1, samples: [] },
            ],
          });
          await flush();
          await flush();

          document.getElementById('spkDiscoveryClusters').querySelector('.spk-who-trigger').click();
          assert.strictEqual(sheets.length, 1);
          sheets[0].onThisIsMe();

          const target = document.getElementById('spkOverviewYourVoiceSection');
          assert.strictEqual(document.activeElement, target);
          assert.strictEqual(target.scrollIntoViewCalls.length, 1);
          assert.strictEqual(target.scrollIntoViewCalls[0].block, 'nearest');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_day_this_is_me_focuses_owner_banner_before_owner_help() -> None:
    _run_workspace_node(
        """
        (async () => {
          function todayKey() {
            const now = new Date();
            return `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
          }
          const today = todayKey();
          const harness = makeWorkspaceContext('day', {
            day: today,
            today,
            hash: '',
          });
          const { document, queues, sheets, window } = harness;
          await flush();
          await flush();
          resolveFetch(queues.discovery[0], {
            clusters: [
              { cluster_id: 7, size: 1, segment_count: 1, samples: [] },
            ],
          });
          resolveFetch(queues.segments[0], { segments: [segment()], total: 1 });
          await flush();
          await flush();

          document.getElementById('spkDiscoveryBanner').querySelector('.spk-who-trigger').click();
          assert.strictEqual(sheets.length, 1);
          sheets[0].onThisIsMe();

          const target = document.getElementById('spkOwnerBanner');
          assert.strictEqual(document.activeElement, target);
          assert.strictEqual(target.scrollIntoViewCalls.at(-1).block, 'nearest');
          assert.strictEqual(target.getAttribute('aria-label'), 'your voice');
          assert.strictEqual(window.location.hash, '#owner-teach');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_non_today_this_is_me_routes_and_destination_focuses_owner_banner() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          function todayKey() {
            const now = new Date();
            return `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
          }
          const today = todayKey();
          const oldHarness = makeWorkspaceContext('day', {
            day: '20000101',
            today,
            hash: '',
          });
          await flush();
          await flush();
          resolveFetch(oldHarness.queues.discovery[0], {
            clusters: [
              { cluster_id: 7, size: 1, segment_count: 1, samples: [] },
            ],
          });
          resolveFetch(oldHarness.queues.segments[0], { segments: [segment()], total: 1 });
          await flush();
          await flush();

          oldHarness.document.getElementById('spkDiscoveryBanner').querySelector('.spk-who-trigger').click();
          oldHarness.sheets[0].onThisIsMe();
          assert.strictEqual(oldHarness.window.location.href, `/app/speakers/${today}#owner-teach`);

          const destination = makeWorkspaceContext('day', {
            day: today,
            today,
            hash: '#owner-teach',
            ownerStatus: {
              status: 'no_cluster',
            },
          });
          await flush();
          await flush();

          const target = destination.document.getElementById('spkOwnerBanner');
          assert.strictEqual(destination.document.activeElement, target);
          assert.strictEqual(target.scrollIntoViewCalls.length, 1);
          assert.strictEqual(target.scrollIntoViewCalls[0].block, 'nearest');
          assert.strictEqual(assertOwnerBannerVisible(destination.document, 'guidance'), target);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_owner_teach_handoff_keeps_banner_visible_for_confirmed_and_fallback() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          async function assertStatus(status) {
            const harness = makeWorkspaceContext('day', {
              hash: '#owner-teach',
              ownerStatus: status,
            });
            await flush();
            await flush();
            assertOwnerBannerVisible(harness.document, 'guidance');
          }

          await assertStatus({ status: 'confirmed', centroid_metadata: {} });
          await assertStatus({ status: 'unhandled_status' });
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_owner_status_latest_authoritative_deferred_settles_old_first() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          function todayKey() {
            const now = new Date();
            return `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
          }
          const today = todayKey();
          const harness = makeWorkspaceContext('day', {
            day: today,
            today,
            hash: '#owner-teach',
            deferOwnerStatus: true,
          });
          const { document, queues, window } = harness;
          await flush();
          await flush();
          dispatchHashChange(window);
          await flush();
          assert.strictEqual(queues.ownerStatus.length, 2);

          resolveApi(queues.ownerStatus[0], { status: 'candidate', samples: [] });
          await flush();
          await flush();
          let target = assertOwnerBannerVisible(document, 'guidance');
          assert(!target.textContent.includes('is this your voice?'));

          resolveApi(queues.ownerStatus[1], { status: 'no_cluster' });
          await flush();
          await flush();
          target = assertOwnerBannerVisible(document, 'guidance');
          assert(!target.textContent.includes('is this your voice?'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_owner_status_latest_authoritative_deferred_settles_new_first() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          function todayKey() {
            const now = new Date();
            return `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
          }
          const today = todayKey();
          const harness = makeWorkspaceContext('day', {
            day: today,
            today,
            hash: '#owner-teach',
            deferOwnerStatus: true,
          });
          const { document, queues, window } = harness;
          await flush();
          await flush();
          dispatchHashChange(window);
          await flush();
          assert.strictEqual(queues.ownerStatus.length, 2);

          resolveApi(queues.ownerStatus[1], { status: 'no_cluster' });
          await flush();
          await flush();
          let target = assertOwnerBannerVisible(document, 'guidance');
          assert(!target.textContent.includes('is this your voice?'));

          resolveApi(queues.ownerStatus[0], { status: 'candidate', samples: [] });
          await flush();
          await flush();
          target = assertOwnerBannerVisible(document, 'guidance');
          assert(!target.textContent.includes('is this your voice?'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_owner_teach_detection_followup_confirmed_keeps_banner() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('day', {
            hash: '#owner-teach',
            deferOwnerStatus: true,
          });
          const { document, queues } = harness;
          await flush();
          await flush();

          resolveApi(queues.ownerStatus[0], { status: 'needs_detection' });
          await flush();
          await flush();
          assert.strictEqual(queues.ownerStatus.length, 2);
          assertOwnerBannerVisible(document, 'teach loading');

          resolveApi(queues.ownerStatus[1], { status: 'confirmed', centroid_metadata: {} });
          await flush();
          await flush();
          assertOwnerBannerVisible(document, 'teach loading');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_owner_teach_detection_followup_no_cluster_keeps_banner() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('day', {
            hash: '#owner-teach',
            deferOwnerStatus: true,
          });
          const { document, queues } = harness;
          await flush();
          await flush();

          resolveApi(queues.ownerStatus[0], { status: 'needs_detection' });
          await flush();
          await flush();
          assert.strictEqual(queues.ownerStatus.length, 2);
          assertOwnerBannerVisible(document, 'teach loading');

          resolveApi(queues.ownerStatus[1], { status: 'no_cluster' });
          await flush();
          await flush();
          assertOwnerBannerVisible(document, 'teach loading');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_owner_teach_visible_outcome_replaces_guidance_without_refocus() -> (
    None
):
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('day', {
            hash: '#owner-teach',
            ownerStatus: { status: 'candidate', samples: [] },
          });
          const { document } = harness;
          await flush();
          await flush();

          const target = assertOwnerBannerVisible(document, 'is this your voice?');
          assert(!target.textContent.includes('guidance'));
          assert.strictEqual(target.scrollIntoViewCalls.length, 1);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_hashchange_owner_teach_begins_and_reuses_generation() -> None:
    _run_workspace_node(
        """
        (async () => {
          function todayKey() {
            const now = new Date();
            return `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
          }
          const today = todayKey();
          const harness = makeWorkspaceContext('day', {
            day: today,
            today,
            hash: '',
            deferOwnerStatus: true,
          });
          const { document, window } = harness;
          await flush();
          await flush();

          window.location.hash = '#owner-teach';
          dispatchHashChange(window);
          await flush();
          let target = assertOwnerBannerVisible(document, 'guidance');
          const firstPanel = target.firstChild;
          assert.strictEqual(target.scrollIntoViewCalls.length, 1);

          dispatchHashChange(window);
          await flush();
          target = assertOwnerBannerVisible(document, 'guidance');
          assert.strictEqual(target.firstChild, firstPanel);
          assert.strictEqual(target.scrollIntoViewCalls.length, 1);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_owner_teach_hash_away_invalidates_late_status_settle() -> None:
    _run_workspace_node(
        """
        (async () => {
          const harness = makeWorkspaceContext('day', {
            hash: '#owner-teach',
            deferOwnerStatus: true,
          });
          const { document, queues, window } = harness;
          await flush();
          await flush();

          const target = assertOwnerBannerVisible(document, 'guidance');
          const originalText = target.textContent;
          const nextFocus = document.getElementById('spkSegmentList');
          window.location.hash = '#seg-a';
          nextFocus.focus();
          dispatchHashChange(window);
          await flush();

          resolveApi(queues.ownerStatus[0], { status: 'candidate', samples: [] });
          await flush();
          await flush();
          assert.strictEqual(document.activeElement, nextFocus);
          assert.strictEqual(target.textContent, originalText);
          assert(!target.textContent.includes('is this your voice?'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_owner_status_ordinary_load_still_hides_without_handoff() -> None:
    _run_workspace_node(
        """
        (async () => {
          async function assertHidden(status) {
            const harness = makeWorkspaceContext('day', {
              hash: '',
              ownerStatus: status,
            });
            await flush();
            await flush();
            assertOwnerBannerHidden(harness.document);
          }

          await assertHidden({ status: 'no_cluster' });
          await assertHidden({ status: 'confirmed', centroid_metadata: {} });
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_workspace_owner_teach_route_state_failure_focuses_error_region() -> None:
    _run_workspace_node(
        """
        (async () => {
          const err = new Error('status failed');
          err.serverMessage = 'offline';
          const harness = makeWorkspaceContext('day', {
            hash: '#owner-teach',
            ownerStatusError: err,
          });
          const { document } = harness;
          await flush();
          await flush();

          const target = document.getElementById('spkOwnerBanner');
          assert.strictEqual(assertOwnerBannerVisible(document, "Couldn't load owner status"), target);
          assert.strictEqual(target.scrollIntoViewCalls.length, 1);
          assert.strictEqual(target.scrollIntoViewCalls[0].block, 'nearest');
          assert(target.textContent.includes('offline'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_resolve_branching_keep_separate_and_request_ids() -> None:
    _run_node(
        """
        (async () => {
          const calls = [];
          const apiJson = (url, request) => {
            calls.push({ url, body: request?.body ? JSON.parse(request.body) : null });
            if (url.includes('/presence')) return Promise.resolve(presence());
            if (url.includes('/identify')) {
              return Promise.resolve({
                status: 'identified',
                entity_name: 'Alicia New',
                operation_id: 'idop_one',
              });
            }
            return Promise.resolve({});
          };
          const { doc, controller, trigger } = makeHarness({ apiJson });
          await controller.open({ cluster: { cluster_id: 7 }, trigger });

          controller.enterPreview({
            mode: 'attach',
            entity_id: 'alice',
            name: 'Alice Example',
            has_voice: false,
          });
          assert.strictEqual(controller.requestId, 'req-1');
          controller.enterPreview({
            mode: 'attach',
            entity_id: 'alice',
            name: 'Alice Example',
            has_voice: false,
          });
          assert.strictEqual(controller.requestId, 'req-1');
          controller.enterPreview({
            mode: 'attach',
            entity_id: 'bob',
            name: 'Bob Example',
            has_voice: false,
          });
          assert.strictEqual(controller.requestId, 'req-2');

          controller.handleResolveResult('Alicia New', {
            status: 'ambiguous',
            candidates: [
              { id: 'alice', name: 'Alice Near' },
              { id: 'ally', name: 'Ally Near' },
            ],
          });
          assert(text(doc.body).includes('near band'));
          click(doc.body.querySelector('.spk-who-create-row'));
          assert(text(doc.body).includes('different Alice Near'));
          click(doc.body.querySelector('.spk-who-keep-decline'));
          assert(text(doc.body).includes('preview Alice Near'));

          controller.handleResolveResult('Alicia New', {
            status: 'no_match',
            candidates: [
              { id: 'alice', name: 'Alice Near' },
              { id: 'ally', name: 'Ally Near' },
            ],
          });
          click(doc.body.querySelector('.spk-who-create-row'));
          click(doc.body.querySelector('.spk-who-keep-confirm'));
          assert(text(doc.body).includes('preview Alicia New'));
          click(doc.body.querySelector('.spk-who-confirm'));
          await Promise.resolve();

          const commit = calls.find((call) => call.body?.reviewed_near_match_entity_ids);
          assert.deepStrictEqual(commit.body.reviewed_near_match_entity_ids, ['alice', 'ally']);
          assert.strictEqual(commit.body.create_new, true);
          assert.strictEqual(commit.body.request_id, controller.requestId);

          controller.handleResolveResult('Alicia New', {
            status: 'no_match',
            candidates: [],
          });
          assert(text(doc.body).includes('preview Alicia New'));

          controller.handleResolveResult('Broken', { status: 'partial' });
          assert(text(doc.body).includes('check error'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_commit_failure_uses_load_error_and_resolve_uses_check_error() -> (
    None
):
    _run_node(
        """
        (async () => {
          const apiJson = (url, request) => {
            if (url.includes('/presence')) return Promise.resolve(presence());
            if (url.includes('/identify') && request?.body?.includes('"resolve_only":true')) {
              return Promise.reject(new Error('resolve failed'));
            }
            if (url.includes('/identify')) return Promise.reject(new Error('commit failed'));
            return Promise.resolve({});
          };
          const { doc, controller, trigger, logs } = makeHarness({ apiJson });
          await controller.open({ cluster: { cluster_id: 7 }, trigger });
          await controller.resolveCreateName('Alicia');
          assert(text(doc.body).includes('check error'));

          controller.enterPreview({
            mode: 'attach',
            entity_id: 'alice',
            name: 'Alice',
            has_voice: false,
          });
          await controller.commitPreview();
          assert(text(doc.body).includes('load error'));
          assert.strictEqual(logs.length, 2);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_receipt_undo_full_partial_and_dismissals() -> None:
    _run_node(
        """
        (async () => {
          let presenceLoads = 0;
          const calls = [];
          const apiJson = (url, request) => {
            const body = request?.body ? JSON.parse(request.body) : null;
            calls.push({ url, body });
            if (url.includes('/presence')) {
              presenceLoads += 1;
              return Promise.resolve(presence());
            }
            if (url.includes('/identify/undo')) {
              return Promise.resolve({
                status: 'undone',
                undo_report: {
                  labels: { restored_count: 1, skipped_count: 0 },
                  corrections: { restored_count: 1, skipped_count: 0 },
                  voiceprints: { restored_count: 1, skipped_count: 0 },
                  tracker: { restored_count: 1, skipped_count: 0 },
                  sentinel: { restored_count: 1, skipped_count: 0 },
                  entity: {
                    restored_count: 1,
                    skipped_count: 0,
                    blocked_categories: [],
                    keep_separate_sources_removed_count: 99,
                  },
                },
              });
            }
            if (url.includes('/dismiss')) {
              return Promise.resolve({ status: 'dismissed', disposition: body.disposition });
            }
            if (url.includes('/identify')) {
              return Promise.resolve({
                status: 'identified',
                entity_name: 'Alice',
                operation_id: 'idop_one',
              });
            }
            return Promise.resolve({});
          };
          const dismissed = [];
          const { doc, controller, trigger } = makeHarness({
            apiJson,
            onDismissed: (payload) => dismissed.push(payload),
          });
          await controller.open({ cluster: { cluster_id: 7 }, trigger });
          controller.enterPreview({
            mode: 'attach',
            entity_id: 'alice',
            name: 'Alice',
            has_voice: false,
          });
          await controller.commitPreview();
          assert(text(doc.body).includes('receipt Alice'));
          await controller.undoReceipt();
          assert.strictEqual(presenceLoads, 2);
          assert(text(doc.body).includes('undo done'));

          const partial = who.summarizeUndoReport({
            status: 'undone',
            undo_report: {
              labels: { restored_count: 1, skipped_count: 2 },
              corrections: { restored_count: 1, skipped_count: 0 },
              voiceprints: { restored_count: 1, skipped_count: 0 },
              tracker: { restored_count: 1, skipped_count: 0 },
              sentinel: { restored_count: 1, skipped_count: 0 },
              entity: {
                restored_count: 1,
                skipped_count: 0,
                blocked_categories: ['keep_separate'],
                keep_separate_sources_removed_count: 99,
              },
            },
          });
          assert.deepStrictEqual(partial, {
            restored: 6,
            skipped: 2,
            blocked_categories: ['keep_separate'],
            fully_restored: false,
          });

          await controller.dismissCluster('not_a_person');
          await controller.dismissCluster('quiet');
          assert.deepStrictEqual(
            calls.filter((call) => call.url.includes('/dismiss')).map((call) => call.body.disposition),
            ['not_a_person', 'quiet'],
          );
          assert.strictEqual(dismissed.length, 2);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_full_undo_refresh_callback_only_for_fully_restored() -> None:
    _run_node(
        """
        (async () => {
          const outcomes = [
            {
              name: 'full',
              result: fullUndoResult(),
              expectedRefreshes: 1,
              expectedText: 'undo done',
            },
            {
              name: 'partial',
              result: fullUndoResult({
                undo_report: {
                  labels: { restored_count: 1, skipped_count: 1 },
                  corrections: { restored_count: 0, skipped_count: 0 },
                  voiceprints: { restored_count: 0, skipped_count: 0 },
                  tracker: { restored_count: 0, skipped_count: 0 },
                  sentinel: { restored_count: 0, skipped_count: 0 },
                  entity: { restored_count: 0, skipped_count: 0, blocked_categories: [] },
                },
              }),
              expectedRefreshes: 0,
              expectedText: 'partial',
            },
            {
              name: 'skipped',
              result: fullUndoResult({
                undo_report: {
                  labels: { restored_count: 0, skipped_count: 0 },
                  corrections: { restored_count: 0, skipped_count: 0 },
                  voiceprints: { restored_count: 0, skipped_count: 0 },
                  tracker: { restored_count: 0, skipped_count: 0 },
                  sentinel: { restored_count: 0, skipped_count: 0 },
                  entity: {
                    restored_count: 0,
                    skipped_count: 0,
                    blocked_categories: ['keep_separate'],
                  },
                },
              }),
              expectedRefreshes: 0,
              expectedText: 'partial',
            },
            {
              name: 'repair-required',
              result: fullUndoResult({ status: 'undo_repair_required' }),
              expectedRefreshes: 0,
              expectedText: 'partial',
            },
            {
              name: 'failed',
              result: fullUndoResult({ status: 'failed' }),
              expectedRefreshes: 0,
              expectedText: 'partial',
            },
            {
              name: 'still-undoing',
              result: fullUndoResult({ status: 'undoing' }),
              expectedRefreshes: 0,
              expectedText: 'partial',
            },
          ];

          for (const outcome of outcomes) {
            let refreshes = 0;
            let undoCalls = 0;
            let presenceLoads = 0;
            const apiJson = (url) => {
              if (url.includes('/presence')) {
                presenceLoads += 1;
                return Promise.resolve(presence());
              }
              if (url.includes('/identify/undo')) {
                undoCalls += 1;
                return Promise.resolve(outcome.result);
              }
              return Promise.resolve({});
            };
            const { doc, controller, trigger } = makeHarness({
              apiJson,
              onFullyRestoredUndo: async () => { refreshes += 1; },
            });
            await controller.open({ cluster: { cluster_id: 7 }, trigger });
            controller.renderReceipt({ operation_id: `op-${outcome.name}`, entity_name: 'Alice' });
            await controller.undoReceipt();

            assert.strictEqual(undoCalls, 1, outcome.name);
            assert.strictEqual(refreshes, outcome.expectedRefreshes, outcome.name);
            assert(text(doc.body).includes(outcome.expectedText), outcome.name);
            assert.strictEqual(
              presenceLoads,
              outcome.expectedRefreshes ? 2 : 1,
              outcome.name,
            );
          }
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_request_ids_survive_retries_sort_reviewed_and_reset_after_undo() -> (
    None
):
    _run_node(
        """
        (async () => {
          const identifyCalls = [];
          const apiJson = (url, request) => {
            const body = request?.body ? JSON.parse(request.body) : null;
            if (url.includes('/presence')) return Promise.resolve(presence());
            if (url.includes('/identify/undo')) return Promise.resolve(fullUndoResult());
            if (url.includes('/identify')) {
              identifyCalls.push(body);
              return Promise.reject(new Error('lost response'));
            }
            return Promise.resolve({});
          };
          const { controller, trigger } = makeHarness({
            apiJson,
            onFullyRestoredUndo: async () => {},
          });
          await controller.open({ cluster: { cluster_id: 7 }, trigger });

          controller.enterPreview({
            mode: 'create',
            name: 'Alicia New',
            reviewed_near_match_entity_ids: ['ally', 'alice'],
          });
          assert.strictEqual(controller.requestId, 'req-1');
          controller.enterPreview({
            mode: 'create',
            name: 'Alicia New',
            reviewed_near_match_entity_ids: ['alice', 'ally'],
          });
          assert.strictEqual(controller.requestId, 'req-1');

          await controller.commitPreview();
          await controller.commitPreview();
          assert.strictEqual(identifyCalls.length, 2);
          assert.strictEqual(identifyCalls[0].request_id, 'req-1');
          assert.strictEqual(identifyCalls[1].request_id, 'req-1');

          controller.renderReceipt({ operation_id: 'op-one', entity_name: 'Alicia New' });
          await controller.undoReceipt();
          assert.strictEqual(controller.requestId, '');
          assert.strictEqual(controller.requestSignature, '');

          controller.enterPreview({
            mode: 'create',
            name: 'Alicia New',
            reviewed_near_match_entity_ids: ['alice', 'ally'],
          });
          assert.strictEqual(controller.requestId, 'req-2');
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_full_undo_refresh_retry_is_read_only() -> None:
    _run_node(
        """
        (async () => {
          let undoCalls = 0;
          let refreshCalls = 0;
          let presenceLoads = 0;
          const apiJson = (url) => {
            if (url.includes('/presence')) {
              presenceLoads += 1;
              return Promise.resolve(presence());
            }
            if (url.includes('/identify/undo')) {
              undoCalls += 1;
              return Promise.resolve(fullUndoResult());
            }
            return Promise.resolve({});
          };
          const { doc, controller, trigger } = makeHarness({
            apiJson,
            onFullyRestoredUndo: async () => {
              refreshCalls += 1;
              if (refreshCalls === 1) throw new Error('refresh failed');
            },
          });
          await controller.open({ cluster: { cluster_id: 7 }, trigger });
          controller.renderReceipt({ operation_id: 'op-one', entity_name: 'Alice' });
          await controller.undoReceipt();

          assert.strictEqual(undoCalls, 1);
          assert.strictEqual(refreshCalls, 1);
          assert.strictEqual(presenceLoads, 1);
          assert(text(doc.body).includes('load error'));

          click(doc.body.querySelector('.spk-who-retry'));
          await flush();
          await flush();

          assert.strictEqual(undoCalls, 1);
          assert.strictEqual(refreshCalls, 2);
          assert.strictEqual(presenceLoads, 2);
          assert(text(doc.body).includes('undo done'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_stale_reviewed_set_refetches_resolve_only_gate() -> None:
    _run_node(
        """
        (async () => {
          const calls = [];
          const apiJson = (url, request) => {
            const body = request?.body ? JSON.parse(request.body) : null;
            calls.push({ url, body });
            if (url.includes('/presence')) return Promise.resolve(presence());
            if (url.includes('/identify') && body?.resolve_only) {
              return Promise.resolve({
                status: 'no_match',
                candidates: [
                  { id: 'fresh_one', name: 'Fresh One' },
                  { id: 'fresh_two', name: 'Fresh Two' },
                ],
              });
            }
            if (url.includes('/identify')) {
              const err = new Error('stale set');
              err.payload = {
                reason_code: 'invalid_request_value',
                invalid_request_code: 'reviewed_near_match_set_mismatch',
              };
              return Promise.reject(err);
            }
            return Promise.resolve({});
          };
          const { doc, controller, trigger } = makeHarness({ apiJson });
          await controller.open({ cluster: { cluster_id: 7 }, trigger });
          controller.enterPreview({
            mode: 'create',
            name: 'Alicia New',
            reviewed_near_match_entity_ids: ['stale_one'],
          });
          await controller.commitPreview();

          const identifyCalls = calls.filter((call) => call.url.includes('/identify'));
          assert.strictEqual(identifyCalls.length, 2);
          assert.strictEqual(identifyCalls[0].body.create_new, true);
          assert.strictEqual(identifyCalls[1].body.resolve_only, true);
          assert.strictEqual(controller.requestId, '');
          assert(text(doc.body).includes('near band'));
          assert(text(doc.body).includes('Fresh One'));
          assert(text(doc.body).includes('Fresh Two'));
          assert(!text(doc.body).includes('load error'));
          assert(!text(doc.body).includes('stale set'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_invalid_request_without_stale_code_does_not_refetch() -> None:
    _run_node(
        """
        (async () => {
          const calls = [];
          const apiJson = (url, request) => {
            const body = request?.body ? JSON.parse(request.body) : null;
            calls.push({ url, body });
            if (url.includes('/presence')) return Promise.resolve(presence());
            if (url.includes('/identify') && body?.resolve_only) {
              throw new Error('resolve_only should not be called');
            }
            if (url.includes('/identify')) {
              const err = new Error('name is unavailable');
              err.payload = { reason_code: 'invalid_request_value' };
              return Promise.reject(err);
            }
            return Promise.resolve({});
          };
          const { doc, controller, trigger } = makeHarness({ apiJson });
          await controller.open({ cluster: { cluster_id: 7 }, trigger });
          controller.enterPreview({
            mode: 'create',
            name: 'Blocked Person',
            reviewed_near_match_entity_ids: ['stale_one'],
          });
          await controller.commitPreview();

          const identifyCalls = calls.filter((call) => call.url.includes('/identify'));
          assert.strictEqual(identifyCalls.length, 1);
          assert.strictEqual(identifyCalls[0].body.create_new, true);
          assert(text(doc.body).includes('load error'));
          assert(!text(doc.body).includes('near band'));
          assert(!text(doc.body).includes('name is unavailable'));
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )


def test_who_is_this_this_is_me_callback_and_back_preserves_main_state() -> None:
    _run_node(
        """
        (async () => {
          const callbacks = [];
          const { doc, controller, trigger } = makeHarness({
            onThisIsMe: (payload) => callbacks.push(payload),
          });
          await controller.open({ cluster: { cluster_id: 7 }, trigger });
          controller.mainState.query = 'alice';
          controller.mainState.people = [{ entity_id: 'alice', name: 'Alice', has_voice: false }];
          controller.mainState.searchComplete = true;
          controller.body.scrollTop = 42;
          controller.enterPreview({
            mode: 'attach',
            entity_id: 'alice',
            name: 'Alice',
            has_voice: false,
          });
          click(doc.body.querySelector('.spk-who-preview-return'));
          assert.strictEqual(doc.body.querySelector('.spk-who-search-input').value, 'alice');
          assert(text(doc.body).includes('Alice'));
          assert.strictEqual(controller.body.scrollTop, 42);

          click(doc.body.querySelector('.spk-who-this-is-me'));
          assert.strictEqual(callbacks.length, 1);
          assert.strictEqual(callbacks[0].clusterId, '7');
          assert.strictEqual(trigger.getAttribute('aria-expanded'), 'false');
          assert.notStrictEqual(doc.activeElement, trigger);
        })().catch((error) => { console.error(error); process.exit(1); });
        """
    )
