// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const manifestDir = process.argv[2];
if (!manifestDir) throw new Error('manifest directory required');

function decodeHtml(str) {
  return String(str ?? '')
    .replace(/&#39;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&amp;/g, '&');
}

class DOMNode {
  constructor(ownerDoc) {
    this.ownerDocument = ownerDoc;
    this.parentNode = null;
    this.children = [];
    this._listeners = {};
  }

  appendChild(child) {
    if (child.parentNode) child.parentNode.removeChild(child);
    child.parentNode = this;
    this.children.push(child);
    return child;
  }

  removeChild(child) {
    const idx = this.children.indexOf(child);
    if (idx !== -1) {
      this.children.splice(idx, 1);
      child.parentNode = null;
    }
    return child;
  }

  insertBefore(newChild, refChild) {
    if (newChild.parentNode) newChild.parentNode.removeChild(newChild);
    const idx = this.children.indexOf(refChild);
    if (idx === -1) {
      return this.appendChild(newChild);
    }
    newChild.parentNode = this;
    this.children.splice(idx, 0, newChild);
    return newChild;
  }

  addEventListener(type, fn) {
    if (!this._listeners[type]) this._listeners[type] = [];
    this._listeners[type].push(fn);
  }

  removeEventListener(type, fn) {
    if (!this._listeners[type]) return;
    this._listeners[type] = this._listeners[type].filter(l => l !== fn);
  }

  dispatchEvent(event) {
    event.target = this;
    let curr = this;
    while (curr) {
      event.currentTarget = curr;
      const list = curr._listeners?.[event.type] || [];
      for (const fn of list) {
        fn.call(curr, event);
        if (event._stopPropagation) break;
      }
      if (event._stopPropagation) break;
      curr = curr.parentNode;
    }
  }
}

class DOMElement extends DOMNode {
  constructor(tagName, ownerDoc) {
    super(ownerDoc);
    this.tagName = tagName.toUpperCase();
    this.attributes = Object.create(null);
    const self = this;
    this.dataset = new Proxy(this.attributes, {
      get(target, prop) {
        const attr = 'data-' + String(prop).replace(/([A-Z])/g, '-$1').toLowerCase();
        return target[attr];
      },
      set(target, prop, value) {
        const attr = 'data-' + String(prop).replace(/([A-Z])/g, '-$1').toLowerCase();
        target[attr] = String(value);
        return true;
      }
    });
    this.classList = {
      contains(cls) {
        const list = (self.attributes['class'] || '').split(/\s+/).filter(Boolean);
        return list.includes(cls);
      },
      add(...classes) {
        const set = new Set((self.attributes['class'] || '').split(/\s+/).filter(Boolean));
        classes.forEach(c => set.add(c));
        self.attributes['class'] = Array.from(set).join(' ');
      },
      remove(...classes) {
        const set = new Set((self.attributes['class'] || '').split(/\s+/).filter(Boolean));
        classes.forEach(c => set.delete(c));
        self.attributes['class'] = Array.from(set).join(' ');
      }
    };
  }

  get id() { return this.attributes['id'] || ''; }
  set id(val) { this.attributes['id'] = String(val); }

  get className() { return this.attributes['class'] || ''; }
  set className(val) { this.attributes['class'] = String(val); }

  get disabled() {
    return this.hasAttribute('disabled') || this._disabled === true;
  }
  set disabled(val) {
    this._disabled = Boolean(val);
    if (val) {
      this.setAttribute('disabled', '');
    } else {
      this.removeAttribute('disabled');
    }
  }

  getAttribute(name) {
    return this.attributes[name] ?? null;
  }

  setAttribute(name, val) {
    this.attributes[name] = String(val);
  }

  hasAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name);
  }

  removeAttribute(name) {
    delete this.attributes[name];
  }

  remove() {
    if (this.parentNode) {
      this.parentNode.removeChild(this);
    }
  }

  // Focus and containment, enough for the day switch's focus rules.
  focus() { this.ownerDocument.activeElement = this; }
  contains(node) {
    for (let curr = node; curr; curr = curr.parentNode) if (curr === this) return true;
    return false;
  }

  closest(selector) {
    let curr = this;
    while (curr && curr instanceof DOMElement) {
      if (matches(curr, selector)) return curr;
      curr = curr.parentNode;
    }
    return null;
  }

  click() {
    const event = {
      type: 'click',
      target: this,
      defaultPrevented: false,
      preventDefault() { this.defaultPrevented = true; },
      stopPropagation() { this._stopPropagation = true; },
    };
    this.dispatchEvent(event);
  }

  get innerHTML() {
    return this.children.map(c => c.outerHTML !== undefined ? c.outerHTML : (c.nodeValue || '')).join('');
  }

  set innerHTML(html) {
    this.children.slice().forEach(c => this.removeChild(c));
    parseHtmlFragment(html, this);
  }

  get outerHTML() {
    const tag = this.tagName.toLowerCase();
    const attrs = Object.keys(this.attributes)
      .map(k => ` ${k}="${this.attributes[k].replace(/"/g, '&quot;')}"`)
      .join('');
    if (['br', 'img', 'input', 'hr', 'progress'].includes(tag)) {
      return `<${tag}${attrs}>${this.innerHTML}</${tag}>`;
    }
    return `<${tag}${attrs}>${this.innerHTML}</${tag}>`;
  }

  set outerHTML(html) {
    if (!this.parentNode) return;
    const parent = this.parentNode;
    const temp = new DOMElement('div', this.ownerDocument);
    parseHtmlFragment(html, temp);
    const newChildren = temp.children.slice();
    newChildren.forEach(c => parent.insertBefore(c, this));
    parent.removeChild(this);
  }

  insertAdjacentHTML(position, html) {
    const temp = new DOMElement('div', this.ownerDocument);
    parseHtmlFragment(html, temp);
    const newChildren = temp.children.slice();
    if (position === 'afterend') {
      if (!this.parentNode) return;
      const nextSibling = this.parentNode.children[this.parentNode.children.indexOf(this) + 1] || null;
      if (nextSibling) {
        newChildren.forEach(c => this.parentNode.insertBefore(c, nextSibling));
      } else {
        newChildren.forEach(c => this.parentNode.appendChild(c));
      }
    } else if (position === 'beforeend') {
      newChildren.forEach(c => this.appendChild(c));
    } else if (position === 'beforebegin') {
      if (!this.parentNode) return;
      newChildren.forEach(c => this.parentNode.insertBefore(c, this));
    } else if (position === 'afterbegin') {
      const firstChild = this.children[0] || null;
      if (firstChild) {
        newChildren.forEach(c => this.insertBefore(c, firstChild));
      } else {
        newChildren.forEach(c => this.appendChild(c));
      }
    }
  }

  querySelector(selector) {
    return findFirst(this, selector);
  }

  querySelectorAll(selector) {
    const list = [];
    findAll(this, selector, list);
    return list;
  }
}

class DOMTextNode extends DOMNode {
  constructor(text, ownerDoc) {
    super(ownerDoc);
    this.nodeValue = text;
  }
  get innerHTML() { return this.nodeValue; }
}

