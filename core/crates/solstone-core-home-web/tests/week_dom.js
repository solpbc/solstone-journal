// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2] || path.resolve(__dirname, '..');
const weekJsPath = path.join(crateDir, 'assets/week.js');
const weekJsCode = fs.readFileSync(weekJsPath, 'utf8');

const conveyShellDir = path.resolve(crateDir, '../solstone-core-convey-shell');
const markedPath = path.join(conveyShellDir, 'assets/static/vendor/marked/marked.min.js');
const marked = require(markedPath);
const appJsPath = path.join(conveyShellDir, 'assets/static/app.js');
const appJsCode = fs.readFileSync(appJsPath, 'utf8');
const sourceLinkBackPath = path.join(conveyShellDir, 'assets/static/source_link_back.js');
const sourceLinkBackCode = fs.readFileSync(sourceLinkBackPath, 'utf8');

const modelsPath = process.argv[3];
if (!modelsPath || !fs.existsSync(modelsPath)) {
  throw new Error('models.json path argument is required and must exist');
}
const realModels = JSON.parse(fs.readFileSync(modelsPath, 'utf8'));

if (!realModels.primary || !realModels.left_out || !realModels.all_states || !realModels.said) {
  throw new Error('models.json missing required models: primary, left_out, all_states, said');
}

const primaryModel = realModels.primary;
const leftOutModel = realModels.left_out;
const allStatesModel = realModels.all_states;
const saidModel = realModels.said;

class ClassList {
  constructor(element) {
    this.element = element;
    this.values = new Set();
  }

  setFromString(value) {
    this.values = new Set(String(value || '').split(/\s+/).filter(Boolean));
  }

