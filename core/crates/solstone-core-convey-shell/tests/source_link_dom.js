// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');

const corpus = JSON.parse(
  fs.readFileSync(path.join(crateDir, 'tests/source_link_corpus.json'), 'utf8')
);

class Node {
  constructor(nodeType, nodeName) {
    this.nodeType = nodeType;
    this.nodeName = nodeName;
    this.parentElement = null;
    this.childNodes = [];
  }

  get parentNode() {
    return this.parentElement;
  }

  appendChild(child) {
    if (child.nodeType === 11) {
      // DocumentFragment
      const children = child.childNodes.slice();
      children.forEach((c) => this.appendChild(c));
      child.childNodes = [];
      return child;
    }
    if (child.parentElement) {
      child.parentElement.removeChild(child);
    }
    child.parentElement = this;
    this.childNodes.push(child);
    return child;
  }

  removeChild(child) {
    const idx = this.childNodes.indexOf(child);
    if (idx !== -1) {
      this.childNodes.splice(idx, 1);
      child.parentElement = null;
    }
    return child;
  }

  replaceWith(...nodes) {
    if (!this.parentElement) return;
    const parent = this.parentElement;
    const idx = parent.childNodes.indexOf(this);
    if (idx === -1) return;
    const toInsert = [];
    nodes.forEach((n) => {
      if (n.nodeType === 11) {
        toInsert.push(...n.childNodes);
        n.childNodes.forEach((c) => { c.parentElement = parent; });
        n.childNodes = [];
      } else {
        if (n.parentElement) n.parentElement.removeChild(n);
        n.parentElement = parent;
        toInsert.push(n);
      }
    });
    parent.childNodes.splice(idx, 1, ...toInsert);
    this.parentElement = null;
  }

  append(...nodes) {
    nodes.forEach((n) => this.appendChild(n));
  }
}

class TextNode extends Node {
  constructor(text) {
    super(3, '#text');
    this.textContent = String(text ?? '');
  }

  get nodeValue() {
    return this.textContent;
  }

  set nodeValue(val) {
    this.textContent = String(val ?? '');
  }
}

class DocumentFragment extends Node {
  constructor() {
    super(11, '#document-fragment');
  }
}

class Element extends Node {
  constructor(tagName) {
    super(1, tagName.toUpperCase());
    this.tagName = tagName.toUpperCase();
    this.attributes = {};
  }

  setAttribute(name, value) {
    this.attributes[name.toLowerCase()] = String(value);
  }

  getAttribute(name) {
    const lower = name.toLowerCase();
    return Object.prototype.hasOwnProperty.call(this.attributes, lower) ? this.attributes[lower] : null;
  }

  removeAttribute(name) {
    delete this.attributes[name.toLowerCase()];
  }

  hasAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name.toLowerCase());
  }

  get href() {
    return this.getAttribute('href') || '';
  }

  set href(val) {
    this.setAttribute('href', val);
  }

  get src() {
    return this.getAttribute('src') || '';
  }

  set src(val) {
    this.setAttribute('src', val);
  }

  get textContent() {
    return this.childNodes.map((c) => c.textContent).join('');
  }

  set textContent(text) {
    this.childNodes.forEach((c) => { c.parentElement = null; });
    this.childNodes = [];
    if (text !== '') {
      this.appendChild(new TextNode(text));
    }
  }

  closest(selector) {
    const tagNames = selector.split(',').map((s) => s.trim().toUpperCase());
    let current = this;
    while (current && current.nodeType === 1) {
      if (tagNames.includes(current.tagName)) {
        return current;
      }
      current = current.parentElement;
    }
    return null;
  }

  get innerHTML() {
    return serializeNodes(this.childNodes);
  }

  set innerHTML(html) {
    this.childNodes.forEach((c) => { c.parentElement = null; });
    this.childNodes = [];
    parseHtmlInto(html, this);
  }
}

