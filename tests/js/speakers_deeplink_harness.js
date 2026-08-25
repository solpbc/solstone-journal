// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const repoRoot = path.resolve(process.argv[2] || path.join(__dirname, '../../../..'));
const shellBootSource = fs.readFileSync(
  path.join(repoRoot, 'solstone/convey/static/shell_boot.js'),
  'utf8'
);
const mountWorkspaceSource = fs.readFileSync(
  path.join(repoRoot, 'solstone/convey/static/mount-workspace.js'),
  'utf8'
);
const workspaceHtml = fs.readFileSync(
  path.join(repoRoot, 'core/crates/solstone-core-convey-shell/assets/speakers/workspace.html'),
  'utf8'
);
const whoIsThisSource = fs.readFileSync(
  path.join(repoRoot, 'core/crates/solstone-core-convey-shell/assets/speakers/who_is_this.js'),
  'utf8'
);

const backgroundDelayMs = Number(process.env.SPK_HARNESS_BACKGROUND_DELAY_MS || '0');
const includeBackground = backgroundDelayMs > 0;

class FakeEvent {
  constructor(type, props = {}) {
    this.type = type;
    this.detail = props.detail || null;
    this.key = props.key || '';
    this.shiftKey = Boolean(props.shiftKey);
    this.target = props.target || null;
    this.defaultPrevented = false;
  }

  preventDefault() {
    this.defaultPrevented = true;
  }
}

class FakeClassList {
  constructor(node) {
    this.node = node;
  }

  _set() {
    return new Set(String(this.node.className || '').split(/\s+/).filter(Boolean));
  }

  _save(values) {
    this.node.className = Array.from(values).join(' ');
    this.node._attrMap.class = this.node.className;
    this.node._refreshAttributesProxy();
  }

  add(...names) {
    const values = this._set();
    names.forEach((name) => values.add(name));
    this._save(values);
  }

  remove(...names) {
    const values = this._set();
    names.forEach((name) => values.delete(name));
    this._save(values);
  }

  toggle(name, force) {
    const values = this._set();
    const shouldAdd = force === undefined ? !values.has(name) : Boolean(force);
    if (shouldAdd) values.add(name);
    else values.delete(name);
    this._save(values);
    return shouldAdd;
  }

  contains(name) {
    return this._set().has(name);
  }
}

function dataKey(name) {
  return name.slice(5).replace(/-([a-z])/g, (_match, char) => char.toUpperCase());
}

