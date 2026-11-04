// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

// A click that posts flow does not await the reply. The browser keeps the
// page up when that reply is a refusal; Node would stop the process.
process.on('unhandledRejection', () => {});

const darkResume = process.argv.includes('--dark-resume');
const dark = process.argv.includes('--dark') || darkResume;
let darkProbe = 'incomplete';
let darkStatus = darkResume ? { ...statusBody('running', true), set_up: true, service: { ...statusBody('running', true).service, starts_at_sign_in: false } } : null;

const here = dirname(fileURLToPath(import.meta.url));
const html = readFileSync(join(here, '../ui/index.html'), 'utf8');

class DomNode {
  constructor() {
    this.parentNode = null;
    this.childNodes = [];
    this.nodeType = 1;
  }

  append(...children) {
    for (let child of children) {
      if (child == null || child === false) continue;
      if (typeof child === 'string' || typeof child === 'number') {
        child = document.createTextNode(String(child));
      }
      if (child.parentNode) {
        child.parentNode.childNodes = child.parentNode.childNodes.filter(node => node !== child);
      }
      child.parentNode = this;
      this.childNodes.push(child);
    }
  }

  appendChild(child) {
    this.append(child);
    return child;
  }

  replaceChildren(...children) {
    for (const child of this.childNodes) child.parentNode = null;
    this.childNodes = [];
    this.append(...children);
    if (document.activeElement && !document.body?.contains(document.activeElement)) {
      document.activeElement = document.body;
    }
  }

  remove() {
    if (!this.parentNode) return;
    this.parentNode.childNodes = this.parentNode.childNodes.filter(node => node !== this);
    this.parentNode = null;
    if (document.activeElement && !document.body?.contains(document.activeElement)) {
      document.activeElement = document.body;
    }
  }

  contains(node) {
    let current = node;
    while (current) {
      if (current === this) return true;
      current = current.parentNode;
    }
    return false;
  }
}

class DomText extends DomNode {
  constructor(data) {
    super();
    this.nodeType = 3;
    this.data = String(data);
    this.tagName = '#text';
    this.localName = '#text';
  }

  get textContent() {
    return this.data;
  }

  set textContent(value) {
    this.data = String(value);
  }
}

class DomElement extends DomNode {
  constructor(tag) {
    super();
    this.tagName = String(tag).toUpperCase();
    this.localName = String(tag).toLowerCase();
    this.attributes = {};
    this.listeners = {};
    this.style = {};
    this.hidden = false;
    this._className = '';
    this._value = '';
    this._disabled = false;
    this.checked = false;
    this.id = '';
    const self = this;
    this.classList = {
      add(...names) {
        const set = new Set(self.className.split(/\s+/).filter(Boolean));
        for (const name of names) set.add(name);
        self.className = [...set].join(' ');
      },
      remove(...names) {
        const drop = new Set(names);
        self.className = self.className.split(/\s+/).filter(name => name && !drop.has(name)).join(' ');
      },
      toggle(name, force) {
        const has = this.contains(name);
        const on = force === undefined ? !has : Boolean(force);
        if (on) this.add(name);
        else this.remove(name);
        return on;
      },
      contains(name) {
        return self.className.split(/\s+/).includes(name);
      },
    };
  }

  get className() {
    return this._className;
  }

  set className(value) {
    this._className = value == null ? '' : String(value);
  }

  get value() {
    return this._value;
  }

  set value(value) {
    this._value = String(value);
  }

  get disabled() {
    return this._disabled;
  }

  set disabled(value) {
    this._disabled = Boolean(value);
  }

  set tabIndex(value) {
    this._tabIndex = value;
    this.attributes.tabindex = String(value);
  }

  get textContent() {
    return this.childNodes.map(child => child.textContent).join('');
  }

  set textContent(value) {
    this.childNodes = [];
    const text = new DomText(value ?? '');
    text.parentNode = this;
    this.childNodes = [text];
  }

  get children() {
    return this.childNodes.filter(child => child.nodeType === 1);
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'id') this.id = String(value);
    if (name === 'class') this.className = String(value);
    if (name === 'value') this._value = String(value);
    if (name === 'disabled') this._disabled = true;
    if (name === 'checked') this.checked = true;
  }

  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'id') this.id = '';
    if (name === 'class') this.className = '';
    if (name === 'value') this._value = '';
    if (name === 'disabled') this._disabled = false;
    if (name === 'checked') this.checked = false;
  }

  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  hasAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name);
  }

  addEventListener(type, handler) {
    (this.listeners[type] ||= []).push(handler);
  }

  focus() {
    document.activeElement = this;
  }

  select() {}

  click() {
    const event = { type: 'click', currentTarget: this, target: this };
    for (const handler of this.listeners.click || []) handler(event);
  }

  querySelector(selector) {
    return query(this, selector);
  }

  querySelectorAll(selector) {
    const results = [];
    walk(this, element => { if (matches(element, selector)) results.push(element); });
    return results;
  }
}

function walk(node, visit) {
  for (const child of node.childNodes) {
    visit(child);
    if (child.childNodes) walk(child, visit);
  }
}