function escapeHtmlText(str) {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function escapeHtmlAttr(str) {
  return str
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function serializeNodes(nodes) {
  let result = '';
  for (const node of nodes) {
    if (node.nodeType === 3) {
      result += node.textContent;
    } else if (node.nodeType === 1) {
      const tag = node.tagName.toLowerCase();
      let attrs = '';
      for (const [k, v] of Object.entries(node.attributes)) {
        attrs += ` ${k}="${escapeHtmlAttr(v)}"`;
      }
      const voidTags = ['img', 'br', 'hr', 'input'];
      if (voidTags.includes(tag) && node.childNodes.length === 0) {
        result += `<${tag}${attrs}>`;
      } else {
        result += `<${tag}${attrs}>${serializeNodes(node.childNodes)}</${tag}>`;
      }
    }
  }
  return result;
}

function parseHtmlInto(html, parent) {
  const tokenRegex = /<!--[\s\S]*?-->|<\/?([A-Za-z0-9-]+)((?:\s+[^\s=>/]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*\/?>|([^<]+)/g;
  let match;
  const stack = [parent];
  while ((match = tokenRegex.exec(html))) {
    if (match[3]) {
      // Text
      const text = match[3]
        .replace(/&amp;/g, '&')
        .replace(/&lt;/g, '<')
        .replace(/&gt;/g, '>')
        .replace(/&quot;/g, '"')
        .replace(/&#39;/g, "'");
      stack[stack.length - 1].appendChild(new TextNode(text));
    } else if (match[1]) {
      // Tag
      const isClosing = match[0].startsWith('</');
      const tagName = match[1].toLowerCase();
      if (isClosing) {
        const idx = stack.map((e) => e.tagName && e.tagName.toLowerCase()).lastIndexOf(tagName);
        if (idx > 0) {
          stack.length = idx;
        }
      } else {
        const el = new Element(tagName);
        const attrStr = match[2] || '';
        const attrRegex = /([^\s=>/]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
        let attrMatch;
        while ((attrMatch = attrRegex.exec(attrStr))) {
          const val = attrMatch[2] ?? attrMatch[3] ?? attrMatch[4] ?? '';
          const decodedVal = val
            .replace(/&amp;/g, '&')
            .replace(/&quot;/g, '"')
            .replace(/&#39;/g, "'")
            .replace(/&lt;/g, '<')
            .replace(/&gt;/g, '>');
          el.setAttribute(attrMatch[1], decodedVal);
        }
        stack[stack.length - 1].appendChild(el);
        const selfClosing = match[0].endsWith('/>') || ['img', 'br', 'hr', 'input'].includes(tagName);
        if (!selfClosing) {
          stack.push(el);
        }
      }
    }
  }
}

class TreeWalker {
  constructor(root, whatToShow) {
    this.root = root;
    this.whatToShow = whatToShow;
    this.nodes = [];
    this._collect(root);
    this._index = -1;
  }

  _collect(node) {
    if (node !== this.root) {
      if (this.whatToShow === 4 && node.nodeType === 3) {
        this.nodes.push(node);
      }
    }
    if (node.childNodes) {
      for (const child of node.childNodes) {
        this._collect(child);
      }
    }
  }

  nextNode() {
    this._index++;
    if (this._index < this.nodes.length) {
      this.currentNode = this.nodes[this._index];
      return this.currentNode;
    }
    this.currentNode = null;
    return null;
  }
}

const hooks = {
  uponSanitizeAttribute: [],
};

const DOMPurify = {
  addHook(name, fn) {
    if (!hooks[name]) hooks[name] = [];
    hooks[name].push(fn);
  },
  sanitize(dirtyHtml, options = {}) {
    const forbidTags = (options.FORBID_TAGS || []).map((t) => t.toUpperCase());
    const temp = new Element('div');
    temp.innerHTML = dirtyHtml;

    function sanitizeElement(el) {
      for (const child of el.childNodes.slice()) {
        if (child.nodeType === 1) {
          if (forbidTags.includes(child.tagName)) {
            el.removeChild(child);
            continue;
          }
          for (const attrName of Object.keys(child.attributes)) {
            const data = {
              attrName,
              attrValue: child.getAttribute(attrName),
              keepAttr: true,
            };
            for (const hook of hooks.uponSanitizeAttribute) {
              hook(child, data);
            }
            if (!data.keepAttr) {
              child.removeAttribute(attrName);
            } else {
              child.setAttribute(attrName, data.attrValue);
            }
          }
          sanitizeElement(child);
        }
      }
    }

    sanitizeElement(temp);
    return temp.innerHTML;
  },
};

const document = {
  createElement(tag) {
    return new Element(tag);
  },
  createTextNode(text) {
    return new TextNode(text);
  },
  createDocumentFragment() {
    return new DocumentFragment();
  },
  createTreeWalker(root, whatToShow) {
    return new TreeWalker(root, whatToShow);
  },
  addEventListener() {},
  removeEventListener() {},
  getElementById() { return null; },
  querySelector() { return null; },
  querySelectorAll() { return []; },
  head: { appendChild() {} },
  body: { appendChild() {} },
};

const window = {
  location: {
    href: 'http://localhost:8080/app/home/',
    origin: 'http://localhost:8080',
    protocol: 'http:',
  },
  document,
  DOMPurify,
  NodeFilter: { SHOW_TEXT: 4 },
  ConveyIcons: { svg() { return ''; } },
  convey: {},
  addEventListener() {},
  removeEventListener() {},
};
window.window = window;
document.defaultView = window;

const marked = require(path.join(crateDir, 'assets/static/vendor/marked/marked.min.js'));

const context = vm.createContext({
  window,
  document,
  DOMPurify,
  marked,
  URL,
  NodeFilter: { SHOW_TEXT: 4 },
  localStorage: { getItem() { return null; }, setItem() {} },
  navigator: {},
  console,
  Map,
  Set,
  Date,
  JSON,
  Promise,
  setTimeout,
  clearTimeout,
  setInterval,
  clearInterval,
});

const appJsSource = fs.readFileSync(
  path.join(crateDir, 'assets/static/app.js'),
  'utf8'
);
vm.runInContext(appJsSource, context, { filename: 'app.js' });

// 1. Test Sanitizer hook cases from corpus
for (const sc of corpus.sanitizer_cases) {
  const rendered = window.AppServices.renderMarkdown(sc.markdown);
  if (sc.id === 'sanitizer-href-rewritten') {
    assert.ok(rendered.includes(sc.expected_href), `${sc.id}: expected href in ${rendered}`);
  } else if (sc.id === 'sanitizer-src-stripped') {
    assert.ok(!rendered.includes('src="sol://'), `${sc.id}: src should be stripped: ${rendered}`);
    assert.ok(!rendered.includes('/source?ref='), `${sc.id}: src should not be rewritten: ${rendered}`);
  } else if (sc.id === 'sanitizer-non-href-denied') {
    assert.ok(!rendered.includes('/source?ref='), `${sc.id}: non-href should not be rewritten: ${rendered}`);
  }
}

// 2. Test Render bounds cases from corpus
for (const rc of corpus.render_cases) {
  const rendered = window.AppServices.renderMarkdown(rc.markdown);
  for (const expectedRef of rc.expected_refs) {
    const expectedHref = `/source?ref=${encodeURIComponent(expectedRef)}`;
    assert.ok(
      rendered.includes(expectedHref),
      `${rc.id}: expected ${expectedHref} in ${rendered}`
    );
  }
  if (rc.id === 'render-bounds-fenced-code') {
    assert.ok(!rendered.includes('/source?ref=' + encodeURIComponent('sol://20260901/100000_300')), 'fenced code unlinked');
  }
  if (rc.id === 'render-bounds-inline-code') {
    assert.ok(!rendered.includes('/source?ref=' + encodeURIComponent('sol://20260901/100000_300')), 'inline code unlinked');
  }
  if (rc.id === 'render-bounds-adjacent-http') {
    assert.ok(rendered.includes('https://example.com'), 'http link preserved');
  }
}

console.log('source_link_dom tests passed');