function parseAttributes(raw) {
  const attrs = [];
  const attrRe = /([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g;
  let match;
  while ((match = attrRe.exec(raw || '')) !== null) {
    attrs.push([match[1], match[2] ?? match[3] ?? match[4] ?? '']);
  }
  return attrs;
}

function isVoidTag(tagName) {
  return new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']).has(tagName);
}

function splitSelector(selector) {
  return selector.split(',').map((part) => part.trim()).filter(Boolean);
}

function splitDescendantSelector(selector) {
  const parts = [];
  let current = '';
  let bracketDepth = 0;
  for (const char of selector) {
    if (char === '[') bracketDepth += 1;
    if (char === ']') bracketDepth -= 1;
    if (/\s/.test(char) && bracketDepth === 0) {
      if (current.trim()) parts.push(current.trim());
      current = '';
    } else {
      current += char;
    }
  }
  if (current.trim()) parts.push(current.trim());
  return parts;
}

function matchesSimple(node, selector) {
  if (!node || node.nodeType !== 1) return false;
  if (selector === '*') return true;
  if (selector === 'button:not([disabled])') {
    return node.tagName === 'BUTTON' && !node.disabled;
  }
  if (selector === 'input:not([disabled])') {
    return node.tagName === 'INPUT' && !node.disabled;
  }
  if (selector === '[tabindex]:not([tabindex="-1"])') {
    const value = node.getAttribute('tabindex');
    return value !== null && value !== '-1';
  }
  if (selector === 'a[href]') {
    return node.tagName === 'A' && node.getAttribute('href') !== null;
  }

  const attrOnly = selector.match(/^\[([^=\]]+)(?:="([^"]*)")?\]$/);
  if (attrOnly) {
    const value = node.getAttribute(attrOnly[1]);
    if (attrOnly[2] === undefined) return value !== null;
    return value === attrOnly[2];
  }

  if (selector.startsWith('#')) {
    return node.id === selector.slice(1);
  }

  if (selector.startsWith('.')) {
    const present = new Set(String(node.className || '').split(/\s+/).filter(Boolean));
    return selector.slice(1).split('.').every((name) => present.has(name));
  }

  const tagClass = selector.match(/^([a-zA-Z0-9-]+)\.([a-zA-Z0-9_.-]+)$/);
  if (tagClass) {
    return (
      node.tagName.toLowerCase() === tagClass[1].toLowerCase()
      && matchesSimple(node, `.${tagClass[2]}`)
    );
  }

  return node.tagName.toLowerCase() === selector.toLowerCase();
}

function matchesSelector(node, selector) {
  return splitSelector(selector).some((candidate) => {
    const parts = splitDescendantSelector(candidate);
    if (!parts.length) return false;
    let current = node;
    if (!matchesSimple(current, parts[parts.length - 1])) return false;
    for (let index = parts.length - 2; index >= 0; index -= 1) {
      let parent = current.parentNode;
      while (parent && !matchesSimple(parent, parts[index])) {
        parent = parent.parentNode;
      }
      if (!parent) return false;
      current = parent;
    }
    return true;
  });
}

class FakeElement {
  constructor(tagName, ownerDocument) {
    this.nodeType = 1;
    this.tagName = String(tagName || '').toUpperCase();
    this.ownerDocument = ownerDocument;
    this.parentNode = null;
    this.children = [];
    this._attrMap = {};
    this.attributes = [];
    this.dataset = {};
    this.listeners = {};
    this.className = '';
    this.classList = new FakeClassList(this);
    this.hidden = false;
    this.disabled = false;
    this.value = '';
    this.defaultValue = '';
    this.checked = false;
    this.defaultChecked = false;
    this.type = '';
    this.id = '';
    this.href = '';
    this.src = '';
    this.style = {};
    this.scrollTop = 0;
    this._text = '';
    this._innerHTML = '';
  }

  get firstChild() {
    return this.children[0] || null;
  }

  get nextSibling() {
    if (!this.parentNode) return null;
    const siblings = this.parentNode.children;
    const index = siblings.indexOf(this);
    return index >= 0 ? siblings[index + 1] || null : null;
  }

  get textContent() {
    return this._text + this.children.map((child) => child.textContent).join('');
  }

  set textContent(value) {
    this._text = String(value ?? '');
    this.children.forEach((child) => { child.parentNode = null; });
    this.children = [];
    this._innerHTML = this._text;
  }

  get text() {
    return this.textContent;
  }

  set text(value) {
    this.textContent = value;
  }

  get innerHTML() {
    if (this.children.length === 0) return this._innerHTML || this._text;
    return this._innerHTML || this.textContent;
  }

  set innerHTML(value) {
    this.replaceChildren();
    this._innerHTML = String(value ?? '');
    parseHtmlInto(this, this._innerHTML);
  }

  _refreshAttributesProxy() {
    this.attributes.length = 0;
    Object.entries(this._attrMap).forEach(([name, value]) => {
      this.attributes.push({ name, value });
    });
  }

  setAttribute(name, value) {
    const text = String(value ?? '');
    this._attrMap[name] = text;
    this._refreshAttributesProxy();
    if (name === 'class') this.className = text;
    if (name === 'id') this.id = text;
    if (name === 'href') this.href = text;
    if (name === 'src') this.src = text;
    if (name === 'type') this.type = text;
    if (name === 'value') {
      this.value = text;
      this.defaultValue = text;
    }
    if (name === 'hidden') this.hidden = true;
    if (name === 'disabled') this.disabled = true;
    if (name.startsWith('data-')) this.dataset[dataKey(name)] = text;
  }

  getAttribute(name) {
    if (name === 'class') return this.className || null;
    if (name === 'id') return this.id || null;
    if (Object.prototype.hasOwnProperty.call(this._attrMap, name)) {
      return this._attrMap[name];
    }
    return null;
  }

  removeAttribute(name) {
    delete this._attrMap[name];
    this._refreshAttributesProxy();
    if (name === 'class') this.className = '';
    if (name === 'id') this.id = '';
    if (name === 'href') this.href = '';
    if (name === 'src') this.src = '';
    if (name === 'hidden') this.hidden = false;
    if (name === 'disabled') this.disabled = false;
    if (name.startsWith('data-')) delete this.dataset[dataKey(name)];
  }

  appendChild(child) {
    if (!child) return child;
    if (child.parentNode) child.parentNode.removeChild(child);
    child.parentNode = this;
    this.children.push(child);
    return child;
  }

  insertBefore(child, reference) {
    if (!reference) return this.appendChild(child);
    if (child.parentNode) child.parentNode.removeChild(child);
    const index = this.children.indexOf(reference);
    child.parentNode = this;
    if (index < 0) this.children.push(child);
    else this.children.splice(index, 0, child);
    return child;
  }

  removeChild(child) {
    const index = this.children.indexOf(child);
    if (index >= 0) this.children.splice(index, 1);
    child.parentNode = null;
    return child;
  }

  replaceChildren(...nodes) {
    this.children.forEach((child) => { child.parentNode = null; });
    this.children = [];
    this._text = '';
    this._innerHTML = '';
    nodes.forEach((node) => this.appendChild(node));
  }

  replaceWith(node) {
    if (!this.parentNode) return;
    const parent = this.parentNode;
    const index = parent.children.indexOf(this);
    if (index >= 0) {
      parent.children[index] = node;
      node.parentNode = parent;
      this.parentNode = null;
    }
    if (node.tagName === 'SCRIPT') {
      this.ownerDocument.executeScriptElement(node);
    }
  }

  remove() {
    if (this.parentNode) this.parentNode.removeChild(this);
  }

  addEventListener(type, handler) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push(handler);
  }

  removeEventListener(type, handler) {
    if (!this.listeners[type]) return;
    this.listeners[type] = this.listeners[type].filter((item) => item !== handler);
  }

  dispatchEvent(event) {
    if (!event.target) event.target = this;
    (this.listeners[event.type] || []).forEach((handler) => handler(event));
    return !event.defaultPrevented;
  }

  focus() {
    this.ownerDocument.activeElement = this;
  }

  scrollIntoView() {}

  matches(selector) {
    return matchesSelector(this, selector);
  }

  closest(selector) {
    let node = this;
    while (node) {
      if (node.matches && node.matches(selector)) return node;
      node = node.parentNode;
    }
    return null;
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }

  querySelectorAll(selector) {
    const found = [];
    const visit = (node) => {
      node.children.forEach((child) => {
        if (matchesSelector(child, selector)) found.push(child);
        visit(child);
      });
    };
    visit(this);
    return found;
  }

  insertAdjacentHTML(position, html) {
    if (position === 'afterend' && this.parentNode) {
      const holder = this.ownerDocument.createElement('div');
      parseHtmlInto(holder, html);
      let reference = this.nextSibling;
      holder.children.slice().forEach((child) => {
        this.parentNode.insertBefore(child, reference);
        reference = child.nextSibling;
      });
      return;
    }
    parseHtmlInto(this, html, { append: true });
  }
}

class FakeDocument {
  constructor() {
    this.nodeType = 9;
    this.listeners = {};
    this.readyState = 'complete';
    this.title = 'journal';
    this.activeElement = null;
    this.scriptExecutions = [];
    this.inlineScriptErrors = [];
    this.externalScriptErrors = [];
    this.workspaceMountedEvents = [];
    this.documentElement = new FakeElement('html', this);
    this.head = new FakeElement('head', this);
    this.body = new FakeElement('body', this);
    this.documentElement.appendChild(this.head);
    this.documentElement.appendChild(this.body);
    this.context = null;
  }

  createElement(tagName) {
    return new FakeElement(tagName, this);
  }

  getElementById(id) {
    return this.documentElement.querySelector(`#${id}`);
  }

  querySelector(selector) {
    return this.documentElement.querySelector(selector);
  }

  querySelectorAll(selector) {
    return this.documentElement.querySelectorAll(selector);
  }

  addEventListener(type, handler) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push(handler);
  }

  removeEventListener(type, handler) {
    if (!this.listeners[type]) return;
    this.listeners[type] = this.listeners[type].filter((item) => item !== handler);
  }

  dispatchEvent(event) {
    if (event.type === 'workspace:mounted') this.workspaceMountedEvents.push(event.detail);
    if (!event.target) event.target = this;
    (this.listeners[event.type] || []).forEach((handler) => handler(event));
    return !event.defaultPrevented;
  }

  contains(node) {
    return Boolean(node && (node === this || node === this.documentElement || node.closest?.('html')));
  }

  executeScriptElement(node) {
    const descriptor = node.src || `inline:${this.scriptExecutions.length + 1}`;
    this.scriptExecutions.push(descriptor);
    if (node.src) {
      const source = node.src === '/app/speakers/static/who_is_this.js' ? whoIsThisSource : null;
      if (source === null) {
        const error = new Error(`missing script stub: ${node.src}`);
        this.externalScriptErrors.push({ src: node.src, message: error.message });
        node.dispatchEvent(new FakeEvent('error'));
        return;
      }
      try {
        vm.runInContext(source, this.context, { filename: node.src });
      } catch (error) {
        this.externalScriptErrors.push({ src: node.src, message: error.message });
      }
      node.dispatchEvent(new FakeEvent('load'));
      return;
    }
    try {
      vm.runInContext(node.textContent || '', this.context, { filename: descriptor });
    } catch (error) {
      this.inlineScriptErrors.push({
        script: descriptor,
        message: error && error.message ? error.message : String(error),
      });
      this.context.window.dispatchEvent(new FakeEvent('error', { error }));
    }
  }
}