function matches(element, selector) {
  if (!(element instanceof DomElement)) return false;
  let s = selector.trim();
  if (!s) return false;
  const tagMatch = s.match(/^([a-zA-Z0-9_-]+)/);
  if (tagMatch) {
    if (element.localName !== tagMatch[1].toLowerCase()) return false;
    s = s.slice(tagMatch[1].length);
  }
  while (s.length > 0) {
    if (s.startsWith('.')) {
      const m = s.match(/^\.([a-zA-Z0-9_-]+)/);
      if (!m) return false;
      if (!element.classList.contains(m[1])) return false;
      s = s.slice(m[0].length);
    } else if (s.startsWith('#')) {
      const m = s.match(/^#([a-zA-Z0-9_-]+)/);
      if (!m) return false;
      if (element.id !== m[1]) return false;
      s = s.slice(m[0].length);
    } else if (s.startsWith('[')) {
      const m = s.match(/^\[([a-zA-Z0-9_-]+)(?:=([^\]]+))?\]/);
      if (!m) return false;
      const attrName = m[1];
      let expectedVal = m[2];
      if (expectedVal !== undefined) {
        expectedVal = expectedVal.replace(/^["']|["']$/g, '');
        if (element.getAttribute(attrName) !== expectedVal) return false;
      } else {
        if (!element.hasAttribute(attrName)) return false;
      }
      s = s.slice(m[0].length);
    } else {
      return false;
    }
  }
  return true;
}

function query(root, selector) {
  let current = [root];
  for (const part of selector.trim().split(/\s+/)) {
    const next = [];
    for (const node of current) walk(node, element => { if (matches(element, part)) next.push(element); });
    current = next;
  }
    return current[0] || null;
}

const document = {
  activeElement: null,
  body: null,
  fonts: {
    load: () => Promise.resolve([]),
  },
  createElement(tag) {
    if (tag.toLowerCase() === 'canvas') {
      const el = new DomElement('canvas');
      const ctx = {
        fillStyle: '',
        font: '',
        textAlign: '',
        textBaseline: '',
        globalAlpha: 1,
        strokeStyle: '',
        lineWidth: 1,
        lineCap: '',
        lineJoin: '',
        save() {},
        restore() {},
        setTransform() {},
        fill() {},
        stroke() {},
        translate() {},
        rotate() {},
        scale() {},
        beginPath() {},
        roundRect() {},
        setLineDash() {},
        fillText() {},
        measureText() { return { width: 1 }; },
      };
      el.getContext = () => ctx;
      el.toBlob = (callback) => {
        callback({
          arrayBuffer: async () => new Uint8Array([1]).buffer,
        });
      };
      return el;
    }
    return new DomElement(tag);
  },
  createElementNS(_namespace, tag) {
    return new DomElement(tag);
  },
  createTextNode(text) {
    return new DomText(text);
  },
  getElementById(id) {
    let found = null;
    walk(document.body, element => { if (element.id === id) found = element; });
    return found;
  },
  execCommand() {
    return false;
  },
  querySelector(selector) {
    return query(document.body, selector);
  },
  querySelectorAll(selector) {
    return document.body.querySelectorAll(selector);
  },
};
document.body = new DomElement('body');
document.activeElement = document.body;

globalThis.Node = DomNode;
globalThis.Path2D = class {
  constructor(_arg) {}
  addPath() {}
  arc() {}
  ellipse() {}
  roundRect() {}
  moveTo() {}
  lineTo() {}
  closePath() {}
};

globalThis.DOMParser = class {
  parseFromString(xmlString) {
    const doc = new DomElement('doc');
    const root = new DomElement('svg');
    doc.documentElement = root;
    doc.querySelector = () => null;
    root.querySelector = () => null;

    let inner = String(xmlString).trim();
    const openMatch = inner.match(/^<svg[^>]*>/i);
    if (openMatch) {
      inner = inner.slice(openMatch[0].length);
    }
    const closeMatch = inner.match(/<\/svg>$/i);
    if (closeMatch) {
      inner = inner.slice(0, inner.length - closeMatch[0].length);
    }
    inner = inner.trim();

    const tagRegex = /<([a-zA-Z0-9]+)\s*([^>]*?)(?:\/>|>[\s\S]*?<\/\1>|>)/g;
    let match;
    while ((match = tagRegex.exec(inner)) !== null) {
      const tagName = match[1].toLowerCase();
      const rawAttrs = match[2];
      const el = new DomElement(tagName);
      el.localName = tagName;
      const attrRegex = /([a-zA-Z0-9_-]+)=["']([^"']*)["']/g;
      let attrMatch;
      while ((attrMatch = attrRegex.exec(rawAttrs)) !== null) {
        el.setAttribute(attrMatch[1], attrMatch[2]);
      }
      root.append(el);
    }
    return doc;
  }
};
globalThis.document = document;
globalThis.window = globalThis;

// The sun arc engine (sunarc.js) as the window loads it: a global the page mounts on its ground.
const sunArc = { mounts: 0, teardowns: 0, live: 0, overlap: false, root: null };
globalThis.SunArc = {
  mount(element) {
    if (sunArc.live > 0) sunArc.overlap = true;
    sunArc.mounts += 1;
    sunArc.live += 1;
    sunArc.root = element;
    return { teardown() { sunArc.teardowns += 1; sunArc.live -= 1; } };
  },
};

const intervals = new Map();
const realSetInterval = globalThis.setInterval.bind(globalThis);
globalThis.setInterval = (fn, ms, ...args) => {
  const timer = realSetInterval(fn, ms, ...args);
  timer.unref?.();
  intervals.set(timer, fn);
  return timer;
};

const ids = ['loading', 'setup-ground', 'firstrun', 'main', 'panes', 'pane', 'admin-terminal', 'quit', 'overlay', 'overlay-text'];
for (const id of ids) {
  if (!html.includes(`id="${id}"`)) throw new Error(`index.html is missing #${id}`);
  const tag = id === 'pane' ? 'main' : id === 'panes' ? 'ul' : id === 'firstrun' ? 'section' : 'div';
  const element = document.createElement(tag);
  element.id = id;
  if (new RegExp(`id="${id}"[^>]*\\shidden`).test(html) || html.includes(`id="${id}" hidden`)) element.hidden = true;
  document.body.append(element);
}

const calls = [];
const inbox = [];
let pairingStatusAnswer = { used: false };
const statusPlan = [];
const startPlan = [];
const syncPlan = [];
const identityPlan = [];
const flowPlan = [];
const setIconCalls = [];

function send(id, ok, extra) {
  window.journalApp.receive({ type: 'reply', id, ok, ...extra });
}

function cachedSeedMark() {
  return {
    words: ['cached', 'seed'],
    icon1: {
      svg: '<path d="M 2 2 L 20 20" />',
      color: { name: 'orange', hex: '#E8913A' },
      rot: 0,
    },
    icon2: {
      svg: '<circle cx="12" cy="12" r="10" />',
      color: { name: 'gold', hex: '#D4A017' },
      rot: 45,
    },
  };
}

function statusBody(display, installed = false) {
  return {
    service: {
      installed,
      port: 5016,
      starts_at_sign_in: true,
      journal: 'C:\\journal',
      running: display === 'running',
    },
    set_up: false,
    answering: display === 'running',
    display,
    about: 'about',
    base: 'http://127.0.0.1:5016',
  };
}

function view(extra) {
  return {
    busy: false,
    error: null,
    resting: false,
    sign_in: true,
    step: 1,
    watch_downloads: false,
    local_offered: false,
    thinking_saved: false,
    thinking_choice: null,
    thinking_cancel: false,
    thinking_retry: false,
    speech_retry: false,
    can_finalize: false,
    setup_running: false,
    setup_failed: false,
    mark: { proposal_id: null, render: null, state: 'unavailable' },
    downloads: {},
    models: { showing: false, detail: null, pending: false },
    copy: {
      heading: 'Head',
      welcome: { title: 'Welcome', body: 'Body' },
      fits: {
        title: 'Fits',
        journal_term: 'Journal',
        journal: 'keeps',
        app_term: 'App',
        app: 'takes',
        thinking_term: 'Think',
        thinking: 'helps',
      },
      login: { label: 'Sign in', help: 'Help' },
      folder: { title: 'Folder', help: 'Where' },
      mark: { title: 'Mark', help: 'Mh', error: 'mark-err', permanence: 'perm', try_another: 'Try', continue: 'Keep' },
      thinking: {
        title: 'Thinking',
        change: 'Change',
        later: 'Later',
        choose_later: 'Choose later',
        continue: 'Continue',
        choices: [
          { id: 'local', label: 'Local lane', sub: 'on this pc', description: 'local description' },
          { id: 'spp', label: 'Spp lane', sub: 'spp', description: 'spp description' },
          { id: 'byo', label: 'Byo lane', sub: 'byo', description: 'byo description' },
        ],
      },
      downloads: {
        speech: 'Speech',
        thinking: 'ThinkDl',
        retry: 'Retry',
        cancel: 'Cancel',
        cancelled: 'Cancelled',
      },
    },
    ...extra,
  };
}

window.qrcode = () => ({
  addData() {},
  make() {},
  getModuleCount: () => 2,
  isDark: () => false,
});

let homeHasLanded = false;

window.chrome = {
  webview: {
    postMessage(raw) {
      const message = JSON.parse(raw);
      const { id, cmd, args } = message;
      calls.push({ cmd, args });
      if (cmd === 'init') {
        send(id, true, {
          value: {
            launch: 'sign-in',
            app_version: '0',
            about: 'about',
            default_location: 'C:\\journal',
            mark: cachedSeedMark(),
            setup_sequence_enabled: !dark,
            flow: null,
          },
        });
        return;
      }
      if (dark && cmd === 'setup') {
        const status = darkStatus = { ...statusBody('running', true), set_up: true };
        send(id, true, { value: { status, models_missing: null } });
        return;
      }
      if (dark && cmd === 'initProbe') { send(id, true, { value: darkProbe }); return; }
      if (dark && cmd === 'modelsReady') { send(id, true, { value: true }); return; }
      if (dark && cmd === 'open') { send(id, true, { value: null }); return; }
      if (cmd === 'status') {
        const planned = statusPlan.shift();
        if (planned && planned.fail) {
          send(id, false, { error: planned.fail });
        } else if (planned) {
          send(id, true, { value: planned });
        } else {
          send(id, true, { value: darkStatus || statusBody('not-running', false) });
        }
        return;
      }
      if (cmd === 'start') {
        const planned = startPlan.shift();
        if (planned && planned.fail) {
          send(id, false, { error: planned.fail });
        } else if (planned) {
          send(id, true, { value: planned });
        } else {
          send(id, true, { value: statusBody('running', true) });
        }
        return;
      }
      if (cmd === 'syncSignInLaunch') {
        const planned = syncPlan.shift();
        if (planned && planned.fail) {
          send(id, false, { error: planned.fail });
        } else {
          send(id, true, { value: null });
        }
        return;
      }
      if (cmd === 'setIcon') {
        setIconCalls.push(args);
        send(id, true, { value: null });
        return;
      }
      if (cmd === 'ping' || cmd === 'holdsJournal' || cmd === 'quit' || cmd === 'adminTerminal') {
        send(id, true, { value: cmd === 'ping' ? { answering: false } : cmd === 'holdsJournal' ? false : null });
        return;
      }
      if (cmd === 'convey') {
        const path = args?.path || '';
        if (path === '/app/link/api/identity') {
          const planned = identityPlan.shift();
          if (planned && planned.fail) {
            send(id, false, { error: planned.fail });
          } else if (planned) {
            send(id, true, { value: planned });
          } else {
            send(id, true, { value: { committed: false, instance_id: null, mark: null } });
          }
          return;
        }
        let value = {};
        if (path === '/app/network/api/devices') value = { devices: [] };
        else if (path === '/app/network/pair-start') {
          value = { pair_link: 'https://pair.example/link', nonce: 'nonce-1', expires_in: 60 };
        } else if (path === '/app/network/api/local-network') value = { open: true };
        else value = { ok: true };
        send(id, true, { value });
        return;
      }
      if (cmd === 'pairingStatus') {
        send(id, true, { value: pairingStatusAnswer });
        return;
      }
      if (cmd === 'flow') {
        if (args?.intent === 'boot') {
          send(id, true, { value: view({ phase: 'welcome', step: 1 }) });
          return;
        }
        if (flowPlan.length > 0) {
          const planned = flowPlan.shift();
          if (planned.manual) {
            inbox.push(message);
            return;
          }
          if (planned.error) {
            send(id, false, { error: planned.error });
            return;
          }
          if (planned.value) {
            if (planned.value?.phase === 'home') homeHasLanded = true;
            send(id, true, { value: planned.value });
            return;
          }
        }
        if (args?.intent === 'retry-state' && homeHasLanded) {
          send(id, true, { value: view({ phase: 'home', step: 5 }) });
          return;
        }
        inbox.push(message);
        return;
      }
      inbox.push(message);
    },
  },
};

function flush() {
  return new Promise(resolve => setImmediate(resolve));
}

function $(id) {
  return document.getElementById(id);
}

function visible(element) {
  let current = element;
  while (current) {
    if (current.hidden) return false;
    current = current.parentNode;
  }
  return true;
}

function buttons() {
  const found = [];
  walk(document.body, element => {
    if (element.tagName === 'BUTTON' && visible(element)) found.push(element);
  });
  return found;
}

function accessibleName(button) {
  const aria = button.getAttribute('aria-label');
  if (typeof aria === 'string' && aria.trim().length > 0) return aria.trim();
  return button.textContent.trim();
}

function click(label) {
  const button = buttons().find(b => accessibleName(b) === label);
  if (!button) throw new Error(`no visible button ${label}; saw ${buttons().map(b => accessibleName(b)).join(' | ')}`);
  button.click();
}

function pressEnter() {
  if (document.activeElement?.tagName === 'BUTTON' && !document.activeElement.disabled) {
    document.activeElement.click();
  }
}

function answer(value) {
  const message = inbox.shift();
  if (!message) throw new Error('no pending call');
  send(message.id, true, { value });
}

function fail(error) {
  const message = inbox.shift();
  if (!message) throw new Error('no pending call');
  send(message.id, false, { error });
}

function paint(value) {
  if (value?.phase === 'home') homeHasLanded = true;
  window.journalApp.receive({ type: 'flow', view: value });
}

let passed = 0;
function pass(name) {
  passed += 1;
  if (process.env.DOM_CASE_LOG) console.error('ok', name);
}

const root = () => $('firstrun');

statusPlan.push({ fail: "couldn't read journal status" });

await import('../ui/app.js');
await flush();

if (!visible($('loading'))) throw new Error('#loading is not visible on initial status failure');
if (visible(root())) throw new Error('welcome is painted on initial status failure');
click('try again');
await flush();
if (calls.some(c => c.cmd === 'flow' && c.args?.intent === 'start-setup') || calls.some(c => c.cmd === 'open')) {
  throw new Error('initial status failure called start-setup or open');
}
pass('initial status failure then retry');

if (darkResume) {
  await flush(); await flush();
  if (!root().textContent.includes('finish setting up in your browser')) throw new Error('incomplete sign-in resume skipped browser setup');
  if (calls.some(c => c.cmd === 'open')) throw new Error('sign-in launch opened a browser before owner activation');
  window.journalApp.receive({ type: 'shown' });
  await flush(); await flush();
  if (!calls.some(c => c.cmd === 'open' && c.args?.target === 'init')) throw new Error('owner activation did not open pending browser setup');
  syncPlan.push({ fail: 'Run removal failed' });
  darkProbe = 'complete';
  for (const callback of intervals.values()) await callback();
  await flush(); await flush();
  if (!$('pane').textContent.includes('Run removal failed')) throw new Error('OFF registration failure was hidden');
  if (!calls.some(c => c.cmd === 'syncSignInLaunch' && c.args?.on === false) || calls.some(c => c.cmd === 'syncSignInLaunch' && c.args?.on === true)) throw new Error('resume changed saved OFF intent');
  click('try again');
  await flush(); await flush();
  if ($('pane').textContent.includes('Run removal failed')) throw new Error('registration retry did not clear recovered error');
  if (calls.some(c => c.cmd === 'flow')) throw new Error('dark resume admitted new flow');
  pass('dark sign-in waits for owner shown');
  pass('dark OFF registration error and retry');
  if (query(document.body, '.setup-band') || query(document.body, '.setup-progress')) {
    throw new Error('dark resume left setup band or progress');
  }
  console.log(`DOM CASES: ${passed} passed`);
  process.exit(0);
}

if (dark) {
  if (!root().textContent.includes("let's create your journal on this PC")) throw new Error('dark startup lost folder selection');
  if (calls.some(c => c.cmd === 'flow')) throw new Error('dark startup admitted new flow');
  click('continue');
  await flush(); await flush();
  if (!root().textContent.includes('finish setting up in your browser')) throw new Error('dark setup did not retain browser step');
  click('open the setup page');
  await flush();
  if (!calls.some(c => c.cmd === 'open' && c.args?.target === 'init')) throw new Error('browser step did not request init');
  darkProbe = 'complete';
  statusPlan.push({ fail: 'completion status unavailable' });
  for (const callback of intervals.values()) await callback();
  await flush();
  if (!visible(root()) || visible($('main'))) throw new Error('completion status failure left a false Home');
  pass('dark completion status failure retains retry');
  for (const callback of intervals.values()) await callback();
  await flush(); await flush();
  if (!visible($('main'))) throw new Error('completed browser setup did not reach Home');
  if (buttons().some(b => b.getAttribute('data-intent') === 'continue-solstone')) throw new Error('dark Home offered continuation');
  click('open your journal');
  await flush();
  if (!calls.some(c => c.cmd === 'open' && c.args?.target === 'journal')) throw new Error('dark Home lost ordinary journal open');
  if (calls.some(c => c.cmd === 'flow')) throw new Error('dark path admitted new flow');
  pass('dark browser setup recovers and retains ordinary Home open');
  if (query(document.body, '.setup-band') || query(document.body, '.setup-progress')) {
    throw new Error('dark setup left setup band or progress');
  }
  console.log(`DOM CASES: ${passed} passed`);
  process.exit(0);
}

const elAttr1 = document.createElement('button');
elAttr1.setAttribute('data-choice-id', 'local');
const elAttr2 = document.createElement('button');
elAttr2.setAttribute('data-choice-id', 'spp');
if (!matches(elAttr1, '[data-choice-id="local"]') || matches(elAttr2, '[data-choice-id="local"]')) {
  throw new Error('attribute matcher failed');
}
pass('compound matcher matches exact attribute');

const btnPrimary = document.createElement('button');
btnPrimary.className = 'primary';
const divPrimary = document.createElement('div');
divPrimary.className = 'primary';
const plainBtn = document.createElement('button');
if (!matches(btnPrimary, 'button.primary') || matches(divPrimary, 'button.primary') || matches(plainBtn, 'button.primary')) {
  throw new Error('tag class matcher failed');
}
pass('compound matcher matches tag and class');

const noticeThinking = document.createElement('div');
noticeThinking.className = 'notice';
noticeThinking.setAttribute('data-download', 'thinking');
const plainNotice = document.createElement('div');
plainNotice.className = 'notice';
if (!matches(noticeThinking, '.notice[data-download=thinking]') || matches(plainNotice, '.notice[data-download=thinking]')) {
  throw new Error('class attribute matcher failed');
}
pass('compound matcher matches class and attribute');

if (root().getAttribute('data-phase') !== 'welcome') {
  throw new Error(`boot did not paint welcome (${root().getAttribute('data-phase')}) ${root().textContent}`);
}

click('start setup');
if (inbox[0]?.cmd !== 'flow' || inbox[0]?.args?.intent !== 'acknowledge') {
  throw new Error(`expected acknowledge, got: ${JSON.stringify(inbox[0])}`);
}
answer(view({ phase: 'sign-in', step: 1 }));
await flush();
if (inbox[0]?.cmd !== 'flow' || inbox[0]?.args?.intent !== 'choose-sign-in' || inbox[0]?.args?.on !== true) {
  throw new Error(`expected choose-sign-in with on: true, got: ${JSON.stringify(inbox[0])}`);
}
answer(view({ phase: 'sign-in', step: 1 }));
await flush();
if (inbox[0]?.cmd !== 'flow' || inbox[0]?.args?.intent !== 'continue-sign-in') {
  throw new Error(`expected continue-sign-in, got: ${JSON.stringify(inbox[0])}`);
}
if (!$('sign-in-check')) throw new Error('sign-in paint missing #sign-in-check');
const primaryBtn = query(root(), 'button.primary');
if (!primaryBtn || primaryBtn.textContent.trim() !== 'start setup') throw new Error('sign-in paint missing start setup button');
answer(view({ phase: 'folder', step: 2 }));
if (root().getAttribute('data-phase') !== 'folder') throw new Error('sign-in continue did not paint folder');
pass('welcome and sign-in continue');

const folderInput = buttons().length >= 0 && [...root().childNodes].length
  ? query(root(), 'input')
  : null;
if (!folderInput || folderInput.tagName !== 'INPUT') throw new Error('folder input is missing');
folderInput.focus();
folderInput.value = 'typed-path';
paint(view({ phase: 'folder', step: 2 }));
if (document.activeElement !== folderInput) throw new Error('folder refresh moved focus');
if (folderInput.value !== 'typed-path') throw new Error('folder refresh cleared the typed value');
pass('heading focus and folder typing');

// Toggle cases:
// 1. Paint welcome, turn switch off, start setup posts acknowledge, choose-sign-in with on: false, continue-sign-in
paint(view({ phase: 'welcome', step: 1 }));
const switchBtn1 = $('sign-in-check');
if (!switchBtn1 || switchBtn1.getAttribute('aria-checked') !== 'true') throw new Error('switch should be on by default');
switchBtn1.click();
if ($('sign-in-check').getAttribute('aria-checked') !== 'false') throw new Error('switch did not toggle off');
click('start setup');
if (inbox[0]?.args?.intent !== 'acknowledge') throw new Error(`expected acknowledge, got ${JSON.stringify(inbox[0])}`);
answer(view({ phase: 'sign-in', step: 1 }));
await flush();
if (inbox[0]?.args?.intent !== 'choose-sign-in' || inbox[0]?.args?.on !== false) throw new Error(`expected choose-sign-in on: false, got ${JSON.stringify(inbox[0])}`);
answer(view({ phase: 'sign-in', step: 1 }));
await flush();
if (inbox[0]?.args?.intent !== 'continue-sign-in') throw new Error(`expected continue-sign-in, got ${JSON.stringify(inbox[0])}`);
answer(view({ phase: 'folder', step: 2 }));
await flush();
pass('sign-in toggle off posts on false');

// 2. Turn switch off, fail acknowledge reply, re-render leaves aria-checked false, next press posts on: false
paint(view({ phase: 'welcome', step: 1 }));
$('sign-in-check').click();
if ($('sign-in-check').getAttribute('aria-checked') !== 'false') throw new Error('switch did not toggle off');
click('start setup');
if (inbox[0]?.args?.intent !== 'acknowledge') throw new Error(`expected acknowledge, got ${JSON.stringify(inbox[0])}`);
fail('ack-fail');
await flush();
if ($('sign-in-check').getAttribute('aria-checked') !== 'false') throw new Error('switch lost false aria-checked after failed acknowledge');
click('start setup');
if (inbox[0]?.args?.intent !== 'acknowledge') throw new Error(`expected acknowledge on retry, got ${JSON.stringify(inbox[0])}`);
answer(view({ phase: 'sign-in', step: 1 }));
await flush();
if (inbox[0]?.args?.intent !== 'choose-sign-in' || inbox[0]?.args?.on !== false) throw new Error(`expected choose-sign-in on: false after retry, got ${JSON.stringify(inbox[0])}`);
answer(view({ phase: 'sign-in', step: 1 }));
await flush();
if (inbox[0]?.args?.intent !== 'continue-sign-in') throw new Error(`expected continue-sign-in, got ${JSON.stringify(inbox[0])}`);
answer(view({ phase: 'folder', step: 2 }));
await flush();
pass('sign-in failure retry retains toggle');

// 3. Turn switch off, same-phase paint with no error, next press still posts on: false
paint(view({ phase: 'welcome', step: 1 }));
$('sign-in-check').click();
paint(view({ phase: 'welcome', step: 1 }));
if ($('sign-in-check').getAttribute('aria-checked') !== 'false') throw new Error('same-phase repaint lost toggle state');
click('start setup');
if (inbox[0]?.args?.intent !== 'acknowledge') throw new Error(`expected acknowledge on repaint, got ${JSON.stringify(inbox[0])}`);
answer(view({ phase: 'sign-in', step: 1 }));
await flush();
if (inbox[0]?.args?.intent !== 'choose-sign-in' || inbox[0]?.args?.on !== false) throw new Error(`expected choose-sign-in on: false after repaint, got ${JSON.stringify(inbox[0])}`);
answer(view({ phase: 'sign-in', step: 1 }));
await flush();
if (inbox[0]?.args?.intent !== 'continue-sign-in') throw new Error(`expected continue-sign-in, got ${JSON.stringify(inbox[0])}`);
answer(view({ phase: 'folder', step: 2 }));
await flush();
pass('same-phase repaint retains sign-in toggle');

// 4. paint({ phase: 'sign-in' }) shows welcome screen: #sign-in-check button, welcome title h1, primary text start setup
paint(view({ phase: 'sign-in', step: 1 }));
if (!$('sign-in-check') || $('sign-in-check').tagName !== 'BUTTON') throw new Error('sign-in-check button missing on sign-in phase');
const welcomeH1 = query(root(), 'h1');
if (!welcomeH1 || welcomeH1.textContent !== 'Welcome') throw new Error(`expected welcome title in h1, got ${welcomeH1?.textContent}`);
const setupPrimary = query(root(), 'button.primary');
if (!setupPrimary || setupPrimary.textContent.trim() !== 'start setup') throw new Error('primary button text is not start setup');
pass('painted phase sign-in renders welcome screen');

// Return to folder for the subsequent continue click
paint(view({ phase: 'folder', step: 2 }));

click('continue');
answer(view({ phase: 'setup', busy: true, error: 'busy-err' }));
if (!root().textContent.includes('busy-err')) throw new Error('busy reply hid the error');
if (!buttons().some(button => button.disabled)) throw new Error('busy reply left every button enabled');
pass('busy reply shows the error');

click('try again');
fail('visible-err');
if (!root().textContent.includes('visible-err')) throw new Error('rejected reply hid the error');
pass('rejected reply is visible');

click('try again');
const phases = [];
answer(view({ phase: 'mark', step: 3, busy: true, error: 'mark-busy' }));
phases.push(root().getAttribute('data-phase'));
paint(view({ phase: 'thinking', step: 4 }));
phases.push(root().getAttribute('data-phase'));
if (phases[0] !== 'mark' || phases[1] !== 'thinking') {
  throw new Error(`reply did not paint before the later flow (${phases.join(' -> ')})`);
}
pass('reply paints before a later flow');

paint(view({
  phase: 'thinking',
  step: 4,
  thinking_saved: true,
  thinking_choice: 'local',
  local_offered: true,
}));
if (!buttons().some(button => accessibleName(button) === 'Later')) throw new Error('saved choice missing Later');
click('Later');
const later = inbox.shift();
if (later?.args?.choice !== 'later') throw new Error(`Later posted ${JSON.stringify(later?.args)}`);
pass('saved choice can change to later');

const downloading = view({
  phase: 'thinking',
  step: 4,
  thinking_cancel: true,
  downloads: {
    thinking: { state: 'downloading', attempt_id: 'painted-a', bytes_done: 12, bytes_total: 40 },
  },
});
paint(downloading);
if (!root().textContent.includes('ThinkDl') || !root().textContent.includes('12 bytes of 40 bytes')) {
  throw new Error(`download card hid its label or formatted bytes (${root().textContent})`);
}
downloading.downloads.thinking.attempt_id = 'newer-b';
click('Cancel');
const cancel = inbox.shift();
if (cancel?.args?.attempt_id !== 'painted-a') throw new Error(`cancel posted ${cancel?.args?.attempt_id}`);
pass('cancel posts the painted attempt');

paint(view({ phase: 'home', step: 5, error: 'home-err' }));
if ($('main').hidden) throw new Error('home left the main window hidden');
if (!$('pane').textContent.includes('home-err')) throw new Error('home hid the error');
pass('home shows its error');

click('start');
await flush();
click('devices');
await flush();
click('pair a device');
await flush();
const pairing = $('pane').querySelector('.pairing');
if (!pairing) throw new Error('pairing dialog did not open');
const devices = buttons().find(button => button.textContent.trim() === 'devices');
if (devices?.getAttribute('aria-selected') !== 'true') throw new Error('devices pane was not selected');
paint(view({ phase: 'home', step: 5 }));
if ($('pane').querySelector('.pairing') !== pairing) throw new Error('a later home update replaced the pairing dialog');
if (devices.getAttribute('aria-selected') !== 'true') throw new Error('a later home update left the devices page');
pass('later home leaves the open page alone');

click('close');
await flush();
click('home');
await flush();

// 1. unsaved capable thinking shows the local line and continue saves local
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_capability: 'ok',
  thinking_download_bytes: 1500000,
  default_line: 'measured-line',
  thinking_saved: false,
  thinking_choice: null,
}));
const localBtn = buttons().find(b => accessibleName(b) === 'on this PC');
if (!localBtn) throw new Error('local button not found with accessible name "on this PC"');
if (!localBtn.textContent.includes('1.5 MB')) throw new Error('local button missing 1.5 MB');
if (root().textContent.includes('measured-line')) throw new Error('measured-line visible');
if (root().textContent.includes('Local lane')) throw new Error('Local lane visible');
if (!buttons().some(b => accessibleName(b) === 'Later')) throw new Error('Later not visible');
if (buttons().some(b => accessibleName(b) === 'Change')) throw new Error('Change button visible');
if (!buttons().some(b => accessibleName(b) === 'Continue')) throw new Error('Continue button not visible');
if (inbox.length !== 0) throw new Error(`inbox not empty: ${JSON.stringify(inbox)}`);
click('Continue');
const chosen = inbox.shift();
if (chosen?.cmd !== 'flow' || chosen?.args?.intent !== 'continue-thinking' || chosen?.args?.choice !== 'local') {
  throw new Error(`expected continue-thinking local, got: ${JSON.stringify(chosen)}`);
}
pass('unsaved capable thinking shows the local line and continue saves local');