  sync() {
    if (this.values.size) {
      this.element.attributes.class = Array.from(this.values).join(' ');
    } else {
      delete this.element.attributes.class;
    }
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
  return attribute.slice(5).replace(/-([a-z])/g, (_, character) => character.toUpperCase());
}

function decodeHtmlEntities(str) {
  return String(str ?? '')
    .replace(/&#39;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&amp;/g, '&');
}

function parseAttributes(source, element) {
  const attributePattern = /([^\s=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g;
  let match;
  while ((match = attributePattern.exec(source))) {
    const name = match[1];
    const value = match[2] ?? match[3] ?? match[4] ?? '';
    element.setAttribute(name, decodeHtmlEntities(value));
  }
}

function parseHtml(html, root) {
  root.children.slice().forEach((child) => root.removeChild(child));
  const stack = [root];
  const tagPattern = /<\/?[^>]+>|[^<]+/g;
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
    if (token.startsWith('<')) {
      const selfClosing = token.endsWith('/>');
      const parts = /^<\s*([^\s/>]+)([\s\S]*?)\/?>$/.exec(token);
      if (!parts) continue;
      const child = root.ownerDocument.createElement(parts[1]);
      parseAttributes(parts[2], child);
      stack[stack.length - 1].appendChild(child);
      if (!selfClosing && !['BR', 'IMG', 'INPUT', 'META', 'LINK', 'HR'].includes(child.tagName)) {
        stack.push(child);
      }
    } else {
      const text = token;
      if (text) {
        const top = stack[stack.length - 1];
        top.appendChild(root.ownerDocument.createTextNode(decodeHtmlEntities(text)));
      }
    }
  }
}

function selectorMatches(element, selector) {
  selector = selector.trim();
  const notMatch = selector.match(/:not\(([^)]+)\)$/);
  if (notMatch) {
    if (selectorMatches(element, notMatch[1])) return false;
    selector = selector.slice(0, notMatch.index);
  }

  const attributes = [...selector.matchAll(/\[([^\]=]+)(?:=["']?([^\]"']+)["']?)?\]/g)];
  selector = selector.replace(/\[[^\]]+\]/g, '');
  const idMatch = selector.match(/#([\w-]+)/);
  const classMatches = [...selector.matchAll(/\.([\w-]+)/g)];
  const tagMatch = selector.match(/^[a-zA-Z][\w-]*/);
  if (tagMatch && element.tagName !== tagMatch[0].toUpperCase()) return false;
  if (idMatch && element.id !== idMatch[1]) return false;
  if (classMatches.some((match) => !element.classList.contains(match[1]))) return false;
  return attributes.every((match) => {
    if (!element.hasAttribute(match[1])) return false;
    return match[2] === undefined || element.getAttribute(match[1]) === match[2];
  });
}

function matchesSelector(element, selector) {
  return selector.split(',').some((part) => selectorMatches(element, part));
}

function queryAll(root, selector) {
  const pieces = selector.trim().split(/\s+/);
  let candidates = [root];
  for (const piece of pieces) {
    const next = [];
    for (const candidate of candidates) {
      const visit = (element) => {
        element.children.forEach((child) => {
          if (matchesSelector(child, piece)) next.push(child);
          visit(child);
        });
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
    this.style = {};
    this.dataset = {};
    this.classList = new ClassList(this);
    this.value = '';
    this.disabled = false;
    this.isContentEditable = false;
    this._textContent = '';
  }

  get id() {
    return this.getAttribute('id') || '';
  }

  set id(value) {
    this.setAttribute('id', value);
  }

  get hidden() {
    return this.hasAttribute('hidden');
  }

  set hidden(value) {
    if (value) this.setAttribute('hidden', '');
    else this.removeAttribute('hidden');
  }

  get className() {
    return this.getAttribute('class') || '';
  }

  set className(value) {
    this.setAttribute('class', value);
  }

  get href() {
    return this.getAttribute('href') || '';
  }

  set href(value) {
    this.setAttribute('href', value);
  }

  get type() {
    return this.getAttribute('type') || '';
  }

  set type(value) {
    this.setAttribute('type', value);
  }

  get src() {
    return this.getAttribute('src') || '';
  }

  set src(value) {
    this.setAttribute('src', value);
  }

  get open() {
    return this.hasAttribute('open');
  }

  set open(value) {
    if (value) this.setAttribute('open', '');
    else this.removeAttribute('open');
  }

  replaceChildren(...nodes) {
    this.children.slice().forEach((c) => this.removeChild(c));
    this.append(...nodes);
  }

  get childNodes() {
    return this.children;
  }

  get nodeName() {
    return this.tagName;
  }

  get nodeType() {
    return this.tagName === 'FRAGMENT' ? 11 : (this.tagName === '#TEXT' ? 3 : 1);
  }

  get isConnected() {
    let current = this;
    while (current.parentElement) current = current.parentElement;
    return current === this.ownerDocument.body || current === this.ownerDocument.documentElement;
  }

  get innerHTML() {
    return this.children
      .map((c) => {
        if (c.nodeType === 3) return c.textContent;
        if (c.nodeType === 1) {
          const tag = c.tagName.toLowerCase();
          const attrs = Object.entries(c.attributes)
            .map(([k, v]) => ` ${k}="${v}"`)
            .join('');
          if (['img', 'br', 'hr', 'input'].includes(tag)) {
            return `<${tag}${attrs}>`;
          }
          return `<${tag}${attrs}>${c.innerHTML}</${tag}>`;
        }
        return '';
      })
      .join('');
  }

  set innerHTML(value) {
    parseHtml(String(value), this);
  }

  get textContent() {
    if (this.tagName === '#TEXT') {
      return this._textContent || '';
    }
    return this.children.map((c) => c.textContent).join('');
  }

  set textContent(value) {
    this.children.slice().forEach((c) => this.removeChild(c));
    if (this.tagName === '#TEXT') {
      this._textContent = String(value);
    } else if (value !== '') {
      this.appendChild(this.ownerDocument.createTextNode(String(value)));
    }
  }

  setAttribute(name, value) {
    const lower = name.toLowerCase();
    this.attributes[lower] = String(value);
    if (lower === 'class') this.classList.setFromString(value);
    if (lower.startsWith('data-')) this.dataset[dataKey(lower)] = String(value);
  }

  getAttribute(name) {
    const lower = name.toLowerCase();
    return Object.prototype.hasOwnProperty.call(this.attributes, lower) ? this.attributes[lower] : null;
  }

  hasAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name.toLowerCase());
  }

  removeAttribute(name) {
    const lower = name.toLowerCase();
    delete this.attributes[lower];
    if (lower === 'class') this.classList.setFromString('');
    if (lower.startsWith('data-')) delete this.dataset[dataKey(lower)];
  }

  appendChild(child) {
    if (child.nodeType === 11) {
      const grandChildren = child.children.slice();
      grandChildren.forEach((gc) => this.appendChild(gc));
      return child;
    }
    if (child.parentElement) child.parentElement.removeChild(child);
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  append(...children) {
    children.forEach((child) => {
      if (typeof child === 'string') {
        this.appendChild(this.ownerDocument.createTextNode(child));
      } else {
        this.appendChild(child);
      }
    });
  }

  removeChild(child) {
    this.children = this.children.filter((item) => item !== child);
    child.parentElement = null;
  }

  remove() {
    if (this.parentElement) {
      this.parentElement.removeChild(this);
    }
  }

  replaceWith(...nodes) {
    if (!this.parentElement) return;
    const parent = this.parentElement;
    const idx = parent.children.indexOf(this);
    if (idx === -1) return;
    const toInsert = [];
    nodes.forEach((n) => {
      if (typeof n === 'string') {
        n = this.ownerDocument.createTextNode(n);
      }
      if (n.nodeType === 11) {
        toInsert.push(...n.children);
        n.children.forEach((c) => {
          c.parentElement = parent;
        });
        n.children = [];
      } else {
        if (n.parentElement) n.parentElement.removeChild(n);
        n.parentElement = parent;
        toInsert.push(n);
      }
    });
    parent.children.splice(idx, 1, ...toInsert);
    this.parentElement = null;
  }

  contains(candidate) {
    if (candidate === this) return true;
    return this.children.some((child) => child.contains(candidate));
  }

  matches(selector) {
    return matchesSelector(this, selector);
  }

  closest(selector) {
    let current = this;
    while (current) {
      if (current.matches(selector)) return current;
      current = current.parentElement;
    }
    return null;
  }

  querySelector(selector) {
    return queryAll(this, selector)[0] || null;
  }

  querySelectorAll(selector) {
    return queryAll(this, selector);
  }

  addEventListener(type, listener) {
    (this.listeners[type] ||= []).push(listener);
  }

  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    if (event.bubbles && !event.cancelBubble && this.parentElement) this.parentElement.dispatchEvent(event);
    else if (event.bubbles && !event.cancelBubble && this === this.ownerDocument.documentElement) {
      this.ownerDocument.dispatchEvent(event);
    }
    return !event.defaultPrevented;
  }

  click() {
    const event = {
      type: 'click',
      bubbles: true,
      cancelBubble: false,
      defaultPrevented: false,
      target: this,
      preventDefault() { this.defaultPrevented = true; },
      stopPropagation() { this.cancelBubble = true; }
    };
    this.dispatchEvent(event);
  }

  focus() {
    this.ownerDocument.activeElement = this;
  }
}

class Document {
  constructor() {
    this.documentElement = new Element('html', this);
    this.body = new Element('body', this);
    this.documentElement.appendChild(this.body);
    this.activeElement = null;
    this.listeners = {};
    this.readyState = 'complete';
  }