function parseHtmlFragment(html, rootElement) {
  const doc = rootElement.ownerDocument;
  const tagRegex = /<\/?([a-zA-Z0-9_-]+)((?:\s+[^>]*?)?)\/?>|([^<]+)/g;
  let match;
  let current = rootElement;
  const voidTags = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr']);

  while ((match = tagRegex.exec(html)) !== null) {
    const [fullToken, tagName, rawAttrs, text] = match;
    if (text !== undefined) {
      if (text.length > 0) {
        current.appendChild(new DOMTextNode(decodeHtml(text), doc));
      }
      continue;
    }
    if (fullToken.startsWith('</')) {
      if (current.parentNode && current !== rootElement) {
        current = current.parentNode;
      }
      continue;
    }
    const isSelfClosing = fullToken.endsWith('/>') || voidTags.has(tagName.toLowerCase());
    const el = new DOMElement(tagName, doc);
    if (rawAttrs) {
      const attrRegex = /([a-zA-Z0-9_-]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
      let attrMatch;
      while ((attrMatch = attrRegex.exec(rawAttrs)) !== null) {
        const name = attrMatch[1];
        const val = attrMatch[2] ?? attrMatch[3] ?? attrMatch[4] ?? '';
        el.setAttribute(name, decodeHtml(val));
      }
    }
    current.appendChild(el);
    if (!isSelfClosing) {
      current = el;
    }
  }
}

function matches(element, selector) {
  if (!(element instanceof DOMElement)) return false;
  selector = selector.trim();
  const idMatch = selector.match(/^#([a-zA-Z0-9_-]+)$/);
  if (idMatch) return element.id === idMatch[1];

  const classMatch = selector.match(/^\.([a-zA-Z0-9_-]+)$/);
  if (classMatch) return element.classList.contains(classMatch[1]);

  const tagMatch = selector.match(/^[a-zA-Z0-9_-]+$/);
  if (tagMatch) return element.tagName.toLowerCase() === selector.toLowerCase();

  // Support optional tag followed by one or more [attr="val"] or [attr] selectors
  const tagAndAttrsMatch = selector.match(/^([a-zA-Z0-9_-]+)?(\[[^\]]+\])+$/);
  if (tagAndAttrsMatch) {
    const tag = tagAndAttrsMatch[1];
    if (tag && element.tagName.toLowerCase() !== tag.toLowerCase()) return false;
    const attrRegex = /\[([a-zA-Z0-9_-]+)(?:="([^"]*)")?\]/g;
    let m;
    while ((m = attrRegex.exec(selector)) !== null) {
      const name = m[1];
      const expected = m[2];
      if (!element.hasAttribute(name)) return false;
      if (expected !== undefined && element.getAttribute(name) !== expected) return false;
    }
    return true;
  }

  return false;
}

function findFirst(root, selector) {
  for (const child of root.children) {
    if (child instanceof DOMElement) {
      if (matches(child, selector)) return child;
      const found = findFirst(child, selector);
      if (found) return found;
    }
  }
  return null;
}

function findAll(root, selector, acc) {
  for (const child of root.children) {
    if (child instanceof DOMElement) {
      if (matches(child, selector)) acc.push(child);
      findAll(child, selector, acc);
    }
  }
}

class DOMDocument extends DOMNode {
  constructor() {
    super(null);
    this.ownerDocument = this;
    this.readyState = 'complete';
  }
  createElement(tag) { return new DOMElement(tag, this); }
  createTextNode(text) { return new DOMTextNode(text, this); }
  querySelector(sel) { return findFirst(this, sel); }
  querySelectorAll(sel) { const l = []; findAll(this, sel, l); return l; }
  getElementById(id) { return findFirst(this, '#' + id); }
}

let source = fs.readFileSync(path.join(manifestDir, 'assets/home.js'), 'utf8');
source = source.replace(
  '  window.selectAccountSide = selectAccountSide;\n',
  `  window.__home_test = {
    renderPulse,
    renderVitals,
    renderNarrative,
    renderBriefing,
    selectAccountSide,
    handleDashboardKeydown,
    readVitals,
    getSides: () => ({ paintedSide, selectedSide }),
    readFirstMinutes,
    readAgents,
    handleSpeechRetry,
    updateGettingStartedSurface,
    initHome,
    setSurface: (s) => { surface = s; },
    getSurface: () => surface,
    setRoot: (r) => { root = r; wireInteractions(); },
    getState: () => ({
      firstMinutesPayload,
      firstMinutesClosed,
      setupPayload,
      agentsPayload,
      agentsSettled,
      downloadTimer,
      speechRetryInFlight,
    }),
    setState: (st) => {
      if ('firstMinutesPayload' in st) firstMinutesPayload = st.firstMinutesPayload;
      if ('firstMinutesClosed' in st) firstMinutesClosed = st.firstMinutesClosed;
      if ('setupPayload' in st) setupPayload = st.setupPayload;
      if ('agentsPayload' in st) agentsPayload = st.agentsPayload;
      if ('agentsSettled' in st) agentsSettled = st.agentsSettled;
      if ('downloadTimer' in st) downloadTimer = st.downloadTimer;
      if ('speechRetryInFlight' in st) speechRetryInFlight = st.speechRetryInFlight;
    },
  };
  window.selectAccountSide = selectAccountSide;\n`,
);

const doc = new DOMDocument();
const rootEl = doc.createElement('div');
rootEl.setAttribute('data-home-root', '');
doc.appendChild(rootEl);
const surface = doc.createElement('div');
surface.setAttribute('data-pulse-surface', '');
rootEl.appendChild(surface);

const apiRequests = [];
let apiResolver = null;
const activeTimers = new Map();
let nextTimerId = 1;
const timerLog = [];

const win = {
  document: doc,
  location: { href: '/app/home/', pathname: '/app/home/' },
  addEventListener(type, fn) { doc.addEventListener(type, fn); },
  removeEventListener(type, fn) { doc.removeEventListener(type, fn); },
  dispatchEvent(event) { doc.dispatchEvent(event); },
  sessionStorage: {
    getItem() { return '{}'; },
    setItem() {},
    removeItem() {},
  },
  apiJson(url, options) {
    const record = {
      url,
      method: options?.method || 'GET',
      noAuthRedirect: options?.noAuthRedirect,
      body: options?.body ? JSON.parse(options.body) : undefined,
    };
    apiRequests.push(record);
    if (apiResolver) {
      return apiResolver(url, options, record);
    }
    return Promise.resolve({});
  },
  setTimeout(fn, delay) {
    const id = nextTimerId++;
    activeTimers.set(id, { fn, delay });
    timerLog.push({ id, delay });
    return id;
  },
  clearTimeout(id) {
    activeTimers.delete(id);
  },
  JournalFormat: {
    day: (d) => d,
    dayFull: (d) => 'day ' + d,
    time: (t) => t,
    timestamp: (ts) => ts,
  },
  SurfaceState: {
    loading: () => '<div>loading</div>',
    error: () => '<div>error</div>',
    serverMessageFrom: () => '',
  },
  AppServices: {
    renderMarkdown: (m) => m,
  },
  convey: {
    copyToClipboard(cmd) {
      win.__copied = cmd;
    },
  },
};
win.window = win;
doc.defaultView = win;

vm.runInNewContext(source, {
  window: win,
  document: doc,
  console,
  Date,
  sessionStorage: win.sessionStorage,
  setTimeout: (fn, delay) => win.setTimeout(fn, delay),
  clearTimeout: (id) => win.clearTimeout(id),
}, { filename: 'home.js' });
const {
  renderPulse,
  renderVitals,
  renderNarrative,
  renderBriefing,
  selectAccountSide,
  handleDashboardKeydown,
  readVitals,
  getSides,
  readFirstMinutes,
  readAgents,
  handleSpeechRetry,
  updateGettingStartedSurface,
  initHome,
  setSurface,
  setRoot,
  getState,
  setState,
} = win.__home_test;

assert(renderPulse, 'renderPulse exported');
setSurface(surface);
setRoot(rootEl);

async function runTests() {
  // 1. STEP 1 BASELINE TEST:
  // A welcome-state journal shows the status line, anything waiting as rows
  // under it, and the welcome line directly after, with no cards.
  const welcomePulse = {
    home_state: 'welcome',
    health_glance: { verdict: 'calm', severity: 'neutral', headline: 'devices quiet' },
    narrative_content: null,
    needs_you_items: [{ text: 'set up your voice', kind: 'route', payload: { href: '/app/speakers/#your-voice' } }],
    today: '20261017',
    now: '2026-10-17T12:00:00Z',
    segment_count: 0,
    activities: [],
  };

  setState({ firstMinutesPayload: null, setupPayload: null, agentsPayload: null, agentsSettled: false });
  renderPulse(welcomePulse);

  const vitalsEl = surface.querySelector('[data-home-surface="vitals"]');
  const welcomeEl = surface.querySelector('[data-home-surface="welcome"]');
  assert(vitalsEl, 'vitals present');
  assert(welcomeEl, 'welcome line present');
  assert(vitalsEl.querySelector('[href="/app/speakers/#your-voice"]'), 'the voice prompt is a waiting row');
  assert.strictEqual(surface.querySelector('[data-home-surface="account"]'), null, 'no account card in the welcome state');
  assert.strictEqual(surface.querySelector('[data-home-surface="pair"]'), null, 'no empty cards in the welcome state');
  assert.strictEqual(surface.children.indexOf(welcomeEl), surface.children.indexOf(vitalsEl) + 1, 'welcome line directly under the status');

  // A live refresh on the welcome page adds no connections or week row, even
  // when the pulse carries a first-week line.
  apiResolver = (url) => url === '/app/home/api/pulse'
    ? Promise.resolve({ ...welcomePulse, latest_weekly_reflection: { state: 'first', line: 'your first week comes on monday.' } })
    : new Promise(() => {});
  await readVitals();
  assert.strictEqual(surface.querySelector('[data-home-surface="pair"]'), null, 'a refresh adds no pair to the welcome page');
  apiResolver = null;

  // A live re-render of the status keeps the order.
  renderVitals(welcomePulse);
  assert.strictEqual(
    surface.children.indexOf(surface.querySelector('[data-home-surface="welcome"]')),
    surface.children.indexOf(surface.querySelector('[data-home-surface="vitals"]')) + 1,
    'welcome line stays under the status after a live update'
  );

  // 2. DEVICE STATES & PHONE CHECK:
  for (const st of ['awaiting_first', 'receiving', 'quiet', 'other']) {
    setState({
      firstMinutesPayload: {
        window_open: true,
        devices_known: true,
        devices: [{ name: 'TestDevice', state: st, last_added_label: '2 minutes ago' }],
        checks: { phone: false, thinking: true },
      },
      agentsPayload: { connections: [] },
      agentsSettled: true,
    });
    renderPulse(welcomePulse);
    const devEl = surface.querySelector(`[data-device-state="${st}"]`);
    assert(devEl, `device state ${st} rendered with [data-device-state="${st}"]`);
  }

  // No device element when devices is empty or devices_known is false
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: false,
      devices: [{ name: 'TestDevice', state: 'receiving' }],
      checks: { phone: false, thinking: true },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-device-state]'), 'no device element when devices_known is false');

  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-device-state]'), 'no device element when devices is empty');

  // Phone check:
  // checks.phone === null: no phone step rendered
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: null, thinking: false },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-phone-step]'), 'no phone step when checks.phone is null');

  // checks.phone === false: phone step and link rendered
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  const phoneStepEl = surface.querySelector('[data-phone-step]');
  assert(phoneStepEl, 'phone step rendered when checks.phone is false');
  assert(phoneStepEl.querySelector('a[href="/app/network/#devices"]'), 'phone step links to /app/network/#devices');

  // checks.phone === true: no phone step
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: true, thinking: false },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-phone-step]'), 'no phone step when checks.phone is true');

  // 3. FIRST SEGMENT:
  for (const th of ['read', 'pending', 'none']) {
    setState({
      firstMinutesPayload: {
        window_open: true,
        devices_known: true,
        devices: [],
        first_segment: {
          start_at: '2026-10-10T12:00:00Z',
          href: '/app/transcripts/20261010?stream=phone#120000_300',
          thinking: th,
        },
        checks: { phone: false, thinking: true },
      },
      agentsPayload: { connections: [] },
      agentsSettled: true,
    });
    renderPulse(welcomePulse);
    const segEl = surface.querySelector(`[data-segment-thinking="${th}"]`);
    assert(segEl, `segment thinking ${th} rendered with [data-segment-thinking="${th}"]`);
    assert(segEl.querySelector('a[href="/app/transcripts/20261010?stream=phone#120000_300"]'), 'segment links to payload href');
  }

  // first_segment null: no segment element
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      first_segment: null,
      checks: { phone: false, thinking: true },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-segment-thinking]'), 'no segment element when first_segment is null');

  // 4. THINKING STEPS & CLIENT ADJUSTMENTS:
  const thinkingExpectedLinks = {
    choose: '/app/thinking/',
    cant_run: '/app/thinking/',
    byo: '/app/thinking/#byo-setup',
    confidential: '/app/thinking/#confidential-setup',
    local: '/app/thinking/#local-setup',
  };

  for (const [st, href] of Object.entries(thinkingExpectedLinks)) {
    setState({
      firstMinutesPayload: {
        window_open: true,
        devices_known: true,
        devices: [],
        thinking: { step: st },
        checks: { phone: true, thinking: false },
      },
      agentsPayload: { connections: [] },
      agentsSettled: true,
      setupPayload: null,
    });
    renderPulse(welcomePulse);
    const thStepEl = surface.querySelector(`[data-thinking-step="${st}"]`);
    assert(thStepEl, `thinking step ${st} rendered with [data-thinking-step="${st}"]`);
    assert(thStepEl.querySelector(`a[href="${href}"]`), `thinking step ${st} links to ${href}`);
  }

  // Adjustment 1: step === 'choose' and thinking.capability === 'blocked' -> cant_run
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'choose' },
      checks: { phone: true, thinking: false },
    },
    setupPayload: { thinking: { capability: 'blocked' } },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  const cantRunEl = surface.querySelector('[data-thinking-step="cant_run"]');
  assert(cantRunEl, 'choose with capability: blocked renders cant_run');
  assert(cantRunEl.querySelector('a[href="/app/thinking/"]'), 'cant_run links to /app/thinking/');

  // Blocked capability suppresses thinking error/cancelled downloads, keeps speech
  for (const dlState of ['error', 'cancelled']) {
    setState({
      firstMinutesPayload: {
        window_open: true,
        devices_known: true,
        devices: [],
        thinking: { step: 'choose' },
        checks: { phone: true, thinking: false },
      },
      setupPayload: {
        thinking: { capability: 'blocked' },
        downloads: {
          thinking: { state: dlState },
          speech: { state: 'error' },
        },
      },
      agentsPayload: { connections: [] },
      agentsSettled: true,
    });
    renderPulse(welcomePulse);
    assert(!surface.querySelector('[data-download="thinking"]'), `blocked capability suppresses thinking download ${dlState}`);
    assert(surface.querySelector('[data-download="speech"]'), `blocked capability still renders speech download ${dlState}`);
  }

  // Unknown capability allows thinking download error
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'choose' },
      checks: { phone: true, thinking: false },
    },
    setupPayload: {
      thinking: { capability: 'unknown' },
      downloads: { thinking: { state: 'error' } },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-download="thinking"]'), 'unknown capability allows thinking download error');

  // Blocked capability keeps downloading state
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'choose' },
      checks: { phone: true, thinking: false },
    },
    setupPayload: {
      thinking: { capability: 'blocked' },
      downloads: { thinking: { state: 'downloading', bytes_done: 100, bytes_total: 1000 } },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-download="thinking"][data-download-state="downloading"]'), 'blocked capability still renders thinking downloading');

  // thinking.step === 'local' + capability blocked -> cant_run, no local
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'local' },
      checks: { phone: true, thinking: false },
    },
    setupPayload: { thinking: { capability: 'blocked' } },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-thinking-step="cant_run"]'), 'local step with capability: blocked renders cant_run');
  assert(!surface.querySelector('[data-thinking-step="local"]'), 'local step with capability: blocked has no local step');

  // step choose + thinking.choice === 'local' + blocked -> cant_run, not local
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'choose' },
      checks: { phone: true, thinking: false },
    },
    setupPayload: { thinking: { choice: 'local', capability: 'blocked' } },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-thinking-step="cant_run"]'), 'choose with choice: local and capability: blocked renders cant_run');
  assert(!surface.querySelector('[data-thinking-step="local"]'), 'choose with choice: local and capability: blocked has no local step');

  // step choose + choice byo + blocked -> byo, not cant_run
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'choose' },
      checks: { phone: true, thinking: false },
    },
    setupPayload: { thinking: { choice: 'byo', capability: 'blocked' } },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-thinking-step="byo"]'), 'choose with choice: byo and capability: blocked renders byo');
  assert(!surface.querySelector('[data-thinking-step="cant_run"]'), 'choose with choice: byo and capability: blocked does not render cant_run');

  // Saved setup intention: nothing active yet, but setup saved a choice ->
  // continue that lane; an active lane (any step but choose) is left alone.
  for (const [choice, expected, href] of [
    ['byo', 'byo', '/app/thinking/#byo-setup'],
    ['spp', 'confidential', '/app/thinking/#confidential-setup'],
    ['local', 'local', '/app/thinking/#local-setup'],
  ]) {
    setState({
      firstMinutesPayload: {
        window_open: true,
        devices_known: true,
        devices: [],
        thinking: { step: 'choose' },
        checks: { phone: true, thinking: false },
      },
      setupPayload: { thinking: { choice, capability: 'ok' } },
      agentsPayload: { connections: [] },
      agentsSettled: true,
    });
    renderPulse(welcomePulse);
    const saved = surface.querySelector(`[data-thinking-step="${expected}"]`);
    assert(saved, `saved choice ${choice} continues as ${expected}`);
    assert(saved.querySelector(`a[href="${href}"]`), `saved choice ${choice} links to ${href}`);
  }
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'byo' },
      checks: { phone: true, thinking: false },
    },
    setupPayload: { thinking: { choice: 'spp', capability: 'ok' } },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-thinking-step="byo"]'), 'an active lane wins over the saved setup choice');
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'choose' },
      checks: { phone: true, thinking: false },
    },
    setupPayload: { thinking: { choice: 'later', capability: 'ok' } },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-thinking-step="choose"]'), 'later stays choose');

  // Adjustment 2: step === 'local' and downloads.thinking.state === 'downloading' -> no thinking step
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'local' },
      checks: { phone: false, thinking: false },
    },
    setupPayload: {
      downloads: { thinking: { state: 'downloading', bytes_done: 1e9, bytes_total: 2e9 } },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-thinking-step]'), 'local step with thinking downloading renders no thinking step');

  // 5. AGENTS:
  // Non-200 agents read (failed): agent step hidden, check done.
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: true, thinking: true },
    },
    agentsPayload: { failed: true },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  // All checks done -> card not visible, welcome line still there
  assert(!surface.querySelector('[data-home-surface="getting-started"]'), 'card hides when agent check done and others done');
  assert(surface.querySelector('[data-home-surface="welcome"]'), 'welcome line present');

  // Non-empty connections: hidden, check done
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: true, thinking: true },
    },
    agentsPayload: { connections: [{ id: 'agent-1' }] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-home-surface="getting-started"]'), 'non-empty connections counts check done');

  // Empty connections []: shown, check not done
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: true, thinking: true },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  const agentStepEl = surface.querySelector('[data-agent-step]');
  assert(agentStepEl, 'empty connections renders agent step');
  assert(agentStepEl.querySelector('a[href="/app/agents/"]'), 'agent step links to /app/agents/');

  // connections not an array: hidden, check NOT done (card stays if open)
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: true, thinking: true },
    },
    agentsPayload: { connections: null },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-home-surface="getting-started"]'), 'card stays when connections not an array');
  assert(!surface.querySelector('[data-agent-step]'), 'agent step hidden when connections not an array');

  // Command block:
  // local_door enabled and listening and address is string
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: true, thinking: true },
    },
    agentsPayload: {
      connections: [],
      local_agent_helpers: true,
      local_door: { enabled: true, listening: true, address: '127.0.0.1:8000' },
    },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-agent-command]'), '[data-agent-command] present when door enabled + listening + address string');

  // A mobile host still exposes the generic local door, but has no desktop
  // helper on that device. Keep the agent step for clients on another device.
  for (const helpers of [false, undefined]) {
    setState({
      firstMinutesPayload: {
        window_open: true,
        devices_known: true,
        devices: [],
        checks: { phone: true, thinking: true },
      },
      agentsPayload: {
        connections: [],
        local_agent_helpers: helpers,
        local_door: { enabled: true, listening: true, address: 'http://127.0.0.1:7659/mcp' },
      },
      agentsSettled: true,
    });
    renderPulse(welcomePulse);
    assert(!surface.querySelector('[data-agent-command]'), 'no same-device desktop command without helper capability');
    const step = surface.querySelector('[data-agent-step]');
    assert(step && step.querySelector('a[href="/app/agents/"]'), 'keep personal-agent connection guidance on a mobile host');
  }

  // door not enabled or not listening or not string
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: true, thinking: true },
    },
    agentsPayload: {
      connections: [],
      local_agent_helpers: true,
      local_door: { enabled: false, listening: true, address: '127.0.0.1:8000' },
    },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-agent-command]'), 'no agent command when door not enabled');

  // 6. DOWNLOADS:
  // Absent when downloads is missing or setup is 403
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
    },
    setupPayload: null,
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-download]'), 'no download lines when setupPayload is null');

  // Present for thinking and speech with downloading, error, cancelled
  for (const kind of ['thinking', 'speech']) {
    for (const dlState of ['downloading', 'error', 'cancelled']) {
      setState({
        firstMinutesPayload: {
          window_open: true,
          devices_known: true,
          devices: [],
          checks: { phone: false, thinking: true },
        },
        setupPayload: {
          downloads: {
            [kind]: {
              state: dlState,
              bytes_done: 1.2e9,
              bytes_total: 3.4e9,
            },
          },
        },
        agentsPayload: { connections: [] },
        agentsSettled: true,
      });
      renderPulse(welcomePulse);
      const dlEl = surface.querySelector(`[data-download="${kind}"][data-download-state="${dlState}"]`);
      assert(dlEl, `${kind} download in ${dlState} state rendered`);
      if (dlState === 'downloading') {
        assert(dlEl.querySelector(`[data-download-bar="${kind}"]`), `bar rendered for ${kind} downloading`);
      }
    }
  }

  // idle and complete omit the line
  for (const st of ['idle', 'complete']) {
    setState({
      firstMinutesPayload: {
        window_open: true,
        devices_known: true,
        devices: [],
        checks: { phone: false, thinking: true },
      },
      setupPayload: {
        downloads: {
          thinking: { state: st, bytes_done: 0, bytes_total: 100 },
          speech: { state: st, bytes_done: 0, bytes_total: 100 },
        },
      },
      agentsPayload: { connections: [] },
      agentsSettled: true,
    });
    renderPulse(welcomePulse);
    assert(!surface.querySelector('[data-download]'), `download line omitted for ${st} state`);
  }

  // Bar only when bytes_total > 0 and both finite
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
    },
    setupPayload: {
      downloads: {
        thinking: { state: 'downloading', bytes_done: NaN, bytes_total: 1e9 },
      },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-download-bar="thinking"]'), 'no bar when bytes_done is NaN');

  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
    },
    setupPayload: {
      downloads: {
        thinking: { state: 'downloading', bytes_done: 10, bytes_total: 0 },
      },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-download-bar="thinking"]'), 'no bar when bytes_total is 0');

  // Speech retry button re-enables after non-2xx POST:
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
    },
    setupPayload: {
      downloads: {
        speech: { state: 'error' },
      },
      actions: true,
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
    speechRetryInFlight: false,
  });
  renderPulse(welcomePulse);
  const speechRetryBtn = surface.querySelector('[data-speech-retry]');
  assert(speechRetryBtn, 'speech retry button present');
  assert(!speechRetryBtn.disabled, 'speech retry button initially enabled');

  apiRequests.length = 0;
  apiResolver = (url, options) => {
    if (url === '/init/api/action') {
      return Promise.reject(new Error('500 server error'));
    }
    return Promise.resolve({});
  };

  speechRetryBtn.click();
  assert(speechRetryBtn.disabled, 'button disabled while request in flight');
  assert.strictEqual(apiRequests.length, 1);
  assert.strictEqual(apiRequests[0].url, '/init/api/action');
  assert.strictEqual(apiRequests[0].method, 'POST');
  assert.strictEqual(apiRequests[0].noAuthRedirect, true);
  assert.deepStrictEqual(apiRequests[0].body, { action: 'retry_speech_download' });

  // Flush rejection
  await Promise.resolve();
  await Promise.resolve();
  assert(!speechRetryBtn.disabled, 'button re-enabled after non-2xx POST');

  // A missing bundled speech package is a repair, not a download: no retry,
  // and the journal's own guidance is shown.
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
    },
    setupPayload: {
      downloads: {
        speech: { state: 'error', error: { code: 'speech_package_unavailable', detail: 'repair guidance' } },
      },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
    speechRetryInFlight: false,
  });
  renderPulse(welcomePulse);
  assert(!surface.querySelector('[data-speech-retry]'), 'no retry for a missing speech package');
  assert(surface.querySelector('[data-speech-guidance]'), 'guidance shown for a missing speech package');
  assert.strictEqual(surface.querySelector('[data-download="speech"]').querySelectorAll('p').length, 1,
    'a missing speech package shows only the guidance, no download line');

  // A linked device reads the same progress but can't take setup actions:
  // the speech line shows without its retry, and the bars still render.
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
    },
    setupPayload: {
      downloads: {
        speech: { state: 'error', error: { code: 'install_failed', detail: '' } },
        thinking: { state: 'downloading', bytes_done: 1, bytes_total: 4 },
      },
      actions: false,
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
    speechRetryInFlight: false,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-download="speech"][data-download-state="error"]'),
    'linked device: speech error line shown');
  assert(!surface.querySelector('[data-speech-retry]'), 'linked device: no speech retry');
  assert(surface.querySelector('[data-download-bar="thinking"]'), 'linked device: thinking bar rendered');

  // Progress, capability and the saved choice arrive with first minutes, for
  // a Localhost home and a linked-device home alike; nothing reads
  // /init/api/state.
  for (const actions of [true, false]) {
    apiRequests.length = 0;
    setState({ firstMinutesClosed: false, firstMinutesPayload: null, setupPayload: null, downloadTimer: null });
    apiResolver = (url) => {
      if (url === '/app/home/api/first-minutes') {
        return Promise.resolve({
          window_open: true,
          devices_known: true,
          devices: [],
          thinking: { step: 'choose' },
          checks: { phone: true, thinking: false },
          setup: {
            downloads: {
              speech: { state: 'downloading', bytes_done: 2, bytes_total: 8 },
              thinking: { state: 'idle', bytes_done: 0, bytes_total: 9 },
            },
            thinking: { capability: 'ok', choice: 'byo' },
            actions,
          },
        });
      }
      return Promise.resolve({});
    };
    await readFirstMinutes();
    assert(surface.querySelector('[data-download-bar="speech"]'), `speech bar from first minutes (actions ${actions})`);
    const continued = surface.querySelector('[data-thinking-step="byo"]');
    assert(continued, `saved choice from first minutes continues own model (actions ${actions})`);
    assert(continued.querySelector('a[href="/app/thinking/#byo-setup"]'), 'own-model continuation links to its setup');
    assert(getState().downloadTimer !== null, 'a download from first minutes arms the refresh timer');
    assert(apiRequests.every(r => r.url !== '/init/api/state'), 'no /init/api/state read');
  }

  // Capability from first minutes: an unchosen lane on a computer that can't
  // run local thinking says so.
  setState({ firstMinutesClosed: false, firstMinutesPayload: null, setupPayload: null, downloadTimer: null });
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({
        window_open: true,
        devices_known: true,
        devices: [],
        thinking: { step: 'choose' },
        checks: { phone: true, thinking: false },
        setup: { downloads: null, thinking: { capability: 'blocked', choice: null }, actions: false },
      });
    }
    return Promise.resolve({});
  };
  await readFirstMinutes();
  assert(surface.querySelector('[data-thinking-step="cant_run"]'), 'capability from first minutes renders cant_run');
  assert(!surface.querySelector('[data-download]'), 'unknown progress renders no download line');
  activeTimers.clear();
  setState({ downloadTimer: null });

  // 7. DOWNLOAD TIMER (15000 ms):
  timerLog.length = 0;
  activeTimers.clear();
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
    },
    setupPayload: {
      downloads: { thinking: { state: 'downloading', bytes_done: 1, bytes_total: 10 } },
    },
    agentsPayload: { connections: [] },
    agentsSettled: true,
    downloadTimer: null,
  });

  // Re-read first minutes should schedule a 15000ms timer
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({
        window_open: true,
        setup: { downloads: { thinking: { state: 'downloading', bytes_done: 2, bytes_total: 10 } } },
      });
    }
    return Promise.resolve({});
  };
  await readFirstMinutes();
  assert(timerLog.some(t => t.delay === 15000), '15000ms timer scheduled while downloading');
  assert(getState().downloadTimer !== null, 'downloadTimer is active');

  // The timer refreshes first minutes alone
  apiRequests.length = 0;
  const armed = [...activeTimers.values()].find(t => t.delay === 15000);
  setState({ downloadTimer: null });
  armed.fn();
  assert.deepStrictEqual(apiRequests.map(r => r.url), ['/app/home/api/first-minutes'],
    'the download timer re-reads first minutes only');
  await readFirstMinutes(); // joins the timer's in-flight read

  // Later read with nothing downloading clears timer
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({
        window_open: true,
        setup: { downloads: { thinking: { state: 'complete' } } },
      });
    }
    return Promise.resolve({});
  };
  await readFirstMinutes();
  assert(getState().downloadTimer === null, 'downloadTimer cleared when download finished');

  // 8. WINDOW CLOSED LATCHING:
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({ window_open: false });
    }
    return Promise.resolve({});
  };
  setState({ firstMinutesClosed: false });
  await readFirstMinutes();
  assert.strictEqual(getState().firstMinutesClosed, true, 'firstMinutesClosed latched on window_open: false');

  apiRequests.length = 0;
  // Further calls do not request first-minutes, setup, or agents
  await readFirstMinutes();
  await readAgents();
  doc.dispatchEvent({ type: 'visibilitychange' });
  assert.strictEqual(apiRequests.length, 0, 'no requests made after window_open: false latched');

  // 9. NEXT HEADING:
  // Absent when no step remains
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'none' },
      checks: { phone: true, thinking: true },
    },
    setupPayload: null,
    agentsPayload: { connections: null }, // agents step hidden, but check not done
    agentsSettled: true,
    firstMinutesClosed: false,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-home-surface="getting-started"]'), 'card shown for open agent check');
  assert(!surface.querySelector('[data-getting-started-next]'), '[data-getting-started-next] absent when no step remains');

  // Present when at least one step remains
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'choose' },
      checks: { phone: true, thinking: false },
    },
    setupPayload: null,
    agentsPayload: { connections: [{ id: 'a' }] },
    agentsSettled: true,
    firstMinutesClosed: false,
  });
  renderPulse(welcomePulse);
  assert(surface.querySelector('[data-getting-started-next]'), '[data-getting-started-next] present when a step remains');

  // 10. GETTING STARTED BESIDE THE WELCOME LINE:
  // Getting started comes and goes; the welcome line stays, exactly once.
  assert(surface.querySelector('[data-home-surface="getting-started"]'), 'card shown before the failed read');
  assert.strictEqual(surface.querySelectorAll('[data-home-surface="welcome"]').length, 1, 'one welcome line beside getting started');
  // Card removed and welcome line kept under vitals when a later read fails
  setState({
    firstMinutesPayload: null,
    setupPayload: null,
    agentsPayload: null,
    agentsSettled: true,
    firstMinutesClosed: false,
  });
  updateGettingStartedSurface();
  assert(!surface.querySelector('[data-home-surface="getting-started"]'), 'card removed on read failure');
  assert.strictEqual(surface.querySelectorAll('[data-home-surface="welcome"]').length, 1, 'still one welcome line');
  const restoredWelcome = surface.querySelector('[data-home-surface="welcome"]');
  assert(restoredWelcome, 'welcome line kept on read failure');
  assert.strictEqual(
    surface.children.indexOf(restoredWelcome),
    surface.children.indexOf(surface.querySelector('[data-home-surface="vitals"]')) + 1,
    'the welcome line is directly under vitals'
  );

  // Response {} does not throw and does not latch
  apiResolver = () => Promise.resolve({});
  setState({ firstMinutesClosed: false });
  await readFirstMinutes();
  assert.strictEqual(getState().firstMinutesClosed, false, '{} does not latch firstMinutesClosed');

  // 11. ACTIVE DAY CARD PLACEMENT & SECTION DEFAULTS:
  const activePulse = {
    home_state: 'active',
    health_glance: { verdict: 'calm', severity: 'neutral', headline: 'devices quiet' },
    narrative_content: 'Some summary text',
    needs_you_items: [],
    today: '20261017',
    now: '2026-10-17T12:00:00Z',
    segment_count: 5,
    activities: [{ id: 1 }],
  };

  // Card shown
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      thinking: { step: 'choose' },
      checks: { phone: false, thinking: false },
    },
    setupPayload: null,
    agentsPayload: { connections: [] },
    agentsSettled: true,
    firstMinutesClosed: false,
  });
  renderPulse(activePulse);

  const activeVitals = surface.querySelector('[data-home-surface="vitals"]');
  const activeGS = surface.querySelector('[data-home-surface="getting-started"]');
  assert(activeVitals, 'active vitals present');
  assert(activeGS, 'active getting-started present');
  assert.strictEqual(
    surface.children.indexOf(activeGS),
    surface.children.indexOf(activeVitals) + 1,
    'card directly under vitals on active day'
  );

  // Live insertions via renderNarrative and renderNeedsYou keep card directly under vitals
  renderNarrative(activePulse);
  assert.strictEqual(
    surface.children.indexOf(surface.querySelector('[data-home-surface="getting-started"]')),
    surface.children.indexOf(surface.querySelector('[data-home-surface="vitals"]')) + 1,
    'card remains directly under vitals after renderNarrative'
  );

  renderVitals(activePulse);
  assert.strictEqual(
    surface.children.indexOf(surface.querySelector('[data-home-surface="getting-started"]')),
    surface.children.indexOf(surface.querySelector('[data-home-surface="vitals"]')) + 1,
    'card remains directly under vitals after a status re-render'
  );

  // 11b. THE DAY SWITCH:
  // A day with material opens on today; the panels follow the tab.
  assert.deepStrictEqual({ ...getSides() }, { paintedSide: 'today', selectedSide: 'today' }, 'a live day opens on today');
  const tabFor = side => surface.querySelector('#pulse-account-tab-' + side);
  const panelFor = side => surface.querySelector('#pulse-account-panel-' + side);
  assert.strictEqual(tabFor('today').getAttribute('aria-selected'), 'true');
  assert.strictEqual(tabFor('today').getAttribute('aria-controls'), 'pulse-account-panel-today');
  assert.strictEqual(panelFor('today').getAttribute('role'), 'tabpanel');
  assert.strictEqual(panelFor('yesterday').hasAttribute('hidden'), true);
  assert.strictEqual(surface.querySelector('[role="tablist"]') !== null, true, 'the switch is a tab list');
  // The date beside the switch is the shown side's day.
  assert.strictEqual(surface.querySelector('[data-account-day-text]').innerHTML.includes('20261017'), true);

  // A quiet day opens on yesterday, with yesterday's analysis day beside it.
  const quietPulse = { ...activePulse, segment_count: 0, activities: [], narrative_content: null, briefing_analysis_day: '20261016',
    briefing_exists: true, briefing_phase: 'morning', briefing_sections: { yesterday: '- a steady day.' }, briefing_meta: { generated: '2026-10-17T07:12:00' } };
  apiResolver = () => new Promise(() => {});
  setState({ firstMinutesPayload: null });
  renderPulse(quietPulse);
  assert.deepStrictEqual({ ...getSides() }, { paintedSide: 'yesterday', selectedSide: 'yesterday' }, 'a quiet day opens on yesterday');
  assert.strictEqual(panelFor('today').hasAttribute('hidden'), true);
  assert.strictEqual(surface.querySelector('[data-account-day-text]').innerHTML.includes('20261016'), true);

  // Today's first material arriving by a live refresh never flips the switch.
  apiResolver = (url) => url === '/app/home/api/pulse' ? Promise.resolve({ ...activePulse }) : new Promise(() => {});
  await readVitals();
  renderNarrative(activePulse);
  assert.strictEqual(getSides().selectedSide, 'yesterday', 'a refresh does not move the switch');

  // The owner's tap holds through refreshes.
  selectAccountSide('today', false);
  assert.strictEqual(panelFor('yesterday').hasAttribute('hidden'), true);
  assert.strictEqual(panelFor('today').hasAttribute('hidden'), false);
  renderBriefing({ exists: true, phase: 'active', analysis_day: '20261016', meta: null, sections: { yesterday: '- a steady day.' } });
  renderNarrative(activePulse);
  assert.strictEqual(getSides().selectedSide, 'today', "the owner's choice survives refreshes");
  assert.strictEqual(tabFor('today').getAttribute('aria-selected'), 'true');

  // Arrow keys move the selection and focus with it.
  handleDashboardKeydown({ key: 'ArrowRight', target: tabFor('today'), preventDefault() {} });
  assert.strictEqual(getSides().selectedSide, 'yesterday', 'ArrowRight wraps to yesterday');
  assert.strictEqual(doc.activeElement, tabFor('yesterday'), 'focus follows the selection');
  handleDashboardKeydown({ key: 'End', target: tabFor('yesterday'), preventDefault() {} });
  assert.strictEqual(getSides().selectedSide, 'today');

  // Focus inside the side a refresh re-renders comes back to the same link.
  selectAccountSide('yesterday', false);
  const runLink = panelFor('yesterday').querySelector('a');
  assert(runLink, 'the yesterday side links its run');
  runLink.focus();
  renderBriefing({ exists: true, phase: 'active', analysis_day: '20261016', meta: null, sections: { yesterday: '- a steady day.' } });
  assert.strictEqual(doc.activeElement && doc.activeElement.getAttribute('href'), runLink.getAttribute('href'), 'focus returns to the same link');
  assert.notStrictEqual(doc.activeElement, runLink, 'the link was re-rendered');

  // An account with an empty yesterday section is still an account.
  renderBriefing({ exists: true, phase: 'active', analysis_day: '20261016', meta: null, sections: {} });
  assert.strictEqual(panelFor('yesterday').querySelector('.pulse-briefing-status-link'), null, 'never "not written" for an account');
  assert(panelFor('yesterday').querySelector('.pulse-briefing-placeholder'), 'says the section is empty');

  // A missing account painted on today rides the waiting list until a
  // briefing read brings it.
  const missingNight = { ...activePulse, briefing_analysis_day: '20261016', briefing_exists: false, briefing_phase: 'missing',
    yesterday_processing: { mode: 'degraded', summary_line: "some overnight processing didn't finish.",
      gap_links: [{ text: "yesterday wasn't written up.", href: '/app/thinking/#runs/20261016/morning_briefing' }], status_reasons: ['briefing_missing'] } };
  renderPulse(missingNight);
  const accountRow = () => surface.querySelector('[data-home-surface="vitals"]').querySelector('[href="/app/thinking/#runs/20261016/morning_briefing"]');
  assert(accountRow(), 'the missing account is a waiting row when the switch opened on today');
  renderBriefing({ exists: true, phase: 'active', analysis_day: '20261016', meta: null, sections: { yesterday: '- late, but here.' } });
  assert.strictEqual(accountRow(), null, 'an account that arrives takes its row away');

  // The zone note sits beside the date when the journal's zone differs, and
  // survives a refresh that re-renders the date.
  const placed = [];
  win.JournalClock = { placeNote(anchor, options) { placed.push({ anchor, options }); } };
  renderPulse(activePulse);
  assert(placed.some(entry => entry.anchor.hasAttribute('data-account-day')), 'zone note anchored beside the date');
  placed.length = 0;
  renderBriefing({ exists: true, phase: 'active', analysis_day: '20261016', meta: null, sections: {} });
  selectAccountSide('yesterday', false);
  assert(placed.some(entry => entry.anchor.hasAttribute('data-account-day')), 'zone note placed again after the date changes');
  delete win.JournalClock;

  // 12. SAY A SENTENCE LAYER (PROOF) TESTS:
  const shownStates = ['waiting', 'no_audio', 'transcribing', 'nothing_heard', 'failed', 'heard'];
  for (const st of shownStates) {
    const proofObj = {
      state: st,
      start_at: '2026-10-10T12:00:00Z',
      href: st === 'failed' ? '/app/health/' : (st === 'heard' ? '/app/transcripts/20261010?stream=phone#120000_300' : undefined),
      text: st === 'heard' ? 'hello test speech' : undefined,
      fresh: st === 'heard' ? true : undefined,
    };
    setState({
      firstMinutesPayload: {
        window_open: true,
        devices_known: true,
        devices: [],
        first_segment: {
          start_at: '2026-10-10T12:00:00Z',
          href: '/app/transcripts/20261010?stream=phone#120000_300',
          thinking: 'read',
        },
        checks: { phone: false, thinking: true, heard: false },
        proof: proofObj,
      },
      setupPayload: null,
      agentsPayload: { connections: [] },
      agentsSettled: true,
      firstMinutesClosed: false,
    });
    updateGettingStartedSurface();

    const gsCard = surface.querySelector('[data-home-surface="getting-started"]');
    assert(gsCard, 'getting-started card present for state ' + st);
    const proofEl = gsCard.querySelector('[data-proof-state="' + st + '"]');
    assert(proofEl, 'data-proof-state="' + st + '" present inside card');

    const segThinkingEl = gsCard.querySelector('[data-segment-thinking]');
    const nextEl = gsCard.querySelector('[data-getting-started-next]');
    assert(segThinkingEl, 'segment-thinking present');
    assert(nextEl, 'getting-started-next present');

    const segIdx = gsCard.children.indexOf(segThinkingEl);
    const proofIdx = gsCard.children.indexOf(proofEl);
    const nextIdx = gsCard.children.indexOf(nextEl);
    assert(segIdx < proofIdx, 'proof element sits after segment-thinking for ' + st);
    assert(proofIdx < nextIdx, 'proof element sits before getting-started-next for ' + st);

    if (st === 'failed') {
      const a = proofEl.querySelector('a');
      assert(a, 'failed state has anchor');
      assert.strictEqual(a.getAttribute('href'), '/app/health/', 'failed anchor points to /app/health/');
    } else if (st === 'heard') {
      const a = proofEl.querySelector('a');
      assert(a, 'heard state has anchor');
      assert.strictEqual(a.getAttribute('href'), proofObj.href, 'heard anchor matches payload href');
    } else {
      assert.strictEqual(proofEl.querySelector('a'), null, 'no anchor inside proof element for ' + st);
    }
  }

  // Fresh heard vs not-fresh heard and XSS escaping
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
      proof: {
        state: 'heard',
        start_at: '2026-10-10T12:00:00Z',
        href: '/app/transcripts/test',
        text: '<img src=x onerror=alert(1)>',
        fresh: true,
      },
    },
    setupPayload: null,
    agentsPayload: { connections: [] },
    agentsSettled: true,
    firstMinutesClosed: false,
  });
  updateGettingStartedSurface();
  const freshGsCard = surface.querySelector('[data-home-surface="getting-started"]');
  const freshProofEl = freshGsCard.querySelector('[data-proof-state="heard"]');
  assert(freshProofEl, 'fresh heard proof element present');
  assert(freshProofEl.querySelector('q'), 'fresh heard has q element');
  assert.strictEqual(freshProofEl.querySelector('img'), null, 'XSS img not rendered in DOM');

  // Not fresh heard: has link, no q
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: false, thinking: true },
      proof: {
        state: 'heard',
        start_at: '2026-10-10T12:00:00Z',
        href: '/app/transcripts/test',
        text: 'stale speech',
        fresh: false,
      },
    },
    setupPayload: null,
    agentsPayload: { connections: [] },
    agentsSettled: true,
    firstMinutesClosed: false,
  });
  updateGettingStartedSurface();
  const staleGsCard = surface.querySelector('[data-home-surface="getting-started"]');
  const staleProofEl = staleGsCard.querySelector('[data-proof-state="heard"]');
  assert(staleProofEl, 'stale heard proof element present');
  assert(staleProofEl.querySelector('a'), 'stale heard has link');
  assert.strictEqual(staleProofEl.querySelector('q'), null, 'stale heard has no q element');

  // none and missing proof render no data-proof-state
  for (const pr of [{ state: 'none' }, undefined]) {
    setState({
      firstMinutesPayload: {
        window_open: true,
        devices_known: true,
        devices: [],
        checks: { phone: false, thinking: true },
        proof: pr,
      },
      setupPayload: null,
      agentsPayload: { connections: [] },
      agentsSettled: true,
      firstMinutesClosed: false,
    });
    updateGettingStartedSurface();
    const c = surface.querySelector('[data-home-surface="getting-started"]');
    assert(c, 'card present');
    assert.strictEqual(c.querySelector('[data-proof-state="none"]'), null, 'none state has no proof element');
    assert.strictEqual(c.querySelector('[data-proof-state]'), null, 'no proof element when state is none or missing');
  }

  // Visibility: phone true, thinking true, agents done
  // checks.heard: null hides card
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: true, thinking: true, heard: null },
      proof: { state: 'none' },
    },
    setupPayload: null,
    agentsPayload: { connections: [{ id: 'agent-1' }] },
    agentsSettled: true,
    firstMinutesClosed: false,
  });
  updateGettingStartedSurface();
  assert.strictEqual(
    surface.querySelector('[data-home-surface="getting-started"]'),
    null,
    'checks.heard null hides card when other checks done'
  );

  // checks.heard: false shows card
  setState({
    firstMinutesPayload: {
      window_open: true,
      devices_known: true,
      devices: [],
      checks: { phone: true, thinking: true, heard: false },
      proof: { state: 'waiting' },
    },
    setupPayload: null,
    agentsPayload: { connections: [{ id: 'agent-1' }] },
    agentsSettled: true,
    firstMinutesClosed: false,
  });
  updateGettingStartedSurface();
  assert(
    surface.querySelector('[data-home-surface="getting-started"]'),
    'checks.heard false shows card even when other checks done'
  );

  // Timer tests at end of runTests
  timerLog.length = 0;
  activeTimers.clear();
  setState({ firstMinutesClosed: false, downloadTimer: null, firstMinutesPayload: null, setupPayload: null });

  // 1. a first-minutes read with no download and no pending proof does not arm timer
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({ window_open: true, setup: { downloads: { thinking: { state: 'idle' } } } });
    }
    return Promise.resolve({});
  };
  await readFirstMinutes();
  assert.strictEqual(getState().downloadTimer, null, 'no download and no pending proof does not arm timer');

  // 2. following readFirstMinutes with proof.state: "waiting" arms 15000ms timer
  timerLog.length = 0;
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({
        window_open: true,
        proof: { state: 'waiting' },
      });
    }
    return Promise.resolve({});
  };
  await readFirstMinutes();
  assert(timerLog.some(t => t.delay === 15000), '15000ms timer scheduled on waiting proof');
  const timerAfterWaiting = getState().downloadTimer;
  assert(timerAfterWaiting !== null, 'downloadTimer is active for waiting proof');

  // 3. a later read still waiting, with no download, leaves the timer armed
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({
        window_open: true,
        proof: { state: 'waiting' },
        setup: { downloads: { thinking: { state: 'idle' } } },
      });
    }
    return Promise.resolve({});
  };
  await readFirstMinutes();
  assert.strictEqual(getState().downloadTimer, timerAfterWaiting, 'later read with no download leaves timer armed');

  // 4. transcribing arms timer
  timerLog.length = 0;
  setState({ firstMinutesClosed: false, downloadTimer: null, firstMinutesPayload: null });
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({
        window_open: true,
        proof: { state: 'transcribing', start_at: '2026-10-10T12:00:00Z' },
      });
    }
    return Promise.resolve({});
  };
  await readFirstMinutes();
  assert(timerLog.some(t => t.delay === 15000), '15000ms timer scheduled for transcribing');
  assert(getState().downloadTimer !== null, 'downloadTimer is active for transcribing');

  // 5. heard with fresh: true arms timer
  timerLog.length = 0;
  setState({ firstMinutesClosed: false, downloadTimer: null, firstMinutesPayload: null });
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({
        window_open: true,
        proof: {
          state: 'heard',
          start_at: '2026-10-10T12:00:00Z',
          href: '/app/transcripts/test',
          text: 'fresh text',
          fresh: true,
        },
      });
    }
    return Promise.resolve({});
  };
  await readFirstMinutes();
  assert(timerLog.some(t => t.delay === 15000), '15000ms timer scheduled for fresh heard');
  assert(getState().downloadTimer !== null, 'downloadTimer is active for fresh heard');

  // 6. heard with fresh: false leaves downloadTimer null
  timerLog.length = 0;
  setState({ firstMinutesClosed: false, downloadTimer: null, firstMinutesPayload: null });
  apiResolver = (url) => {
    if (url === '/app/home/api/first-minutes') {
      return Promise.resolve({
        window_open: true,
        proof: {
          state: 'heard',
          start_at: '2026-10-10T12:00:00Z',
          href: '/app/transcripts/test',
          text: 'stale text',
          fresh: false,
        },
      });
    }
    return Promise.resolve({});
  };
  await readFirstMinutes();
  assert.strictEqual(getState().downloadTimer, null, 'heard with fresh false leaves downloadTimer null');

  console.log('all getting_started_render acceptance assertions passed');
}

runTests().catch((err) => {
  console.error(err);
  process.exit(1);
});