// 2. unsaved capable thinking lists every choice
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_capability: 'ok',
  thinking_download_bytes: 1500000,
  thinking_saved: false,
  thinking_choice: null,
}));
const choiceBtns = root().querySelectorAll('button.setup-choice');
const choiceIds = choiceBtns.map(b => b.getAttribute('data-choice-id'));
if (!choiceIds.includes('local') || !choiceIds.includes('spp') || !choiceIds.includes('byo')) {
  throw new Error(`expected local, spp, byo choices, got ${choiceIds.join(', ')}`);
}
if (!root().textContent.includes('model requests stay on this PC.')) throw new Error('the local row does not carry its this-PC description');
if (root().textContent.includes('local description')) throw new Error('the local row shows the shared description');
if (!root().textContent.includes('spp description')) throw new Error('spp description hidden before it is chosen');
if (!root().textContent.includes('byo description')) throw new Error('byo description hidden before it is chosen');
if (buttons().some(b => accessibleName(b) === 'Change')) throw new Error('Change button visible');
pass('unsaved capable thinking lists every choice');

// 3. unsaved blocked shows choose later
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_capability: 'blocked',
  default_line: 'measured-line',
  thinking_saved: false,
  thinking_choice: null,
}));
if (!root().textContent.includes('Choose later')) throw new Error('choose_later text not visible');
if (root().textContent.includes('measured-line')) throw new Error('default_line visible');
if (buttons().some(b => accessibleName(b) === 'Continue')) throw new Error('Continue button visible');
if (buttons().some(b => accessibleName(b) === 'on this PC') || root().textContent.includes('Local lane')) throw new Error('local row visible');
if (!buttons().some(b => accessibleName(b) === 'Spp lane')) throw new Error('Spp lane not visible');
if (!buttons().some(b => accessibleName(b) === 'Byo lane')) throw new Error('Byo lane not visible');
if (!buttons().some(b => accessibleName(b) === 'Later')) throw new Error('Later not visible');
click('Later');
const laterMsg = inbox.shift();
if (laterMsg?.cmd !== 'flow' || laterMsg?.args?.intent !== 'continue-thinking' || laterMsg?.args?.choice !== 'later') {
  throw new Error(`expected continue-thinking later, got: ${JSON.stringify(laterMsg)}`);
}
pass('unsaved blocked shows choose later');