function parseHtmlInto(parent, html, options = {}) {
  if (!options.append) parent.replaceChildren();
  const stack = [parent];
  const tagRe = /<!--[\s\S]*?-->|<script\b([^>]*)>([\s\S]*?)<\/script>|<\/?([a-zA-Z0-9-]+)\b([^>]*)>/gi;
  let match;
  while ((match = tagRe.exec(String(html || ''))) !== null) {
    if (match[0].startsWith('<!--')) continue;
    if (match[1] !== undefined) {
      const script = parent.ownerDocument.createElement('script');
      parseAttributes(match[1]).forEach(([name, value]) => script.setAttribute(name, value));
      script.textContent = match[2] || '';
      stack[stack.length - 1].appendChild(script);
      continue;
    }
    const raw = match[0];
    const tag = String(match[3] || '').toLowerCase();
    if (!tag) continue;
    if (raw.startsWith('</')) {
      while (stack.length > 1) {
        const node = stack.pop();
        if (node.tagName.toLowerCase() === tag) break;
      }
      continue;
    }
    const node = parent.ownerDocument.createElement(tag);
    parseAttributes(match[4]).forEach(([name, value]) => node.setAttribute(name, value));
    stack[stack.length - 1].appendChild(node);
    if (!raw.endsWith('/>') && !isVoidTag(tag)) stack.push(node);
  }
}