  createElement(tagName) {
    return new Element(tagName, this);
  }

  createDocumentFragment() {
    return new Element('fragment', this);
  }

  createTextNode(text) {
    const el = new Element('#text', this);
    el.textContent = text;
    return el;
  }

  getElementById(id) {
    return this.querySelector('#' + id);
  }

  querySelector(selector) {
    return queryAll(this.documentElement, selector)[0] || null;
  }

  querySelectorAll(selector) {
    return queryAll(this.documentElement, selector);
  }

  addEventListener(type, listener) {
    (this.listeners[type] ||= []).push(listener);
  }

  dispatchEvent(event) {
    event.target ||= this;
    event.currentTarget = this;
    for (const listener of this.listeners[event.type] || []) listener.call(this, event);
    return !event.defaultPrevented;
  }

  createTreeWalker(root, _whatToShow) {
    const textNodes = [];
    function collect(n) {
      if (n.nodeType === 3) {
        textNodes.push(n);
      } else if (n.children) {
        n.children.forEach(collect);
      }
    }
    collect(root);
    let idx = -1;
    return {
      currentNode: null,
      nextNode() {
        idx++;
        if (idx < textNodes.length) {
          this.currentNode = textNodes[idx];
          return true;
        }
        this.currentNode = null;
        return false;
      }
    };
  }
}

function createDOMPurify() {
  return {
    hooks: {},
    addHook(name, fn) {
      this.hooks[name] = this.hooks[name] || [];
      this.hooks[name].push(fn);
    },
    sanitize(html, _options) {
      const doc = new Document();
      const frag = doc.createElement('div');
      frag.innerHTML = html;
      const uponSanitizeAttributeHooks = this.hooks['uponSanitizeAttribute'] || [];
      function checkNode(n) {
        if (n.nodeType === 1) {
          const attrNames = Object.keys(n.attributes);
          for (const attr of attrNames) {
            const data = { attrName: attr, attrValue: n.attributes[attr], keepAttr: true };
            uponSanitizeAttributeHooks.forEach((h) => h(n, data));
            if (!data.keepAttr) {
              n.removeAttribute(attr);
            } else {
              n.setAttribute(attr, data.attrValue);
            }
          }
        }
        if (n.children) {
          n.children.slice().forEach(checkNode);
        }
      }
      checkNode(frag);
      return frag.innerHTML;
    }
  };
}

class CustomEvent {
  constructor(type, eventInitDict = {}) {
    this.type = type;
    this.detail = eventInitDict.detail || null;
    this.bubbles = !!eventInitDict.bubbles;
    this.cancelable = !!eventInitDict.cancelable;
    this.defaultPrevented = false;
    this.cancelBubble = false;
    this.target = null;
  }
  preventDefault() { this.defaultPrevented = true; }
  stopPropagation() { this.cancelBubble = true; }
}

function createEnvironment(pathname = '/app/home/week/20260308', evalWeek = true) {
  const doc = new Document();
  const DOMPurify = createDOMPurify();
  const win = {
    document: doc,
    location: {
      pathname: pathname,
      href: 'http://localhost:8080' + pathname,
      origin: 'http://localhost:8080',
      protocol: 'http:',
      search: ''
    },
    DOMPurify: DOMPurify,
    marked: marked,
    ConveyIcons: {
      svg: (name) => `<svg data-icon="${name}"></svg>`
    },
    CONVEY_COPY: {
      RELOAD_HINT: 'reload to try again.'
    },
    addEventListener: (type, listener) => doc.addEventListener(type, listener),
    removeEventListener: () => {},
    CustomEvent: CustomEvent,
    Element: Element,
    HTMLElement: Element,
    Node: Element,
    Document: Document
  };
  const sandbox = {
    window: win,
    document: doc,
    location: win.location,
    DOMPurify: DOMPurify,
    marked: marked,
    URL: URL,
    NodeFilter: { SHOW_TEXT: 4 },
    localStorage: { getItem() { return null; }, setItem() {} },
    navigator: {},
    console: console,
    CustomEvent: CustomEvent,
    Event: CustomEvent,
    Element: Element,
    HTMLElement: Element,
    Node: Element,
    Document: Document,
    encodeURIComponent: encodeURIComponent,
    JSON: JSON,
    fetch: null,
    setTimeout: setTimeout,
    clearTimeout: clearTimeout,
    Promise: Promise
  };
  vm.createContext(sandbox);
  vm.runInContext(appJsCode, sandbox, { filename: 'app.js' });
  vm.runInContext(sourceLinkBackCode, sandbox, { filename: 'source_link_back.js' });
  if (evalWeek) {
    vm.runInContext(weekJsCode, sandbox, { filename: 'week.js' });
  }
  return { doc, win, sandbox };
}

let caseCount = 0;

async function runTests() {
  // 1. Test real primary page model assertions & markdown rendering
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.window.renderWeekPage(surface, primaryModel);

    // Exact titles, intros, from-lines
    const title = surface.querySelector('.week-title');
    assert.strictEqual(title.textContent, primaryModel.title, 'exact title');
    const intro = surface.querySelector('.week-intro');
    assert.strictEqual(intro.textContent, primaryModel.intro, 'exact intro');
    const fromLine = surface.querySelector('.week-from-line');
    assert.strictEqual(fromLine.textContent, primaryModel.from_line, 'exact from_line');

    // 7 cells with role="img" and accessible names
    const cells = surface.querySelectorAll('.week-cell');
    assert.strictEqual(cells.length, 7, '7 cells in grid');
    cells.forEach((cell, idx) => {
      assert.strictEqual(cell.getAttribute('role'), 'img', `cell ${idx} role="img"`);
      assert.strictEqual(cell.getAttribute('aria-label'), primaryModel.cells[idx].accessible_name, `cell ${idx} aria-label`);
    });

    // Menu button names and order
    const menuBtn = surface.querySelector('.week-menu-btn');
    assert.strictEqual(menuBtn.getAttribute('aria-label'), 'more for sun 8', 'menu btn aria-label');

    const menuDropdown = surface.querySelector('.week-menu-dropdown');
    assert.strictEqual(menuDropdown.children.length, 2, '2 menu items in dropdown');
    // First item: leave out button
    assert.strictEqual(menuDropdown.children[0].tagName, 'BUTTON');
    assert.strictEqual(menuDropdown.children[0].getAttribute('data-action'), 'leave-out');
    // Second item: this isn't right link
    assert.strictEqual(menuDropdown.children[1].tagName, 'A');
    assert.strictEqual(menuDropdown.children[1].getAttribute('href'), primaryModel.rows[0].peek_href);

    // Single provenance anchor
    const peekAnchor = surface.querySelector('.week-row-peek a');
    assert.ok(peekAnchor, 'peek anchor exists');
    assert.strictEqual(peekAnchor.getAttribute('href'), primaryModel.rows[0].peek_href);

    // Nav positioning: week-nav has both prev and next, is above week-header
    assert.ok(primaryModel.prev_href, 'primary model has prev_href');
    assert.ok(primaryModel.next_href, 'primary model has next_href');
    const dashChildren = surface.querySelector('.week-dashboard').children;
    const navIdx = dashChildren.findIndex((c) => c.classList.contains('week-nav'));
    const headerIdx = dashChildren.findIndex((c) => c.classList.contains('week-header'));
    assert.ok(navIdx !== -1 && headerIdx !== -1 && navIdx < headerIdx, 'nav precedes header');

    // Trailing prev link: week-prev-repeat is below week-end-line and contains no next_href
    const endLineIdx = dashChildren.findIndex((c) => c.classList.contains('week-end-line'));
    const prevRepeatIdx = dashChildren.findIndex((c) => c.classList.contains('week-prev-repeat'));
    assert.ok(endLineIdx !== -1 && prevRepeatIdx !== -1 && endLineIdx < prevRepeatIdx, 'prev-repeat follows end-line');
    const repeatAnchors = surface.querySelectorAll('.week-prev-repeat a');
    assert.strictEqual(repeatAnchors.length, 1);
    assert.strictEqual(repeatAnchors[0].getAttribute('href'), primaryModel.prev_href);

    // Markdown rendered memory text: any sol:// links rendered as anchor text moment, newsletter, or source (not raw sol://)
    const rowTexts = surface.querySelectorAll('.week-row-text');
    rowTexts.forEach((rt) => {
      const anchors = rt.querySelectorAll('a');
      anchors.forEach((a) => {
        const text = a.textContent.trim();
        assert.ok(
          !text.startsWith('sol://') &&
            (text.startsWith('moment') ||
              text.startsWith('newsletter') ||
              text.startsWith('source') ||
              !a.getAttribute('href')?.startsWith('/source?ref=')),
          `anchor text is classified label: ${text}`
        );
      });
    });
  }