// 4. unsaved unknown shows choose later
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_capability: 'unknown',
  default_line: 'measured-line',
  thinking_saved: false,
  thinking_choice: null,
}));
if (!root().textContent.includes('Choose later')) throw new Error('choose_later text not visible');
if (root().textContent.includes('measured-line')) throw new Error('default_line visible');
if (buttons().some(b => accessibleName(b) === 'Continue')) throw new Error('Continue button visible');
if (buttons().some(b => accessibleName(b) === 'on this PC') || root().textContent.includes('Local lane')) throw new Error('local row visible');
if (!buttons().some(b => accessibleName(b) === 'Spp lane')) throw new Error('Spp lane not visible');
if (!buttons().some(b => accessibleName(b) === 'Byo lane')) throw new Error('Byo lane not visible');
if (!buttons().some(b => accessibleName(b) === 'Later')) throw new Error('Later not visible');
paint(view({
  phase: 'thinking',
  step: 4,
  default_line: 'measured-line',
  thinking_saved: false,
  thinking_choice: null,
}));
if (buttons().some(b => accessibleName(b) === 'on this PC') || root().textContent.includes('Local lane')) {
  throw new Error('missing thinking_capability admitted local row');
}
pass('unsaved unknown shows choose later');

// 5. saved local spp byo and later name themselves
const savedCases = [
  { id: 'local', label: 'on this PC' },
  { id: 'spp', label: 'Spp lane' },
  { id: 'byo', label: 'Byo lane' },
  { id: 'later', label: 'Later' },
];
for (const sc of savedCases) {
  paint(view({
    phase: 'thinking',
    step: 4,
    thinking_saved: true,
    thinking_choice: sc.id,
  }));
  if (sc.id === 'local') {
    if (!buttons().some(b => accessibleName(b) === 'on this PC')) throw new Error('local visible name is not on this PC');
    if (root().textContent.includes('Local lane')) throw new Error('Local lane is visible');
  } else if (sc.id === 'spp' || sc.id === 'byo') {
    if (!buttons().some(b => accessibleName(b) === sc.label)) throw new Error(`${sc.id} label not visible`);
  }
  if (!buttons().some(b => accessibleName(b) === 'Later')) {
    throw new Error(`${sc.id} missing Later button without Change click`);
  }
}
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_saved: true,
  thinking_choice: 'local',
  thinking_capability: 'blocked',
}));
const localSavedBlocked = buttons().find(b => b.getAttribute('data-choice-id') === 'local');
if (!localSavedBlocked || localSavedBlocked.getAttribute('aria-checked') !== 'true') {
  throw new Error('saved local with blocked capability did not retain aria-checked=true');
}
pass('saved local spp byo and later name themselves');

// 6. proposed null does not commit
paint(view({
  phase: 'mark',
  step: 3,
  mark: { proposal_id: 'prop-1', render: null, state: 'proposed' },
}));
click('Keep');
if (inbox.some(m => m.args?.intent === 'commit-mark')) throw new Error('null render committed');
pass('proposed null does not commit');

// 7. empty mark does not commit
paint(view({
  phase: 'mark',
  step: 3,
  mark: { proposal_id: 'prop-1', render: {}, state: 'proposed' },
}));
click('Keep');
if (inbox.some(m => m.args?.intent === 'commit-mark')) throw new Error('empty mark committed');
pass('empty mark does not commit');

// 8. malformed mark does not commit
const badMarks = [
  { words: ['only-one'], icon1: { svg: '<path d="M1 1L5 5"/>', color: { name: 'c', hex: '#112233' }, rot: 0 }, icon2: { svg: '<circle cx="1" cy="1" r="1"/>', color: { name: 'c', hex: '#445566' }, rot: 0 } },
  { words: ['bad', 'hex'], icon1: { svg: '<path d="M1 1L5 5"/>', color: { name: 'c', hex: 'not-hex' }, rot: 0 }, icon2: { svg: '<circle cx="1" cy="1" r="1"/>', color: { name: 'c', hex: '#445566' }, rot: 0 } },
  { words: ['empty', 'color'], icon1: { svg: '<path d="M1 1L5 5"/>', color: { name: '', hex: '#112233' }, rot: 0 }, icon2: { svg: '<circle cx="1" cy="1" r="1"/>', color: { name: 'c', hex: '#445566' }, rot: 0 } },
];
for (const bm of badMarks) {
  paint(view({
    phase: 'mark',
    step: 3,
    mark: { proposal_id: 'prop-1', render: bm, state: 'proposed' },
  }));
  click('Keep');
  if (inbox.some(m => m.args?.intent === 'commit-mark')) throw new Error('bad mark committed');
}
pass('malformed mark does not commit');

// 9. literal mark shows both glyphs and both words and commits the painted id
const validRender = {
  words: ['oak', 'river'],
  icon1: { svg: '<path d="M1 1L5 5"/>', color: { name: 'c1', hex: '#112233' }, rot: 0 },
  icon2: { svg: '<circle cx="1" cy="1" r="1"/>', color: { name: 'c2', hex: '#445566' }, rot: 45 },
};
paint(view({
  phase: 'mark',
  step: 3,
  mark: { proposal_id: 'prop-painted-id', render: validRender, state: 'proposed' },
}));
if (!root().textContent.includes('oak') || !root().textContent.includes('river')) {
  throw new Error('words oak and river not visible');
}
if (!query(root(), 'path') || !query(root(), 'circle')) {
  throw new Error('path and circle not in card');
}
click('Keep');
const commitMsg = inbox.shift();
if (commitMsg?.cmd !== 'flow' || commitMsg?.args?.intent !== 'commit-mark' || commitMsg?.args?.proposal_id !== 'prop-painted-id') {
  throw new Error(`commit-mark did not post painted id: ${JSON.stringify(commitMsg)}`);
}
pass('literal mark shows both glyphs and both words and commits the painted id');