function responseJson(data, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status >= 200 && status < 300 ? 'OK' : 'ERROR',
    headers: { get() { return ''; } },
    async text() { return JSON.stringify(data); },
    async json() { return data; },
  };
}

function responseText(text, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status >= 200 && status < 300 ? 'OK' : 'ERROR',
    headers: { get() { return ''; } },
    async text() { return text; },
    async json() { return JSON.parse(text); },
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

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function speakerCopy() {
  const keys = [
    'SPK_SHEET_TITLE',
    'SPK_SHEET_LEDE_MANY',
    'SPK_SHEET_LEDE_ONE',
    'SPK_SHELF_CANDIDATES',
    'SPK_SHELF_NO_EVIDENCE',
    'SPK_EVIDENCE_SCREEN_MANY',
    'SPK_EVIDENCE_SCREEN_ONE',
    'SPK_EVIDENCE_MEETING_MANY',
    'SPK_EVIDENCE_MEETING_ONE',
    'SPK_SHELF_MENTIONS',
    'SPK_ANCHOR',
    'SPK_ANCHOR_HAS_VOICE',
    'SPK_SEARCH_LABEL',
    'SPK_SEARCH_PLACEHOLDER',
    'SPK_THIS_IS_ME',
    'SPK_THIS_IS_ME_GUIDANCE',
    'SPK_SEARCH_NO_RESULTS',
    'SPK_CREATE_ROW',
    'SPK_NEAR_MATCH_BAND',
    'SPK_KEEP_SEPARATE_TITLE',
    'SPK_KEEP_SEPARATE_BODY',
    'SPK_KEEP_SEPARATE_CONFIRM',
    'SPK_KEEP_SEPARATE_DECLINE',
    'SPK_PREVIEW_TITLE',
    'SPK_PREVIEW_BODY_FRESH',
    'SPK_PREVIEW_BODY_HAS_VOICE',
    'SPK_PREVIEW_FACTS',
    'SPK_PREVIEW_CONFIRM',
    'SPK_PREVIEW_BACK',
    'SPK_RECEIPT_TITLE',
    'SPK_RECEIPT_BODY',
    'SPK_RECEIPT_UNDO',
    'SPK_UNDO_DONE',
    'SPK_UNDO_PARTIAL',
    'SPK_EXIT_NOT_PERSON',
    'SPK_EXIT_NOT_NOW',
    'SPK_NOT_PERSON_DONE',
    'SPK_NOT_NOW_DONE',
    'SPK_ACTION_WHO_IS_THIS',
    'SPK_LOAD_ERROR',
    'SPK_SEARCH_ERROR',
    'SPK_CHECK_NAME_ERROR',
    'SPK_SAMPLE_UNAVAILABLE',
    'SPK_ACTION_RETRY',
    'SPK_DISCOVERY_ERROR',
    'SPK_DISCOVERY_DEGRADED_TEMPLATE',
    'SPK_OVERVIEW_CARD_SEGMENTS_LABEL',
    'SPK_OVERVIEW_CARD_SAMPLES_LABEL',
    'SPK_OVERVIEW_CARD_LAST_HEARD_PREFIX',
    'SPK_OVERVIEW_CARD_STREAMS_PREFIX',
    'SPK_OVERVIEW_KNOWN_VOICES_EMPTY',
    'SPK_OVERVIEW_KNOWN_VOICES_HEADER',
    'SPK_OVERVIEW_NEW_VOICES_HEADER',
    'SPK_OVERVIEW_YOUR_VOICE_HEADER',
    'SPK_OVERVIEW_QUALITY_HEADER',
    'SPK_OVERVIEW_OWNER_PROGRESS_UNKNOWN',
    'SPK_OVERVIEW_OWNER_PROGRESS_SUFFIX',
    'SPK_OVERVIEW_OWNER_STREAMS_LABEL',
    'SPK_OVERVIEW_YOUR_VOICE_LEARNING',
    'SPK_OVERVIEW_OWNER_HELP_LABEL',
    'SPK_OVERVIEW_OWNER_BUILD_FROM_TAGS_LABEL',
    'SPK_OVERVIEW_OWNER_STATUS_ERROR',
    'SPK_OVERVIEW_OWNER_SAMPLES_LABEL',
    'SPK_OVERVIEW_YOUR_VOICE_CONFIRMED',
    'SPK_OVERVIEW_OWNER_BUILT_UNKNOWN',
    'SPK_OVERVIEW_OWNER_COHESION_LABEL',
    'SPK_OVERVIEW_OWNER_BUILT_PREFIX',
    'SPK_OVERVIEW_OWNER_REFRESHED_PREFIX',
    'SPK_OVERVIEW_QUALITY_PREBOOTSTRAP',
    'SPK_OVERVIEW_QUALITY_READY',
    'SPK_OVERVIEW_QUALITY_ERROR_HEADING',
    'SPK_OVERVIEW_QUALITY_HIGH_LABEL',
    'SPK_OVERVIEW_QUALITY_MEDIUM_LABEL',
    'SPK_OVERVIEW_QUALITY_MARGIN_LABEL',
    'SPK_OVERVIEW_QUALITY_UNLABELED_LABEL',
    'SPK_OVERVIEW_QUALITY_MISSING_LABEL',
    'SPK_OVERVIEW_QUALITY_SKIPPED_LABEL',
    'SPK_OVERVIEW_QUALITY_TEACHING_ZERO',
    'SPK_OVERVIEW_QUALITY_TEACHING_LABEL',
    'SPK_OVERVIEW_QUALITY_UNREADABLE_WARNING',
    'SPK_GRID_TITLE',
    'SPK_GRID_BODY',
    'SPK_GRID_UNIT_ONE',
    'SPK_GRID_UNIT_OTHER',
    'SPK_GRID_UNIT_NONE',
    'SPK_GRID_ACTIVITY_ONE',
    'SPK_GRID_ACTIVITY_OTHER',
    'SPK_OVERVIEW_TODAY_LINK_LABEL',
  ];
  const copy = Object.fromEntries(keys.map((key) => [key, `copy_${key}`]));
  copy.SPK_OVERVIEW_KNOWN_VOICES_SORTS = ['copy_sort_recent', 'copy_sort_name'];
  copy.SPK_OVERVIEW_COHESION_LABELS = [
    '',
    'copy_cohesion_1',
    'copy_cohesion_2',
    'copy_cohesion_3',
    'copy_cohesion_4',
    'copy_cohesion_5',
  ];
  return copy;
}

function statePayload() {
  return {
    speaker_copy: speakerCopy(),
    owner_status_routing_tokens: {
      candidate: 'candidate',
      confirmed: 'confirmed',
    },
    not_in_new_voices_copy: 'copy_statement_handoff_notice',
    today: '20260721',
    owner_min_statements: 3,
    speaker_filter_name: '',
  };
}

function clusterPayload(clusterId, name) {
  return {
    cluster_id: clusterId,
    size: 4,
    segment_count: 4,
    suggested_name: name || `Cluster ${clusterId}`,
    samples: [],
  };
}

function malformedClusterPayload() {
  return {
    size: 4,
    segment_count: 4,
    suggested_name: 'Malformed cluster',
    samples: [],
  };
}

function presencePayload(clusterId) {
  return {
    cluster_id: Number(clusterId),
    evidence_complete: true,
    facts: {
      statement_count: 4,
      conversation_count: 2,
      samples: [],
    },
    candidates: {
      co_presence: [],
      mention: [],
    },
  };
}

function locationFromUrl(rawUrl) {
  const parsed = new URL(rawUrl, 'http://solstone.local');
  return {
    href: parsed.pathname + parsed.search + parsed.hash,
    pathname: parsed.pathname,
    search: parsed.search,
    hash: parsed.hash,
    assign(value) {
      this.href = value;
    },
    reload() {},
  };
}

function makeDocument() {
  const document = new FakeDocument();
  document.body.innerHTML = `
    <nav class="menu-bar"><ul class="menu-items"></ul></nav>
    <aside id="diagnostic-console">
      <h3 id="diagnostic-console-title"></h3>
      <div class="diagnostic-console-tabs"></div>
      <button data-diagnostic-action="clear"></button>
      <button data-diagnostic-action="send-all"></button>
      <button data-diagnostic-action="close"></button>
      <p data-diagnostic-reporting-off></p>
      <button data-diagnostic-filter="all"><span data-diagnostic-count="all"></span></button>
      <button data-diagnostic-filter="error"><span data-diagnostic-count="error"></span></button>
      <button data-diagnostic-filter="warning"><span data-diagnostic-count="warning"></span></button>
      <button data-diagnostic-filter="info"><span data-diagnostic-count="info"></span></button>
      <p id="diagnostic-console-empty"></p>
    </aside>
    <a id="status-pane-console-link"></a>
    <main id="main-content" class="workspace"></main>
  `;
  return document;
}

async function flush(times = 1) {
  for (let index = 0; index < times; index += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

function classify(result) {
  if (result.mountErrorContexts.length || result.shellErrorVisible) {
    return 'a: mountWorkspaceFragment rejected and shell rendered an error';
  }
  if (!result.workspaceMountedEvents && result.mainContentEmpty) {
    return 'pending: background load delayed workspace mount';
  }
  if (result.routeKind === 'overview' && result.overviewHidden) {
    return 'b: startOverview early-returned and overview stayed hidden';
  }
  if (result.routeKind === 'day' && result.dayHidden) {
    return 'b: startSpeakersDay early-returned and day view stayed hidden';
  }
  if (!result.discoveryScanOk && !result.sheetOpen) {
    return 'c: loadDiscovery resolved false before handoff';
  }
  if (result.expectedClusterId && !result.renderedClusterIds.includes(result.expectedClusterId) && !result.sheetOpen) {
    return 'd: openOverviewDiscoveryCluster returned false because discoveryClustersById lacks the id';
  }
  if (result.sheetOpen) return 'none: mounted and opened sheet';
  return 'unclassified: mounted but no sheet';
}

async function runRoute(scenario) {
  const { label, url: rawUrl } = scenario;
  const startedAtMs = Date.now();
  const document = makeDocument();
  const fetchCalls = [];
  const apiCalls = [];
  const logs = [];
  const windowListeners = {};
  const location = locationFromUrl(rawUrl);
  let shellReady = null;
  let scanCalled = false;
  const shell = {
    apps: [
      ...(includeBackground
        ? [{
            name: 'support',
            label: 'Support',
            icon: 'S',
            starred: false,
            app_bar: false,
            facets_enabled: false,
            workspace_url: '/app/support/workspace',
            background_url: '/app/support/background',
          }]
        : []),
      {
        name: 'speakers',
        label: 'Speakers',
        icon: 'S',
        starred: true,
        app_bar: false,
        facets_enabled: false,
        workspace_url: '/app/speakers/workspace',
      },
    ],
    facets: [],
    selected_facet: null,
    settings: { reporting_enabled: true },
  };
  const fetchStub = async (url, options = {}) => {
    fetchCalls.push({ url: String(url), method: String(options.method || 'GET').toUpperCase() });
    if (url === '/app/support/background') {
      if (backgroundDelayMs > 0) await delay(backgroundDelayMs);
      return responseText('window.__BACKGROUND_LOADED__ = true;');
    }
    if (url === '/app/speakers/workspace') return responseText(workspaceHtml);
    if (url === '/app/speakers/api/discovery/cache') {
      return responseJson(scenario.cachePayload || { status: 'ok', clusters: [] });
    }
    if (url === '/app/speakers/api/discovery/scan') {
      scanCalled = true;
      const payload = scenario.scanPayload || { clusters: [clusterPayload(3)] };
      if (payload === null) return responseJson({ error: 'scan failed' }, 500);
      return responseJson(payload);
    }
    if (url === '/app/speakers/api/grid') return responseJson({ coverage: {}, activity: {} });
    if (url === '/app/speakers/api/owner/status') {
      return responseJson({
        status: 'not_ready',
        manual_tags_count: 0,
        streams_represented: 0,
      });
    }
    if (url === '/app/speakers/api/quality') {
      return responseJson({
        owner_voice: { bootstrap_state: 'ready' },
        tier_histogram: {},
        corrections_window_count: 0,
        unreadable_files: {},
      });
    }
    if (String(url).startsWith('/app/speakers/api/speakers/known')) {
      return responseJson({ speakers: [] });
    }
    return responseJson({ error: `unexpected fetch ${url}` }, 404);
  };
  const apiJson = async (url, options = {}) => {
    apiCalls.push({
      url: String(url),
      method: String(options.method || 'GET').toUpperCase(),
      body: options.body ? JSON.parse(options.body) : null,
    });
    if (url === '/api/shell') return shell;
    if (String(url).startsWith('/app/speakers/api/state')) return statePayload();
    if (url === '/app/speakers/api/discovery/cache') {
      return scenario.cachePayload || { status: 'ok', clusters: [] };
    }
    if (url === '/app/speakers/api/discovery/scan') {
      scanCalled = true;
      const payload = scenario.scanPayload || { clusters: [clusterPayload(3)] };
      if (payload === null) throw apiError({ error: 'scan failed' }, 500);
      return payload;
    }
    if (String(url).startsWith('/app/speakers/api/discovery/resolve-statement')) {
      if (scenario.resolveStatement === 'dynamic-cache-hit-before-scan') {
        return scanCalled
          ? { status: 'cache_unavailable', cluster_id: null }
          : { status: 'hit', cluster_id: 9 };
      }
      return scenario.resolveStatement || { status: 'hit', cluster_id: 9 };
    }
    const presence = String(url).match(/^\/app\/speakers\/api\/discovery\/cluster\/([^/]+)\/presence/);
    if (presence) return presencePayload(decodeURIComponent(presence[1]));
    throw new Error(`unexpected apiJson ${url}`);
  };
  const window = {
    document,
    location,
    console,
    URLSearchParams,
    CustomEvent: FakeEvent,
    Event: FakeEvent,
    setTimeout,
    clearTimeout,
    setImmediate,
    fetch: fetchStub,
    apiJson,
    addEventListener(type, handler) {
      if (!windowListeners[type]) windowListeners[type] = [];
      windowListeners[type].push(handler);
    },
    removeEventListener(type, handler) {
      if (!windowListeners[type]) return;
      windowListeners[type] = windowListeners[type].filter((item) => item !== handler);
    },
    dispatchEvent(event) {
      (windowListeners[event.type] || []).forEach((handler) => handler(event));
      return true;
    },
    AppServices: {
      escapeHtml(value) {
        return String(value ?? '')
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;')
          .replace(/"/g, '&quot;')
          .replace(/'/g, '&#39;');
      },
      markBackgroundFailing(app, error) {
        logs.push({ context: 'background', app, message: error.message });
      },
    },
    SurfaceState: {
      loading({ text }) {
        return `<div class="surface-state surface-state--loading">${text || ''}</div>`;
      },
      error() {
        return '<div class="surface-state surface-state--error"><button class="surface-state-retry">retry</button></div>';
      },
    },
    CONVEY_COPY: { RELOAD_HINT: 'reload to try again' },
    CONVEY_SETTINGS: {},
    RelativeTime: { formatTimestamp() { return 'just now'; } },
    formatDateShort(day) { return `day-${day}`; },
    logError(error, meta) {
      logs.push({
        context: meta && meta.context ? meta.context : '',
        message: error && error.message ? error.message : String(error),
      });
    },
    resolveSolShellReady(data) {
      shellReady = data;
    },
  };
  window.window = window;
  const context = {
    console,
    setTimeout,
    clearTimeout,
    setImmediate,
    URL,
    URLSearchParams,
    document,
    window,
    fetch: fetchStub,
    CustomEvent: FakeEvent,
    Event: FakeEvent,
  };
  document.context = vm.createContext(context);

  vm.runInContext(mountWorkspaceSource, document.context, { filename: 'mount-workspace.js' });
  vm.runInContext(shellBootSource, document.context, { filename: 'shell_boot.js' });
  await flush(10);

  const contextSnapshot = document.context.window.SPEAKERS_CONTEXT || null;
  const pathContextSnapshot = document.context.window.solPathContext
    ? document.context.window.solPathContext()
    : null;
  const overview = document.getElementById('speakersOverviewView');
  const day = document.getElementById('speakersDayView');
  const backdrop = document.querySelector('.spk-who-backdrop');
  const renderedClusterIds = document
    .querySelectorAll('.spk-discovery-card')
    .map((node) => String(node.dataset.clusterId || ''));
  const transportCalls = [...fetchCalls, ...apiCalls];
  const discoveryCall = transportCalls.find((call) => call.url === '/app/speakers/api/discovery/scan');
  const discoveryCacheCall = transportCalls.find((call) => call.url === '/app/speakers/api/discovery/cache');
  const presenceClusterIds = apiCalls
    .map((call) => String(call.url || '').match(/^\/app\/speakers\/api\/discovery\/cluster\/([^/]+)\/presence/))
    .filter(Boolean)
    .map((match) => decodeURIComponent(match[1]));
  const expectedClusterId = contextSnapshot && contextSnapshot.voiceClusterId
    ? String(contextSnapshot.voiceClusterId)
    : '9';
  const result = {
    label,
    url: rawUrl,
    backgroundDelayMs,
    backgroundLoaded: Boolean(document.context.window.__BACKGROUND_LOADED__),
    mainContentEmpty: !(document.getElementById('main-content')?.children || []).length,
    elapsedMsUntilInspection: Date.now() - startedAtMs,
    pathContext: pathContextSnapshot,
    speakersContext: contextSnapshot,
    routeKind: contextSnapshot && contextSnapshot.isDay ? 'day' : 'overview',
    shellReady: Boolean(shellReady),
    workspaceMountedEvents: document.workspaceMountedEvents.length,
    scriptExecutions: document.scriptExecutions,
    inlineScriptErrors: document.inlineScriptErrors,
    externalScriptErrors: document.externalScriptErrors,
    statePromiseAssigned: Boolean(document.context.window.SPEAKERS_STATE_PROMISE && typeof document.context.window.SPEAKERS_STATE_PROMISE.then === 'function'),
    overviewHidden: overview ? Boolean(overview.hidden) : null,
    dayHidden: day ? Boolean(day.hidden) : null,
    discoveryScanOk: Boolean(discoveryCall),
    discoveryCacheOk: Boolean(discoveryCacheCall),
    renderedClusterIds,
    presenceClusterIds,
    expectedClusterId,
    sheetOpen: Boolean(backdrop && backdrop.hidden === false),
    handoffNoticeHidden: document.getElementById('spkStatementHandoffNotice')
      ? Boolean(document.getElementById('spkStatementHandoffNotice').hidden)
      : null,
    shellErrorVisible: Boolean(document.getElementById('main-content')?.querySelector('.surface-state--error')),
    mountErrorContexts: logs.filter((entry) => ['workspace-fragment-mount', 'shell-boot'].includes(entry.context)),
    fetchCalls,
    apiCalls,
    logs,
  };
  result.classification = classify(result);
  return result;
}

(async () => {
  const cases = [
    {
      label: 'cluster-id handoff opens cached cluster before empty scan',
      url: '/app/speakers/?voice_cluster_id=9',
      cachePayload: { status: 'ok', clusters: [clusterPayload(9)] },
      scanPayload: { clusters: [] },
      expectSheetOpen: true,
      expectPresenceClusterId: '9',
    },
    {
      label: 'statement handoff resolves cached cluster before renumbering scan',
      url: '/app/speakers/?voice_day=20240101&voice_stream=test&voice_segment_key=090000_300&voice_source=audio&voice_sentence_id=12',
      cachePayload: { status: 'ok', clusters: [clusterPayload(9)] },
      scanPayload: { clusters: [clusterPayload(3)] },
      resolveStatement: 'dynamic-cache-hit-before-scan',
      expectSheetOpen: true,
      expectPresenceClusterId: '9',
    },
    {
      label: 'statement miss shows existing notice',
      url: '/app/speakers/?voice_day=20240101&voice_stream=test&voice_segment_key=090000_300&voice_source=audio&voice_sentence_id=12',
      cachePayload: { status: 'ok', clusters: [clusterPayload(9)] },
      resolveStatement: { status: 'miss', cluster_id: null },
      expectNotice: true,
    },
    {
      label: 'statement cache unavailable shows existing notice',
      url: '/app/speakers/?voice_day=20240101&voice_stream=test&voice_segment_key=090000_300&voice_source=audio&voice_sentence_id=12',
      cachePayload: { status: 'cache_unavailable', clusters: [] },
      resolveStatement: { status: 'cache_unavailable', cluster_id: null },
      expectNotice: true,
    },
    {
      label: 'absent cluster id never opens a different rendered cluster',
      url: '/app/speakers/?voice_cluster_id=9',
      cachePayload: { status: 'ok', clusters: [clusterPayload(3)] },
      scanPayload: { clusters: [clusterPayload(3)] },
      expectNotice: true,
      forbidPresenceClusterId: '3',
    },
    {
      label: 'malformed cluster row never opens',
      url: '/app/speakers/?voice_cluster_id=9',
      cachePayload: { status: 'ok', clusters: [malformedClusterPayload()] },
      scanPayload: { clusters: [clusterPayload(3)] },
      expectNotice: true,
      forbidPresenceClusterId: '3',
    },
    {
      label: 'normal navigation uses scan as refresh after cached render',
      url: '/app/speakers/',
      cachePayload: { status: 'ok', clusters: [clusterPayload(9)] },
      scanPayload: { clusters: [clusterPayload(3)] },
      expectScan: true,
      expectRenderedClusterIds: ['3'],
    },
  ];
  const results = [];
  const failures = [];
  for (const scenario of cases) {
    const result = await runRoute(scenario);
    results.push(result);
    const fail = (message) => failures.push(`${scenario.label}: ${message}`);
    if (!result.shellReady) fail('shell did not become ready');
    if (result.shellErrorVisible) fail('shell error rendered');
    if (result.mainContentEmpty) fail('workspace stayed empty');
    if (result.workspaceMountedEvents !== 1) {
      fail(`expected one workspace mount, got ${result.workspaceMountedEvents}`);
    }
    if (!result.discoveryCacheOk) {
      fail('discovery cache was not read');
    }
    if (scenario.expectScan && !result.discoveryScanOk) {
      fail('scan refresh did not run');
    }
    if (scenario.expectSheetOpen && !result.sheetOpen) {
      fail('expected sheet to open');
    }
    if (!scenario.expectSheetOpen && result.sheetOpen) {
      fail('sheet opened unexpectedly');
    }
    if (
      scenario.expectPresenceClusterId
      && !result.presenceClusterIds.includes(scenario.expectPresenceClusterId)
    ) {
      fail(`did not load presence for cluster ${scenario.expectPresenceClusterId}`);
    }
    if (
      scenario.forbidPresenceClusterId
      && result.presenceClusterIds.includes(scenario.forbidPresenceClusterId)
    ) {
      fail(`opened forbidden cluster ${scenario.forbidPresenceClusterId}`);
    }
    const noticeVisible = result.handoffNoticeHidden === false;
    if (scenario.expectNotice && !noticeVisible) {
      fail('expected statement handoff notice');
    }
    if (!scenario.expectNotice && noticeVisible) {
      fail('unexpected statement handoff notice');
    }
    if (scenario.expectRenderedClusterIds) {
      const actual = result.renderedClusterIds.join(',');
      const expected = scenario.expectRenderedClusterIds.join(',');
      if (actual !== expected) {
        fail(`expected rendered clusters ${expected}, got ${actual}`);
      }
    }
  }
  console.log(JSON.stringify(results, null, 2));
  if (failures.length) {
    throw new Error(failures.join('\n'));
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