  // 2. Test left-out row rendering and undo button
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.window.renderWeekPage(surface, leftOutModel);
    const leftOutRow = surface.querySelector('.week-row--left-out');
    assert.ok(leftOutRow, 'left out row rendered');
    const undoBtn = leftOutRow.querySelector('.week-undo-btn');
    assert.ok(undoBtn, 'undo btn exists');
    assert.strictEqual(undoBtn.getAttribute('data-action'), 'undo');
    assert.strictEqual(undoBtn.getAttribute('data-key'), 'b359922341f75193');
  }

  // 3. Test all states model grid cells
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.window.renderWeekPage(surface, allStatesModel);
    const cells = surface.querySelectorAll('.week-cell');
    assert.strictEqual(cells.length, 7, '7 cells in all_states model');
    cells.forEach((cell, idx) => {
      assert.strictEqual(cell.getAttribute('role'), 'img', `cell ${idx} role="img"`);
      assert.strictEqual(cell.getAttribute('aria-label'), allStatesModel.cells[idx].accessible_name, `cell ${idx} aria-label`);
    });
  }

  // 4. Test focus tracking on successful leave-out and undo
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.fetch = (url, opts) => {
      if (opts && opts.method === 'POST') {
        const body = JSON.parse(opts.body);
        if (body.undo) {
          return Promise.resolve({ ok: true, json: () => Promise.resolve(primaryModel) });
        } else {
          return Promise.resolve({ ok: true, json: () => Promise.resolve(leftOutModel) });
        }
      }
      return Promise.resolve({ ok: true, json: () => Promise.resolve(primaryModel) });
    };

    sandbox.window.renderWeekPage(surface, primaryModel);

    // Click leave-out button
    const leaveOutBtn = surface.querySelector('.week-leave-out-btn');
    assert.ok(leaveOutBtn, 'leave-out button exists');
    leaveOutBtn.click();

    await new Promise((r) => setTimeout(r, 10));

    // Focus should be on the undo button
    const undoBtn = surface.querySelector('.week-undo-btn');
    assert.ok(undoBtn, 'undo btn rendered');
    assert.strictEqual(doc.activeElement, undoBtn, 'focus moved to undo button after leave-out');

    // Click undo button
    undoBtn.click();
    await new Promise((r) => setTimeout(r, 10));

    // Focus should be on the restored menu button
    const menuBtn = surface.querySelector('.week-menu-btn');
    assert.ok(menuBtn, 'menu btn restored');
    assert.strictEqual(doc.activeElement, menuBtn, 'focus moved to menu button after undo');
  }

  // 5. Test failure and error re-fetch paths
  // 5a. Leave-out POST fails, re-fetch GET returns SAME model -> inline error, focus trigger button
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    let getCount = 0;
    sandbox.fetch = (url, opts) => {
      if (opts && opts.method === 'POST') {
        return Promise.reject(new Error('POST failed'));
      }
      getCount++;
      return Promise.resolve({ ok: true, json: () => Promise.resolve(primaryModel) });
    };

    sandbox.window.renderWeekPage(surface, primaryModel);
    const leaveOutBtn = surface.querySelector('.week-leave-out-btn');
    leaveOutBtn.focus();
    assert.strictEqual(doc.activeElement, leaveOutBtn);
    leaveOutBtn.click();

    await new Promise((r) => setTimeout(r, 10));

    assert.strictEqual(getCount, 1, 're-fetch GET was executed');
    const errP = surface.querySelector('.week-row-error');
    assert.ok(errP, 'inline row error exists');
    assert.strictEqual(errP.textContent, "couldn't leave this out. nothing changed.");
    assert.strictEqual(doc.activeElement, leaveOutBtn, 'focus remained on trigger button');
  }

  // 5b. Leave-out POST fails, re-fetch GET returns FLIPPED model -> renders flipped, focuses undo, no 'nothing changed'
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.fetch = (url, opts) => {
      if (opts && opts.method === 'POST') {
        return Promise.reject(new Error('POST failed'));
      }
      return Promise.resolve({ ok: true, json: () => Promise.resolve(leftOutModel) });
    };

    sandbox.window.renderWeekPage(surface, primaryModel);
    const leaveOutBtn = surface.querySelector('.week-leave-out-btn');
    leaveOutBtn.click();

    await new Promise((r) => setTimeout(r, 10));

    const undoBtn = surface.querySelector('.week-undo-btn');
    assert.ok(undoBtn, 'undo btn exists after state changed on disk');
    assert.strictEqual(doc.activeElement, undoBtn, 'focus moved to undo button');
    assert.ok(!surface.innerHTML.includes('nothing changed'), 'no error text on flipped model');
  }

  // 5c. Leave-out POST fails, re-fetch GET also FAILS -> full surface error
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.fetch = () => Promise.reject(new Error('Network offline'));

    sandbox.window.renderWeekPage(surface, primaryModel);
    const leaveOutBtn = surface.querySelector('.week-leave-out-btn');
    leaveOutBtn.click();

    await new Promise((r) => setTimeout(r, 10));

    const surfaceErr = surface.querySelector('.surface-state--error');
    assert.ok(surfaceErr, 'surface error rendered when re-fetch fails');
    const heading = surface.querySelector('.surface-state-heading');
    assert.strictEqual(heading.textContent, "couldn't load this week. reload to try again.");
  }

  // 5d. Undo POST fails, re-fetch GET returns SAME model -> inline error "couldn't bring this back. nothing changed."
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.fetch = (url, opts) => {
      if (opts && opts.method === 'POST') {
        return Promise.reject(new Error('POST undo failed'));
      }
      return Promise.resolve({ ok: true, json: () => Promise.resolve(leftOutModel) });
    };

    sandbox.window.renderWeekPage(surface, leftOutModel);
    const undoBtn = surface.querySelector('.week-undo-btn');
    undoBtn.focus();
    assert.strictEqual(doc.activeElement, undoBtn);
    undoBtn.click();

    await new Promise((r) => setTimeout(r, 10));

    const errP = surface.querySelector('.week-row-error');
    assert.ok(errP, 'inline row error exists on undo failure');
    assert.strictEqual(errP.textContent, "couldn't bring this back. nothing changed.");
    assert.strictEqual(doc.activeElement, undoBtn, 'focus remained on undo button');
  }

  // 5e. Non-JSON POST responses (parser boom) for leave-out
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.fetch = (url, opts) => {
      if (opts && opts.method === 'POST') {
        return Promise.resolve({
          ok: false,
          status: 500,
          json: () => Promise.reject(new SyntaxError('<html>parser-boom-body</html>'))
        });
      }
      return Promise.resolve({ ok: true, json: () => Promise.resolve(primaryModel) });
    };

    sandbox.window.renderWeekPage(surface, primaryModel);
    const leaveOutBtn = surface.querySelector('.week-leave-out-btn');
    leaveOutBtn.click();

    await new Promise((r) => setTimeout(r, 10));

    const errP = surface.querySelector('.week-row-error');
    assert.ok(errP, 'inline error shown on non-json POST');
    assert.strictEqual(errP.textContent, "couldn't leave this out. nothing changed.");
    assert.ok(!surface.innerHTML.includes('parser-boom-body'), 'no raw boom html leaked');
  }

  // 5f. Non-JSON POST responses for undo
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.fetch = (url, opts) => {
      if (opts && opts.method === 'POST') {
        return Promise.resolve({
          ok: false,
          status: 500,
          json: () => Promise.reject(new SyntaxError('<html>parser-boom-body</html>'))
        });
      }
      return Promise.resolve({ ok: true, json: () => Promise.resolve(leftOutModel) });
    };

    sandbox.window.renderWeekPage(surface, leftOutModel);
    const undoBtn = surface.querySelector('.week-undo-btn');
    undoBtn.click();

    await new Promise((r) => setTimeout(r, 10));

    const errP = surface.querySelector('.week-row-error');
    assert.ok(errP, 'inline error shown on non-json undo POST');
    assert.strictEqual(errP.textContent, "couldn't bring this back. nothing changed.");
    assert.ok(!surface.innerHTML.includes('parser-boom-body'), 'no raw boom html leaked');
  }

  // 6. Test search hit rendering with week_url
  {
    caseCount++;
    const searchHtmlPath = path.resolve(crateDir, '../solstone-core-records-web/assets/search.html');
    if (!fs.existsSync(searchHtmlPath)) {
      throw new Error('search.html not found');
    }
    const searchHtml = fs.readFileSync(searchHtmlPath, 'utf8');
    const searchDoc = new Document();
    searchDoc.body.innerHTML = searchHtml;

    const scriptMatch = searchHtml.match(/<script[\s\S]*?>([\s\S]*?)<\/script>/i);
    assert.ok(scriptMatch, 'script tag in search.html');
    let searchScript = scriptMatch[1];
    searchScript = searchScript.replace(
      'function renderHit(hit,dayHref){',
      'window.renderHit = renderHit; function renderHit(hit,dayHref){'
    );

    class DOMParser {
      parseFromString(html, _mime) {
        const d = new Document();
        d.body.innerHTML = html;
        return d;
      }
    }

    const searchWin = {
      document: searchDoc,
      location: { search: '' },
      URLSearchParams: URLSearchParams,
      DOMParser: DOMParser,
      JournalFormat: {
        time: () => '12:00',
        stream: (s) => s || '',
        day: (d) => d
      },
      apiJson: () => Promise.resolve({})
    };
    const searchSandbox = {
      window: searchWin,
      document: searchDoc,
      location: searchWin.location,
      URLSearchParams: URLSearchParams,
      DOMParser: DOMParser,
      console: console
    };
    vm.createContext(searchSandbox);
    vm.runInContext(searchScript, searchSandbox);

    if (typeof searchWin.renderHit !== 'function') {
      throw new Error('renderHit is not a function in search.html');
    }

    const hit = {
      week_url: '/app/home/week/20260809',
      path: 'reflections/weekly/20260809.md',
      text: 'Weekly reflection text'
    };
    const renderedCard = searchWin.renderHit(hit, null);
    const searchLink = renderedCard.querySelector('.search-link');
    assert.ok(searchLink, 'search-link anchor exists');
    assert.strictEqual(searchLink.textContent, 'open that week →');
    assert.strictEqual(searchLink.getAttribute('href'), '/app/home/week/20260809');
  }

  // 7. Test initWeek error handling
  {
    caseCount++;
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);

    sandbox.fetch = () => Promise.resolve({ ok: false, status: 404 });

    sandbox.window.initWeek('20260308');
    await new Promise((r) => setTimeout(r, 10));

    const heading = surface.querySelector('.surface-state-heading');
    assert.strictEqual(heading.textContent, "couldn't load this week. reload to try again.");
  }

  // 8. Test runMountRaceTest: home.js, week.js, removals.js workspace:mounted behavior
  {
    // 8a: Mount on /app/home/week/20260308
    caseCount++;
    const { doc, win, sandbox } = createEnvironment('/app/home/week/20260308', false);
    doc.readyState = 'complete';

    const homeRoot = doc.createElement('div');
    homeRoot.setAttribute('data-home-root', '');
    const pulseSurface = doc.createElement('div');
    pulseSurface.setAttribute('data-pulse-surface', '');
    homeRoot.appendChild(pulseSurface);
    doc.body.appendChild(homeRoot);

    const apiCalls = [];
    win.apiJson = (url) => {
      apiCalls.push(url);
      return Promise.resolve({});
    };

    let weekFetchCount = 0;
    sandbox.fetch = (url) => {
      if (url === '/app/home/api/week/20260308') {
        weekFetchCount++;
        return Promise.resolve({
          ok: true,
          json: () => Promise.resolve(primaryModel)
        });
      }
      apiCalls.push(url);
      return Promise.reject(new Error('unknown url ' + url));
    };

    const homeJsPath = path.join(crateDir, 'assets/home.js');
    const homeJsCode = fs.readFileSync(homeJsPath, 'utf8');
    const removalsJsPath = path.join(crateDir, 'assets/removals.js');
    const removalsJsCode = fs.readFileSync(removalsJsPath, 'utf8');

    // 1. home.js
    vm.runInContext(homeJsCode, sandbox, { filename: 'home.js' });
    // 2. week.js
    vm.runInContext(weekJsCode, sandbox, { filename: 'week.js' });
    // 3. removals.js
    vm.runInContext(removalsJsCode, sandbox, { filename: 'removals.js' });

    // 4. workspace:mounted
    doc.dispatchEvent(
      new CustomEvent('workspace:mounted', {
        detail: { appName: 'home', url: '/app/home/' }
      })
    );

    await new Promise((r) => setTimeout(r, 10));

    assert.strictEqual(weekFetchCount, 1, 'GET /app/home/api/week/20260308 called exactly 1 time');
    assert.strictEqual(apiCalls.filter((u) => u.includes('pulse')).length, 0, 'pulse API not called on week page');
    assert.strictEqual(apiCalls.filter((u) => u.includes('removals')).length, 0, 'removals API not called on week page');
    assert.strictEqual(doc.querySelector('[data-removals-card]'), null, 'no removals card mounted on week page');
    const title = doc.querySelector('.week-title');
    assert.ok(title, 'week view rendered');
    assert.strictEqual(title.textContent, primaryModel.title);

    // 8b: Twin test on /app/home/
    caseCount++;
    const envHome = createEnvironment('/app/home/', false);
    envHome.doc.readyState = 'complete';
    const homeRoot2 = envHome.doc.createElement('div');
    homeRoot2.setAttribute('data-home-root', '');
    const pulseSurface2 = envHome.doc.createElement('div');
    pulseSurface2.setAttribute('data-pulse-surface', '');
    homeRoot2.appendChild(pulseSurface2);
    envHome.doc.body.appendChild(homeRoot2);

    const apiCalls2 = [];
    envHome.win.apiJson = (url) => {
      apiCalls2.push(url);
      if (url.includes('pulse')) {
        return Promise.resolve({ vitals: {} });
      }
      if (url.includes('removals')) {
        return Promise.resolve({ items: [] });
      }
      return Promise.resolve({});
    };

    // 1. home.js
    vm.runInContext(homeJsCode, envHome.sandbox, { filename: 'home.js' });
    // 2. week.js
    vm.runInContext(weekJsCode, envHome.sandbox, { filename: 'week.js' });
    // 3. removals.js
    vm.runInContext(removalsJsCode, envHome.sandbox, { filename: 'removals.js' });

    // 4. workspace:mounted
    envHome.doc.dispatchEvent(
      new CustomEvent('workspace:mounted', {
        detail: { appName: 'home', url: '/app/home/' }
      })
    );

    await new Promise((r) => setTimeout(r, 10));

    assert.ok(apiCalls2.some((u) => u.includes('/app/home/api/pulse')), 'pulse requested on /app/home');
    assert.ok(apiCalls2.some((u) => u.includes('/app/home/api/removals')), 'removals requested on /app/home');
    assert.ok(envHome.doc.querySelector('[data-removals-card]'), 'removals card mounted on /app/home');
  }

  function render(model) {
    const { doc, sandbox } = createEnvironment();
    const surface = doc.createElement('div');
    surface.setAttribute('data-pulse-surface', '');
    doc.body.appendChild(surface);
    sandbox.window.renderWeekPage(surface, model);
    return { doc, sandbox, surface };
  }

  function quoteTwin(intro) {
    const markup = '<b>x</b> [y](z)';
    return {
      day: '20260308',
      title: 'week of march 8',
      intro: intro,
      cells: [],
      legend: [],
      from_line: null,
      rows: [{
        key: 'mem',
        text: markup,
        day_label: 'sun 8',
        source_href: '/source?ref=mem',
        peek_href: '/source?ref=mem',
        peek_label: 'open it',
        peek_caption: 'from the morning',
        menu_href: '/source?ref=mem',
        left_out: false
      }],
      said: {
        heading: 'heading',
        leave_out_hint: 'hint',
        days: [{
          day_label: 'mon 9',
          rows: [{
            key: 'quote-key',
            quote: markup,
            href: '/source?ref=quote',
            link_label: 'open the quote',
            left_out: false
          }]
        }]
      },
      end_line: "that's the week."
    };
  }

  // Said section from a real page model sits after memory rows and before the end line.
  {
    caseCount++;
    const { surface } = render(saidModel);
    const dash = surface.querySelector('.week-dashboard').children;
    const rowsIdx = dash.findIndex((child) => child.classList.contains('week-rows'));
    const saidIdx = dash.findIndex((child) => child.classList.contains('week-said'));
    const endIdx = dash.findIndex((child) => child.classList.contains('week-end-line'));
    assert.ok(rowsIdx !== -1 && saidIdx !== -1 && endIdx !== -1 && rowsIdx < saidIdx && saidIdx < endIdx);
    const heading = surface.querySelector('.week-said-heading');
    assert.strictEqual(heading.tagName, 'H2');
    assert.strictEqual(heading.textContent, saidModel.said.heading);
    const shown = surface.querySelector('[data-memory-key="said-mon"]');
    const quote = shown.querySelector('.week-said-quote');
    assert.strictEqual(quote.querySelector('a'), null);
    assert.strictEqual(quote.querySelector('b'), null);
    assert.ok(quote.textContent.includes(saidModel.said.days[0].rows[0].quote));
    const link = shown.querySelector('.week-said-link');
    assert.strictEqual(link.getAttribute('href'), saidModel.said.days[0].rows[0].href);
    assert.strictEqual(link.textContent, saidModel.said.days[0].rows[0].link_label);
    assert.strictEqual(quote.querySelector('.week-said-link'), null);
    const menu = shown.querySelector('.week-menu-btn');
    assert.strictEqual(menu.getAttribute('aria-label'), 'more for ' + saidModel.said.days[0].day_label);
    const hint = shown.querySelector('.week-leave-out-btn .week-menu-hint');
    assert.strictEqual(hint.textContent, saidModel.said.leave_out_hint);
    const hidden = surface.querySelector('[data-memory-key="said-tue"]');
    assert.ok(hidden.classList.contains('week-row--left-out'));
    assert.ok(hidden.querySelector('[data-action="undo"]'));
  }

  // No said section when the model has none, and one intro when intro is a string.
  {
    caseCount++;
    const { surface } = render(primaryModel);
    assert.strictEqual(surface.querySelector('.week-said'), null);
    assert.strictEqual(surface.querySelector('.week-said-unreadable'), null);
    assert.strictEqual(surface.querySelectorAll('.week-intro').length, 1);
  }

  // Quote text stays literal. The same text in a memory row is markdown. Null intro emits no paragraph.
  {
    caseCount++;
    const markup = '<b>x</b> [y](z)';
    const { surface } = render(quoteTwin(null));
    assert.strictEqual(surface.querySelectorAll('.week-intro').length, 0);
    const quote = surface.querySelector('.week-said-quote');
    assert.strictEqual(quote.querySelector('a'), null);
    assert.strictEqual(quote.querySelector('b'), null);
    assert.ok(quote.textContent.includes(markup));
    assert.ok(quote.children.every((child) => child.nodeType === 3));
    const memory = surface.querySelector('.week-row-text');
    assert.ok(memory.querySelector('a'), 'memory row renders the link');
    const { surface: withIntro } = render(quoteTwin('intro line'));
    assert.strictEqual(withIntro.querySelectorAll('.week-intro').length, 1);
  }

  // Leave-out and undo on a quote row move focus the same way a memory row does.
  {
    caseCount++;
    const shown = JSON.parse(JSON.stringify(saidModel));
    const hidden = JSON.parse(JSON.stringify(saidModel));
    hidden.said.days[0].rows[0] = { left_out: true, key: 'said-mon' };
    const { doc, sandbox, surface } = render(shown);
    sandbox.fetch = (_url, opts) => {
      const body = JSON.parse(opts.body);
      const next = body.undo ? shown : hidden;
      return Promise.resolve({ ok: true, json: () => Promise.resolve(next) });
    };
    const leaveOutBtn = surface.querySelector('[data-memory-key="said-mon"] .week-leave-out-btn');
    leaveOutBtn.click();
    await new Promise((r) => setTimeout(r, 10));
    const undoBtn = surface.querySelector('[data-action="undo"][data-key="said-mon"]');
    assert.ok(undoBtn);
    assert.strictEqual(doc.activeElement, undoBtn);
    undoBtn.click();
    await new Promise((r) => setTimeout(r, 10));
    const menuBtn = surface.querySelector('[data-memory-key="said-mon"] .week-menu-btn');
    assert.ok(menuBtn);
    assert.strictEqual(doc.activeElement, menuBtn);
  }

  // A failed leave-out whose re-read already left the quote out re-renders without the unchanged line.
  {
    caseCount++;
    const shown = quoteTwin(null);
    const hidden = quoteTwin(null);
    hidden.said.days[0].rows[0] = { left_out: true, key: 'quote-key' };
    const { doc, sandbox, surface } = render(shown);
    sandbox.fetch = (_url, opts) => {
      if (opts && opts.method === 'POST') return Promise.reject(new Error('POST failed'));
      return Promise.resolve({ ok: true, json: () => Promise.resolve(hidden) });
    };
    surface.querySelector('[data-memory-key="quote-key"] .week-leave-out-btn').click();
    await new Promise((r) => setTimeout(r, 10));
    const undoBtn = surface.querySelector('[data-action="undo"][data-key="quote-key"]');
    assert.ok(undoBtn);
    assert.strictEqual(doc.activeElement, undoBtn);
    assert.ok(!surface.innerHTML.includes('nothing changed'));
  }

  // A failed leave-out whose re-read is unchanged puts the error on that quote row and returns focus.
  {
    caseCount++;
    const shown = quoteTwin('kept');
    const { doc, sandbox, surface } = render(shown);
    sandbox.fetch = (_url, opts) => {
      if (opts && opts.method === 'POST') return Promise.reject(new Error('POST failed'));
      return Promise.resolve({ ok: true, json: () => Promise.resolve(shown) });
    };
    const leaveOutBtn = surface.querySelector('[data-memory-key="quote-key"] .week-leave-out-btn');
    leaveOutBtn.focus();
    leaveOutBtn.click();
    await new Promise((r) => setTimeout(r, 10));
    const err = surface.querySelector('[data-memory-key="quote-key"] .week-row-error');
    assert.ok(err);
    assert.strictEqual(doc.activeElement, leaveOutBtn);
  }

  console.log(`DOM CASES: ${caseCount} passed`);
}

runTests().catch((err) => {
  console.error(err);
  process.exit(1);
});