// 10. mark whose glyphs strip does not commit
const stripRender = {
  words: ['oak', 'river'],
  icon1: { svg: '<g></g>', color: { name: 'c1', hex: '#112233' }, rot: 0 },
  icon2: { svg: '<script></script>', color: { name: 'c2', hex: '#445566' }, rot: 45 },
};
paint(view({
  phase: 'mark',
  step: 3,
  mark: { proposal_id: 'prop-strip', render: stripRender, state: 'proposed' },
}));
click('Keep');
if (inbox.some(m => m.args?.intent === 'commit-mark')) throw new Error('stripped mark committed');
pass('mark whose glyphs strip does not commit');
for (const svg of ['<path/>', '<path d=""/>', '<path d="M0 0"/>', '<circle r="0"/>']) {
  paint(view({ phase: 'mark', step: 3,
    mark: { proposal_id: 'empty-glyph', state: 'proposed', render: {
      ...validRender, icon1: { ...validRender.icon1, svg },
    } },
  }));
  click('Keep');
  if (inbox.some(m => m.args?.intent === 'commit-mark')) throw new Error(`empty geometry committed: ${svg}`);
}
pass('empty geometry never enables commit');
const catalog = JSON.parse(readFileSync(join(here, '../../solstone-core-sol-link/assets/mark_assets/glyphs.json'), 'utf8'));
const markModule = await import('../ui/mark.js');
if (!catalog.drum) throw new Error('catalog control missing drum');
// A browser measures a path in an unrendered parsed document, and throws for
// every other shape.
DomElement.prototype.getTotalLength = function getTotalLength() {
  if (this.localName !== 'path') throw new Error('InvalidStateError');
  return this.getAttribute('d') ? 1 : 0;
};
for (const [name, svg] of Object.entries(catalog)) {
  const render = { ...validRender,
    icon1: { ...validRender.icon1, svg }, icon2: { ...validRender.icon2, svg },
  };
  if (!markModule.displayableMark(render)) throw new Error(`catalog glyph rejected: ${name}`);
  const card = document.createElement('div');
  markModule.renderMarkCard(card, render);
  const expected = new DOMParser().parseFromString(`<svg>${svg}</svg>`, 'image/svg+xml').documentElement.children;
  for (const tag of ['path', 'circle', 'rect', 'line', 'polyline', 'polygon', 'ellipse']) {
    const originals = [...expected].filter(shape => shape.localName === tag);
    const copies = [...card.querySelectorAll(tag)];
    if (copies.length !== originals.length * 2) throw new Error(`catalog shapes changed: ${name}/${tag}`);
    if (tag === 'path' && copies.some((shape, index) => shape.getAttribute('d') !== originals[index % originals.length].getAttribute('d'))) {
      throw new Error(`catalog path changed: ${name}`);
    }
  }
}
delete DomElement.prototype.getTotalLength;
pass('unchanged canonical catalog keeps every drawing shape and path');

paint(view({ phase: 'mark', step: 3, busy: true,
  mark: { proposal_id: null, state: 'unavailable', render: null },
}));
if (!query(root(), 'progress')) throw new Error('first mark creation has no loading indicator');
if (root().textContent.includes('mark-err')) throw new Error('first mark creation shows the expected absence as an error');
pass('first mark creation stays loading');


// 11. committed identity sets the icon
const identityMark = {
  words: ['mountain', 'sky'],
  icon1: { svg: '<path d="M 1 1 L 8 8"/>', color: { name: 'blue', hex: '#112233' }, rot: 0 },
  icon2: { svg: '<circle cx="1" cy="1" r="1"/>', color: { name: 'teal', hex: '#445566' }, rot: 45 },
};
identityPlan.push({
  committed: true,
  instance_id: 'inst-1',
  mark: identityMark,
});
statusPlan.push(statusBody('running', true));
paint(view({ phase: 'home', step: 5 }));
window.journalApp.receive({ type: 'shown' });
await flush();
if (!setIconCalls.some(c => JSON.stringify(c.mark) === JSON.stringify(identityMark))) {
  throw new Error('setIcon not called with identityMark');
}
if (!$('pane').textContent.includes('mountain') || !$('pane').textContent.includes('sky')) {
  throw new Error('identity words not in card');
}
if (!query($('pane'), '.mark-confirmed')) {
  throw new Error('.mark-confirmed node missing');
}
pass('committed identity sets the icon');

// 12. neutral identity does not confirm the cached mark
const setIconCountBefore = setIconCalls.length;
identityPlan.push({ committed: false, instance_id: null, mark: null });
statusPlan.push(statusBody('running', true));
window.journalApp.receive({ type: 'shown' });
await flush();
if (setIconCalls.length !== setIconCountBefore) throw new Error('new setIcon called on neutral identity');
if ($('pane').textContent.includes('cached') || $('pane').textContent.includes('seed')) {
  throw new Error('cached words shown on neutral identity');
}
if (query($('pane'), '.mark-confirmed')) throw new Error('.mark-confirmed shown on neutral identity');
pass('neutral identity does not confirm the cached mark');

// 13. a committed mark that can't be drawn, with none saved, shows unavailable
identityPlan.push({ committed: true, instance_id: 'inst-2', mark: null });
statusPlan.push(statusBody('running', true));
window.journalApp.receive({ type: 'shown' });
await flush();
if (setIconCalls.length !== setIconCountBefore) throw new Error('new setIcon called on malformed identity');
if ($('pane').textContent.includes('cached') || $('pane').textContent.includes('seed')) {
  throw new Error('cached words shown on malformed identity');
}
if (query($('pane'), '.mark-confirmed')) throw new Error('.mark-confirmed shown on malformed identity');
if (!query($('pane'), '.mark-chip.unavailable')) throw new Error('malformed identity with no saved mark did not show unavailable');
pass('malformed identity with no saved mark shows unavailable');

// 14. a failed read, with no mark saved, shows unavailable
identityPlan.push({ fail: 'convey failed' });
statusPlan.push(statusBody('running', true));
window.journalApp.receive({ type: 'shown' });
await flush();
if (setIconCalls.length !== setIconCountBefore) throw new Error('new setIcon called on failed identity');
if ($('pane').textContent.includes('cached') || $('pane').textContent.includes('seed')) {
  throw new Error('cached words shown on failed identity');
}
if (query($('pane'), '.mark-confirmed')) throw new Error('.mark-confirmed shown on failed identity');
if (!query($('pane'), '.mark-chip.unavailable')) throw new Error('failed identity with no saved mark did not show unavailable');
if (query($('pane'), '.mark-chip.generic')) throw new Error('failed identity painted the no-identity-yet mark');
pass('failed identity with no saved mark shows unavailable');

// 14a. a failed or undrawable read keeps a saved mark
identityPlan.push({ committed: true, instance_id: 'inst-1', mark: identityMark });
statusPlan.push(statusBody('running', true));
window.journalApp.receive({ type: 'shown' });
await flush();
for (const [name, read] of [
  ['failed', { fail: 'convey failed' }],
  ['undrawable', { committed: true, instance_id: 'inst-1', mark: null }],
]) {
  identityPlan.push(read);
  statusPlan.push(statusBody('running', true));
  window.journalApp.receive({ type: 'shown' });
  await flush();
  if (!$('pane').textContent.includes('mountain')) throw new Error(`${name} read dropped the saved mark`);
  if (query($('pane'), '.mark-chip.unavailable')) throw new Error(`${name} read marked a saved mark unavailable`);
}
pass('failed or undrawable read keeps a saved mark');

// 14b. no identity at all still shows the no-identity-yet mark
identityPlan.push({ committed: false, instance_id: null, mark: null });
statusPlan.push(statusBody('running', true));
window.journalApp.receive({ type: 'shown' });
await flush();
if (!query($('pane'), '.mark-chip.generic')) throw new Error('no identity did not show the generic mark');
pass('no identity still shows the generic mark');

// 15. thinking focus survives a progress repaint
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_capability: 'ok',
  thinking_download_bytes: 1000000,
  thinking_saved: false,
  thinking_choice: null,
}));
const sppBtn = buttons().find(b => b.getAttribute('data-choice-id') === 'spp');
if (!sppBtn) throw new Error('spp button not found');
sppBtn.focus();
pressEnter();
const chooseSpp = inbox.shift();
if (chooseSpp?.args?.intent !== 'choose-thinking' || chooseSpp?.args?.choice !== 'spp') {
  throw new Error(`expected choose-thinking spp, got ${JSON.stringify(chooseSpp)}`);
}
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_capability: 'ok',
  thinking_download_bytes: 1000000,
  thinking_saved: true,
  thinking_choice: 'spp',
}));
const sppAfter = buttons().find(b => b.getAttribute('data-choice-id') === 'spp');
if (document.activeElement !== sppAfter || sppAfter.getAttribute('aria-checked') !== 'true') {
  throw new Error(`expected focus on spp row with aria-checked true, active is ${document.activeElement?.getAttribute?.('data-focus-key')}`);
}
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_capability: 'ok',
  thinking_download_bytes: 2000000,
  thinking_saved: true,
  thinking_choice: 'spp',
}));
const sppAfterDlChange = buttons().find(b => b.getAttribute('data-choice-id') === 'spp');
if (document.activeElement !== sppAfterDlChange) {
  throw new Error(`focus lost on spp row after thinking_download_bytes change`);
}
pass('thinking focus survives a progress repaint');

// 16. home focus survives a background repaint
paint(view({ phase: 'home', step: 5 }));
statusPlan.push(statusBody('not-running', false));
window.journalApp.receive({ type: 'shown' });
await flush();
const startBtn = buttons().find(b => b.textContent.trim() === 'start');
if (!startBtn) throw new Error('start button not found');
startBtn.focus();
paint(view({ phase: 'home', step: 5 }));
if (document.activeElement?.textContent.trim() !== 'start') {
  throw new Error(`focus lost on home repaint: active is ${document.activeElement?.tagName}`);
}
pass('home focus survives a background repaint');

// 17. a step change focuses the new heading
paint(view({ phase: 'mark', step: 3 }));
if (document.activeElement?.tagName !== 'H1') {
  throw new Error(`phase change did not focus H1, focused: ${document.activeElement?.tagName}`);
}
pass('a step change focuses the new heading');

// 18. a removed thinking control is not focused
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_cancel: true,
  downloads: { thinking: { state: 'downloading', attempt_id: 'att-1', bytes_done: 5, bytes_total: 50 } },
}));
const cancelBtn = buttons().find(b => b.textContent.trim() === 'Cancel');
if (!cancelBtn) throw new Error('Cancel button not found');
cancelBtn.focus();
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_cancel: false,
  downloads: {},
}));
if (document.activeElement === cancelBtn || document.activeElement?.tagName === 'H1') {
  throw new Error(`removed control focused or stole heading: ${document.activeElement?.tagName}`);
}
pass('a removed thinking control is not focused');

// 19. mark thinking and home show retry when authority is clear
flowPlan.push({ manual: true });
paint(view({
  phase: 'mark',
  step: 3,
  authoritative: false,
  mark: { proposal_id: 'prop-auth', render: validRender, state: 'proposed' },
}));
if (!buttons().some(b => b.textContent.trim() === 'try again')) throw new Error('mark missing try again on unauthoritative');
click('Keep');
if (inbox.some(m => m.args?.intent === 'commit-mark')) throw new Error('unauthoritative mark committed');
click('try again');
const retryMarkMsg = inbox.shift();
if (retryMarkMsg?.cmd !== 'flow' || retryMarkMsg?.args?.intent !== 'retry-state') {
  throw new Error(`try again did not post retry-state: ${JSON.stringify(retryMarkMsg)}`);
}

flowPlan.push({ manual: true });
paint(view({
  phase: 'thinking',
  step: 4,
  authoritative: false,
  thinking_saved: false,
  thinking_capability: 'ok',
  local_offered: true,
  default_line: 'line',
}));
if (!buttons().some(b => b.textContent.trim() === 'try again')) throw new Error('thinking missing try again on unauthoritative');
click('Continue');
if (inbox.some(m => ['choose-thinking', 'continue-thinking'].includes(m.args?.intent))) throw new Error('unauthoritative thinking action posted');
click('try again');
const retryThinkMsg = inbox.shift();
if (retryThinkMsg?.cmd !== 'flow' || retryThinkMsg?.args?.intent !== 'retry-state') {
  throw new Error(`try again did not post retry-state: ${JSON.stringify(retryThinkMsg)}`);
}

flowPlan.push({ manual: true });
paint(view({
  phase: 'home',
  step: 5,
  authoritative: false,
}));
if (!buttons().some(b => b.textContent.trim() === 'try again')) throw new Error('home missing try again on unauthoritative');
click('try again');
const retryHomeMsg = inbox.shift();
if (retryHomeMsg?.cmd !== 'flow' || retryHomeMsg?.args?.intent !== 'retry-state') {
  throw new Error(`try again did not post retry-state: ${JSON.stringify(retryHomeMsg)}`);
}
pass('mark thinking and home show retry when authority is clear');

// 20. stopped registration can start
const callsCountBefore20 = calls.length;
statusPlan.push(statusBody('stopped', true));
window.journalApp.receive({ type: 'shown' });
await flush();
paint(view({ phase: 'opening' }));
if ($('main').hidden) throw new Error('main is hidden');
if (!buttons().some(b => b.textContent.trim() === 'start')) throw new Error('start button not visible');
if (root().getAttribute('data-phase') === 'home') throw new Error('phase is home');
const newCalls20 = calls.slice(callsCountBefore20);
if (newCalls20.some(c => c.cmd === 'open' || (c.cmd === 'flow' && c.args?.intent === 'start-setup'))) {
  throw new Error('called open or start-setup');
}
pass('stopped registration can start');

// 21. failed start is visible
startPlan.push({ fail: 'start daemon failed' });
click('start');
await flush();
if (!$('pane').textContent.includes('start daemon failed')) throw new Error('start error not visible in pane');
if (inbox.some(m => m.args?.intent === 'retry-state')) throw new Error('failed start posted retry-state');
pass('failed start is visible');

// 22. started registration reconciles to the incomplete step
flowPlan.push({ manual: true });
startPlan.push(statusBody('running', true));
click('start');
await flush();
const startMsg = inbox.shift();
if (startMsg?.cmd !== 'flow' || startMsg?.args?.intent !== 'retry-state') {
  throw new Error(`expected retry-state after start, got: ${JSON.stringify(startMsg)}`);
}
send(startMsg.id, true, { value: view({ phase: 'mark', step: 3 }) });
await flush();
if (!visible($('firstrun')) || root().getAttribute('data-phase') !== 'mark') {
  throw new Error('firstrun did not show mark');
}
if (calls.some(c => c.cmd === 'open')) throw new Error('open called');
pass('started registration reconciles to the incomplete step');

// 23. started registration reconciles to home
const callsCountBefore23 = calls.length;
statusPlan.push(statusBody('stopped', true));
$('main').hidden = false;
window.journalApp.receive({ type: 'shown' });
await flush();
paint(view({ phase: 'opening' }));
flowPlan.push({ manual: true });
startPlan.push(statusBody('running', true));
click('start');
await flush();
const startHomeMsg = inbox.shift();
if (startHomeMsg?.cmd !== 'flow' || startHomeMsg?.args?.intent !== 'retry-state') {
  throw new Error(`expected retry-state after start, got: ${JSON.stringify(startHomeMsg)}`);
}
send(startHomeMsg.id, true, { value: view({ phase: 'home', step: 5 }) });
await flush();
if ($('main').hidden) throw new Error('main not shown after reconciling to home');
const newCalls23 = calls.slice(callsCountBefore23);
if (newCalls23.some(c => c.cmd === 'open' || (c.cmd === 'flow' && c.args?.intent === 'start-setup'))) {
  throw new Error('called open or start-setup');
}
pass('started registration reconciles to home');

// 24. fresh setup refreshes status before the home action
paint(view({ phase: 'setup', setup_running: true }));
statusPlan.push({
  service: { installed: true, port: 5016, starts_at_sign_in: false, journal: 'C:\\journal', running: true },
  set_up: true,
  answering: true,
  display: 'running',
  about: 'about',
  base: 'http://127.0.0.1:5016',
});
paint(view({ phase: 'home', step: 5 }));
await flush();
const journalAction = buttons().find(b => b.getAttribute('data-intent') === 'open-journal');
if (!journalAction) {
  throw new Error('fresh status did not expose the existing journal-open action');
}
pass('fresh setup refreshes status before the home action');

// 25. registered sign-in readback is not the first status
const lastSyncCall = [...calls].reverse().find(c => c.cmd === 'syncSignInLaunch');
if (!lastSyncCall || lastSyncCall.args?.on !== false) {
  throw new Error(`expected syncSignInLaunch with on: false, got: ${JSON.stringify(lastSyncCall)}`);
}
pass('registered sign-in readback is not the first status');

// 26. run entry failure is visible and retry only updates the app run entry
syncPlan.push({ fail: 'sync run entry failed' });
paint(view({ phase: 'setup', setup_running: true }));
statusPlan.push({
  service: { installed: true, port: 5016, starts_at_sign_in: true, journal: 'C:\\journal', running: true },
  set_up: true,
  answering: true,
  display: 'running',
  about: 'about',
  base: 'http://127.0.0.1:5016',
});
paint(view({ phase: 'home', step: 5 }));
await flush();
if (!$('pane').textContent.includes('sync run entry failed')) {
  throw new Error('sync failure not visible in pane');
}
const callsBeforeRetry = calls.length;
statusPlan.push(statusBody('running', true));
click('try again');
await flush();
const retryCalls = calls.slice(callsBeforeRetry);
if (retryCalls.length < 2 || retryCalls[0].cmd !== 'status' || retryCalls[1].cmd !== 'syncSignInLaunch') {
  throw new Error(`retry calls expected status then syncSignInLaunch, got: ${JSON.stringify(retryCalls)}`);
}
if (retryCalls.some(c => ['signIn', 'start-setup', 'open'].includes(c.cmd))) {
  throw new Error('prohibited command in retry calls');
}
pass('run entry failure is visible and retry only updates the app run entry');

// 27. status failure after setup retries without setup
const callsBefore27 = calls.length;
paint(view({ phase: 'setup', setup_running: true }));
statusPlan.push({ fail: 'status read failed after setup' });
paint(view({ phase: 'home', step: 5 }));
await flush();
if (!visible($('firstrun')) || !$('firstrun').textContent.includes('status read failed after setup')) {
  throw new Error('error not shown in firstrun on post-setup status failure');
}
if (visible($('main'))) {
  throw new Error('home shown on stale display');
}
statusPlan.push(statusBody('running', true));
click('try again');
await flush();
if ($('main').hidden) throw new Error('main not shown after successful status retry');
const calls27 = calls.slice(callsBefore27);
if (calls27.some(c => c.cmd === 'open' || (c.cmd === 'flow' && c.args?.intent === 'start-setup'))) {
  throw new Error('called open or start-setup');
}
pass('status failure after setup retries without setup');

// 28. continue in solstone app and download offer
paint(view({ phase: 'home', step: 5, authoritative: true }));
await flush();
if (calls.some(c => c.cmd === 'flow' && c.args?.intent === 'continue-solstone')) {
  throw new Error('found prior continue-solstone call');
}
const homeButtons = document.querySelectorAll('button');
const continueBtn = homeButtons.find(b => b.getAttribute('data-intent') === 'continue-solstone');
if (!continueBtn) throw new Error('continue-solstone button not found');
const callsBeforeContinue = calls.length;
continueBtn.click();
await flush();
const continueCalls = calls.slice(callsBeforeContinue);
const continueCall = continueCalls.find(c => c.cmd === 'flow' && c.args?.intent === 'continue-solstone');
if (!continueCall) {
  throw new Error(`expected continue-solstone call, got: ${JSON.stringify(continueCalls)}`);
}

for (const admission of [{authoritative:false}, {authoritative:true, busy:true}]) {
  paint(view({phase:'home', step:5, ...admission}));
  await flush();
  const unavailable = document.querySelectorAll('button').find(b => b.getAttribute('data-intent') === 'continue-solstone');
  if (!unavailable?.disabled) throw new Error('continuation available without current idle authority');
}

const callsBeforePaint = calls.length;
paint(view({ phase: 'home', step: 5 }));
await flush();
const paintCalls = calls.slice(callsBeforePaint);
if (paintCalls.some(c => c.cmd === 'open')) {
  throw new Error(`paint triggered open call: ${JSON.stringify(paintCalls)}`);
}

paint(view({ phase: 'home', step: 5, solstone_download: true }));
await flush();
const dlButtons = document.querySelectorAll('button');
const downloadBtn = dlButtons.find(b => b.getAttribute('data-intent') === 'download-solstone');
if (!downloadBtn) throw new Error('download-solstone button not found');
const callsBeforeDl = calls.length;
downloadBtn.click();
await flush();
const dlCalls = calls.slice(callsBeforeDl);
const dlCall = dlCalls.find(c => c.cmd === 'flow' && c.args?.intent === 'download-solstone');
if (!dlCall) {
  throw new Error(`expected download-solstone call, got: ${JSON.stringify(dlCalls)}`);
}
if (dlCall.args?.url != null || dlCall.args?.path != null || dlCall.args?.exe != null) {
  throw new Error(`download call contained prohibited args: ${JSON.stringify(dlCall.args)}`);
}

click('devices');
await flush();
const deviceButtons = document.querySelectorAll('button');
const thisPcBtn = deviceButtons.find(b => b.getAttribute('data-pairing') === 'this-pc');
if (!thisPcBtn) throw new Error('this-pc button not found');
const callsBeforeThisPc = calls.length;
thisPcBtn.click();
await flush();
const thisPcCalls = calls.slice(callsBeforeThisPc);
const pairStartCall = thisPcCalls.find(c => c.cmd === 'convey' && c.args?.path === '/app/network/pair-start');
if (!pairStartCall || pairStartCall.args?.body?.same_machine !== true) {
  throw new Error(`expected convey pair-start with same_machine: true, got: ${JSON.stringify(thisPcCalls)}`);
}
if (thisPcCalls.some(c => c.cmd === 'flow' && c.args?.intent === 'continue-solstone')) {
  throw new Error('this-pc click added continue-solstone call');
}
pass('continue in solstone app and download offer');

// A used link keeps the journal's mark up while the device asks, and offers to unpair it.
pairingStatusAnswer = { present: true, used: true, device: { cid: 'sha256:new-device', display_label: 'studio' } };
const pairingTick = [...intervals.values()].at(-1);
await pairingTick();
await pairingTick();
await flush();
const pairedPane = $('pane').querySelector('.pairing');
if (!pairedPane) throw new Error('the pairing view closed when a device arrived');
if (!pairedPane.querySelector('.mark-card') && !pairedPane.querySelector('[class*="mark"]')) {
  throw new Error('the mark left the pairing view when a device arrived');
}
if (!pairedPane.querySelector('[data-pairing="paired-note"]')) throw new Error('no note about answering on the device');
const unpairBtn = pairedPane.querySelector('[data-pairing="unpair"]');
if (!unpairBtn) throw new Error('no way to unpair the device that just arrived');
const callsBeforeUnpair = calls.length;
unpairBtn.click();
await flush();
await flush();
const unpairCall = calls.slice(callsBeforeUnpair).find(c => c.cmd === 'convey' && c.args?.path === '/app/network/unpair');
if (unpairCall?.args?.body?.fingerprint !== 'sha256:new-device') {
  throw new Error(`expected unpair of the new device, got: ${JSON.stringify(unpairCall)}`);
}
const unpairedPane = $('pane').querySelector('.pairing');
if (!unpairedPane?.querySelector('[data-pairing="unpaired"]')) throw new Error('unpairing did not say so');
if (unpairedPane.querySelector('[data-pairing="paired-note"]')) throw new Error('the note outlived the pairing');
if (unpairedPane.querySelector('[data-pairing="unpair"]')) throw new Error('unpair stayed offered after it worked');
click('done');
await flush();
if ($('pane').querySelector('.pairing')) throw new Error('done left the pairing view open');
pairingStatusAnswer = { used: false };
pass('a used link keeps the mark up and offers to unpair');

inbox.length = 0;

// 29. download progress formats bytes in megabytes
paint(view({
  phase: 'thinking',
  step: 4,
  downloads: {
    thinking: { state: 'downloading', attempt_id: 'att-bytes', bytes_done: 1500000, bytes_total: 40000000 },
  },
}));
if (!root().textContent.includes('1.5 MB of 40 MB')) {
  throw new Error(`downloading bytes formatted incorrectly: ${root().textContent}`);
}
if (root().textContent.includes('1500000')) {
  throw new Error('raw byte number 1500000 should not be visible');
}
pass('download progress formats bytes in megabytes');

// 30. cancel focus survives progress repaints
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_cancel: true,
  downloads: {
    thinking: { state: 'downloading', attempt_id: 'att-cancel', bytes_done: 10, bytes_total: 100 },
  },
}));
const cancelBtnF = root().querySelector('button[data-focus-key="thinking:cancel"]');
if (!cancelBtnF) throw new Error('cancel button not found');
cancelBtnF.focus();
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_cancel: true,
  downloads: {
    thinking: { state: 'downloading', attempt_id: 'att-cancel', bytes_done: 20, bytes_total: 100 },
  },
}));
if (document.activeElement?.getAttribute?.('data-focus-key') !== 'thinking:cancel') {
  throw new Error('focus lost on first cancel repaint');
}
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_cancel: true,
  downloads: {
    thinking: { state: 'downloading', attempt_id: 'att-cancel', bytes_done: 30, bytes_total: 100 },
  },
}));
if (document.activeElement?.getAttribute?.('data-focus-key') !== 'thinking:cancel') {
  throw new Error('focus lost on second cancel repaint');
}
pass('cancel focus survives progress repaints');

// 31. retry focus distinguishes thinking from speech
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_retry: true,
  speech_retry: true,
  downloads: {
    speech: { state: 'error' },
    thinking: { state: 'error' },
  },
}));
const retryButtons = root().querySelectorAll('button').filter(b => b.textContent.trim() === 'Retry');
if (retryButtons.length < 2) throw new Error(`expected at least 2 retry buttons, found ${retryButtons.length}`);
const thinkRetry = root().querySelector('button[data-focus-key="thinking:retry"]');
if (!thinkRetry) throw new Error('thinking:retry button not found');
thinkRetry.focus();
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_retry: true,
  speech_retry: true,
  downloads: {
    speech: { state: 'error' },
    thinking: { state: 'error' },
  },
}));
if (document.activeElement?.getAttribute?.('data-focus-key') !== 'thinking:retry') {
  throw new Error(`expected thinking:retry focused, active is ${document.activeElement?.getAttribute?.('data-focus-key')}`);
}
pass('retry focus distinguishes thinking from speech');

// 32. indeterminate download shows progress and cancel
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_cancel: true,
  downloads: {
    thinking: { state: 'downloading', attempt_id: 'att-indet', bytes_done: 10 },
  },
}));
const dlRow = root().querySelector('.setup-download[data-download="thinking"]');
if (!dlRow) throw new Error('thinking download row missing');
const bar = dlRow.querySelector('.setup-download-bar');
if (!bar) throw new Error('download bar missing');
const rowCancel = dlRow.querySelector('button[data-focus-key="thinking:cancel"]');
if (!rowCancel) throw new Error('cancel button missing on indeterminate download');
pass('indeterminate download shows progress and cancel');

// 33. idle thinking with retry flag offers download retry
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_saved: true,
  thinking_choice: 'local',
  thinking_retry: true,
  downloads: {
    thinking: { state: 'idle' },
  },
}));
const idleRetry = root().querySelector('button[data-focus-key="thinking:retry"]');
if (!idleRetry) throw new Error('thinking retry missing on idle retryable download');
idleRetry.click();
const startDlMsg = inbox.shift();
if (startDlMsg?.cmd !== 'flow' || startDlMsg?.args?.intent !== 'start-thinking-download') {
  throw new Error(`expected start-thinking-download, got ${JSON.stringify(startDlMsg)}`);
}
pass('idle thinking with retry flag offers download retry');

// 34. complete and unretryable idle hide thinking download row
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_retry: false,
  downloads: {
    thinking: { state: 'complete' },
  },
}));
if (root().querySelector('[data-download="thinking"]')) {
  throw new Error('completed download should not have thinking download row');
}
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_retry: false,
  downloads: {
    thinking: { state: 'idle' },
  },
}));
if (root().querySelector('[data-download="thinking"]')) {
  throw new Error('idle download without retry should not have thinking download row');
}
pass('complete and unretryable idle hide thinking download row');

// 35. unretryable speech error displays detail without retry button
paint(view({
  phase: 'thinking',
  step: 4,
  speech_retry: false,
  downloads: {
    speech: { state: 'error', error: { detail: 'speech-fail-detail' }, detail: 'speech-fail-detail' },
  },
}));
if (!root().textContent.includes('speech-fail-detail')) {
  throw new Error('speech error detail not visible');
}
if (root().querySelector('button[data-focus-key="speech:retry"]')) {
  throw new Error('unretryable speech error showed retry button');
}
pass('unretryable speech error displays detail without retry button');

// 36. setup progress tracks phases accurately
const progressChecks = [
  { view: view({ phase: 'welcome' }), expected: 'step 1 of 8' },
  { view: view({ phase: 'sign-in' }), expected: 'step 1 of 8' },
  { view: view({ phase: 'folder' }), expected: 'step 2 of 8' },
  { view: view({ phase: 'setup', setup_running: true, setup_failed: false }), expected: 'step 2 of 8' },
  { view: view({ phase: 'setup', setup_running: false, setup_failed: true }), expected: 'step 2 of 8' },
  { view: view({ phase: 'mark' }), expected: 'step 3 of 8' },
  { view: view({ phase: 'thinking' }), expected: 'step 4 of 8' },
];
for (const pc of progressChecks) {
  paint(pc.view);
  const prog = root().querySelector('.setup-progress');
  if (!prog || prog.getAttribute('aria-label') !== pc.expected) {
    throw new Error(`expected progress ${pc.expected} for ${pc.view.phase}, got ${prog?.getAttribute('aria-label')}`);
  }
}
const noProgressChecks = [
  view({ phase: 'home' }),
  view({ phase: 'opening' }),
  view({ phase: 'setup', setup_running: false, setup_failed: false }),
];
for (const np of noProgressChecks) {
  paint(np);
  await flush();
  if (query(document.body, '.setup-progress')) {
    throw new Error(`expected no progress for ${np.phase}`);
  }
}
pass('setup progress tracks phases accurately');

// 37. home notice renders download and clears setup frame
click('home');
await flush();
paint(view({
  phase: 'home',
  step: 5,
  thinking_cancel: true,
  downloads: {
    thinking: { state: 'downloading', attempt_id: 'home-att-1', bytes_done: 1500000, bytes_total: 40000000 },
  },
}));
const homeNotice = $('pane').querySelector('.notice[data-download="thinking"]');
if (!homeNotice) throw new Error('home notice for thinking download missing');
if (!homeNotice.textContent.includes('ThinkDl') || !homeNotice.textContent.includes('1.5 MB of 40 MB')) {
  throw new Error(`home notice text missing ThinkDl or human-unit bytes: ${homeNotice.textContent}`);
}
if (homeNotice.textContent.includes('1500000') || homeNotice.textContent.includes('40000000')) {
  throw new Error(`home notice shows a raw byte count: ${homeNotice.textContent}`);
}
if (!homeNotice.querySelector('button') || !homeNotice.querySelector('button').textContent.includes('Cancel')) {
  throw new Error('home notice missing Cancel button');
}
if (root().querySelector('.setup-band') || root().querySelector('.setup-progress')) {
  throw new Error('firstrun retained setup band or progress on home');
}
statusPlan.push(statusBody('stopped', true));
window.journalApp.receive({ type: 'shown' });
await flush();
paint(view({ phase: 'opening' }));
if (root().querySelector('.setup-band') || root().querySelector('.setup-progress')) {
  throw new Error('firstrun retained setup band or progress on opening jump');
}
pass('home notice renders download and clears setup frame');

// 38. pressEnter on try another triggers regenerate-mark
paint(view({
  phase: 'mark',
  step: 3,
  mark: { proposal_id: 'prop-try', render: validRender, state: 'proposed' },
}));
const tryAnotherBtn = buttons().find(b => b.textContent.trim() === 'Try');
if (!tryAnotherBtn) throw new Error('try another button not found');
tryAnotherBtn.focus();
pressEnter();
const tryAnotherMsg = inbox.shift();
if (tryAnotherMsg?.cmd !== 'flow' || tryAnotherMsg?.args?.intent !== 'regenerate-mark') {
  throw new Error(`expected regenerate-mark, got ${JSON.stringify(tryAnotherMsg)}`);
}
if (inbox.some(m => m.args?.intent === 'commit-mark')) {
  throw new Error('pressEnter committed mark instead of regenerating');
}
pass('pressEnter on try another triggers regenerate-mark');

// 39. qualification hooks align with styling and accessibility contracts
paint(view({ phase: 'welcome', step: 1 }));
const welcomePrimaries = root().querySelectorAll('button.primary');
if (welcomePrimaries.length !== 1 || welcomePrimaries[0].textContent.trim() !== 'start setup') {
  throw new Error(`welcome expected 1 primary button (start setup), found ${welcomePrimaries.length}`);
}
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_capability: 'ok',
  thinking_saved: false,
  thinking_choice: null,
}));
const thinkingPrimaries = root().querySelectorAll('button.primary');
if (thinkingPrimaries.length !== 1 || thinkingPrimaries[0].textContent.trim() !== 'Continue') {
  throw new Error(`thinking expected 1 primary button (Continue), found ${thinkingPrimaries.length}`);
}
const rowsLocal = root().querySelector('button[data-choice-id="local"]');
const rowsSpp = root().querySelector('button[data-choice-id="spp"]');
const rowsByo = root().querySelector('button[data-choice-id="byo"]');
if (!rowsLocal || !rowsSpp || !rowsByo) {
  throw new Error('missing data-choice-id for local, spp, or byo');
}
paint(view({ phase: 'welcome', step: 1 }));
const signInCheckBtn = $('sign-in-check');
if (!signInCheckBtn || signInCheckBtn.tagName !== 'BUTTON' || !signInCheckBtn.hasAttribute('aria-checked')) {
  throw new Error('#sign-in-check is not a BUTTON with aria-checked');
}
paint(view({
  phase: 'thinking',
  step: 4,
  thinking_capability: 'ok',
  thinking_saved: false,
  thinking_choice: null,
}));
const laterBtn = root().querySelector('button[data-focus-key="later"]');
if (!laterBtn || laterBtn.textContent.trim() !== 'Later') {
  throw new Error(`expected later button text 'Later', got '${laterBtn?.textContent}'`);
}
pass('qualification hooks align with styling and accessibility contracts');

// 40. horizontal lockup asset exists and is served with svg mime type
const appRsText = readFileSync(join(here, '../src/app.rs'), 'utf8');
const appJsText = readFileSync(join(here, '../ui/app.js'), 'utf8');
if (!appJsText.includes("'/lockup-solstone-horizontal.svg'") && !appJsText.includes('"/lockup-solstone-horizontal.svg"')) {
  throw new Error('app.js does not reference /lockup-solstone-horizontal.svg');
}
if (!appRsText.includes('"/lockup-solstone-horizontal.svg"')) {
  throw new Error('app.rs does not match /lockup-solstone-horizontal.svg in asset()');
}
if (!appRsText.includes('"image/svg+xml"')) {
  throw new Error('app.rs does not serve image/svg+xml for lockup');
}
if (!appRsText.includes('include_bytes!("../ui/lockup-solstone-horizontal.svg")')) {
  throw new Error('app.rs does not include_bytes! lockup SVG');
}
const svgPath = join(here, '../ui/lockup-solstone-horizontal.svg');
if (!existsSync(svgPath)) {
  throw new Error(`lockup SVG file does not exist at ${svgPath}`);
}
pass('horizontal lockup asset exists and is served with svg mime type');

// Craft pass: no stray "null" text, the welcome card carries the fits rows and the sign-in switch,
// and the footer shows its step in words.
for (const phase of ['welcome', 'folder', 'mark', 'thinking']) {
  paint(view({ phase, step: { welcome: 1, folder: 2, mark: 3, thinking: 4 }[phase], thinking_capability: 'ok' }));
  if (/\bnull\b/.test(root().textContent)) throw new Error(`${phase} renders the word null`);
  const footer = root().querySelector('.setup-footer');
  if (!footer || !footer.textContent.includes(`step ${{ welcome: 1, folder: 2, mark: 3, thinking: 4 }[phase]} of 8`)) {
    throw new Error(`${phase} footer does not show its step in words`);
  }
}
paint(view({ phase: 'welcome', step: 1 }));
{
  const card = root().querySelector('.setup-card');
  if (!card || !card.textContent.includes('what this sets up')) throw new Error('the welcome card does not carry what this sets up');
  if (!card.textContent.includes('lives on this PC and keeps everything that goes into it.')) throw new Error('the welcome card lacks the journal row');
  const sw = root().querySelector('#sign-in-check');
  if (!sw || sw.getAttribute('role') !== 'switch' || !card.contains(sw)) throw new Error('the sign-in switch is not in the welcome card');
  if (!sw.textContent.includes('start automatically after you sign in to this PC.')) throw new Error('the switch lacks its help line');
}
pass('craft pass: no null text, welcome card rows and switch, footer step words');

// Behaviour holds under the shared layout.
// A fresh install's flow is not yet authoritative; start setup must still be pressable.
paint(view({ phase: 'welcome', step: 1, authoritative: false }));
{
  const primary = root().querySelector('button.primary');
  if (!primary || primary.disabled) throw new Error('start setup is disabled while the flow is not yet authoritative');
}
pass('welcome start setup works before the flow is authoritative');

// Between setup finishing and the state loading, nothing reads as stopped.
paint(view({ phase: 'setup', step: null, setup_running: false, setup_failed: false, error: null, busy: true }));
if (root().textContent.includes("setup stopped before it finished.")) throw new Error('a finished setup reads as stopped');
pass('the post-setup interstitial does not say setup stopped');

// Every choice's description is readable before it is chosen.
paint(view({ phase: 'thinking', step: 4, thinking_capability: 'blocked', thinking_saved: false, thinking_choice: null }));
if (!root().textContent.includes('spp description') || !root().textContent.includes('byo description')) {
  throw new Error('unselected choices hide their descriptions');
}
pass('every thinking choice shows its description');

// A choice keeps focus through its busy round-trip.
paint(view({ phase: 'thinking', step: 4, thinking_capability: 'blocked', thinking_saved: false, thinking_choice: null }));
{
  const spp = root().querySelector('[data-choice-id="spp"]');
  spp.focus();
  paint(view({ phase: 'thinking', step: 4, thinking_capability: 'blocked', thinking_saved: false, thinking_choice: null, busy: true }));
  paint(view({ phase: 'thinking', step: 4, thinking_capability: 'blocked', thinking_saved: true, thinking_choice: 'spp', can_finalize: true }));
  const after = root().querySelector('[data-choice-id="spp"]');
  if (document.activeElement !== after) throw new Error('focus left the chosen row after its busy round-trip');
}
pass('a chosen row keeps focus through a busy repaint');

// Cancel needs the attempt it would cancel.
paint(view({ phase: 'thinking', step: 4, thinking_capability: 'ok', thinking_saved: true, thinking_choice: 'local', can_finalize: true,
  thinking_cancel: true, downloads: { thinking: { state: 'downloading', bytes_done: 1, bytes_total: 2, attempt_id: '  ' } } }));
if (root().querySelector('[data-focus-key="thinking:cancel"]')) throw new Error('cancel shows without an attempt id');
pass('cancel needs a painted attempt id');

// The sun arc ground: mounted once while the setup frame shows, outside the node every refresh
// rebuilds, and torn down when the frame goes.
{
  const ground = $('setup-ground');
  if (!ground || root().contains(ground)) throw new Error('the sun arc ground is missing or inside the rebuilt setup frame');
  if (ground.hidden || sunArc.live !== 1 || sunArc.root !== ground) throw new Error(`setup does not show one live sun arc on its ground (live ${sunArc.live})`);
  const mounts = sunArc.mounts;
  for (let done = 1; done <= 3; done += 1) {
    paint(view({ phase: 'thinking', step: 4, thinking_capability: 'ok', local_offered: true, can_finalize: true, watch_downloads: true,
      downloads: { speech: { state: 'downloading', bytes_done: done * 1000000, bytes_total: 4000000, attempt_id: 'a1' } } }));
  }
  if (sunArc.mounts !== mounts || $('setup-ground') !== ground || ground.hidden) throw new Error('a download repaint remounted or hid the sun arc');
  paint(view({ phase: 'thinking', step: 4, resting: true }));
  if (!visible($('main')) || !ground.hidden || sunArc.live !== 0) throw new Error(`leaving setup did not tear the sun arc down (live ${sunArc.live})`);
  paint(view({ phase: 'welcome', step: 1 }));
  if (ground.hidden || sunArc.live !== 1 || sunArc.mounts !== mounts + 1) throw new Error('returning to setup did not mount the sun arc again');
  if (sunArc.overlap) throw new Error('two sun arcs were live at once');
}
pass('the sun arc lives outside the setup frame, survives repaints and is torn down on leaving');

// The window serves the engine and the mark at the path the engine asks for, and loads the engine first.
{
  const engine = readFileSync(join(here, '../../solstone-core-convey-shell/assets/static/sunarc.js'), 'utf8');
  if (!engine.includes("'/static/sol-status/mark.svg'")) throw new Error('sunarc.js no longer asks for /static/sol-status/mark.svg; move the asset() row with it');
  if (!appRsText.includes('"/static/sol-status/mark.svg"') || !appRsText.includes('assets/static/sol-status/mark.svg")')) {
    throw new Error('app.rs does not serve the sun mark at /static/sol-status/mark.svg');
  }
  if (!appRsText.includes('"/sunarc.js"') || !appRsText.includes('assets/static/sunarc.js")')) throw new Error('app.rs does not serve /sunarc.js');
  const engineTag = html.indexOf('<script src="sunarc.js"></script>');
  if (engineTag < 0 || engineTag > html.indexOf('src="app.js"')) throw new Error('index.html does not load sunarc.js before app.js');
  if (html.indexOf('id="setup-ground"') > html.indexOf('id="firstrun"')) throw new Error('#setup-ground must precede #firstrun for the ground rule');
}
pass('the sun arc engine and mark are served and loaded before the app');

console.log(`DOM CASES: ${passed} passed`);
