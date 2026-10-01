// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = path.resolve(process.argv[2]);
assert.ok(crateDir, 'crate directory argument is required');

const corpus = JSON.parse(
  fs.readFileSync(path.join(crateDir, 'tests/source_link_corpus.json'), 'utf8')
);

const knownCategories = new Set([
  'render_cases',
  'sanitizer_cases',
  'segment_cases',
  'newsletter_cases',
  'activity_cases',
  'run_cases',
  'cant_show_cases',
  'containment_cases',
  'cant_open_page_cases',
]);

for (const key of Object.keys(corpus)) {
  assert.ok(knownCategories.has(key), `unknown corpus category: ${key}`);
}

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
        n.childNodes.forEach((c) => {
          c.parentElement = parent;
        });
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

  querySelectorAll(selector) {
    const target = selector.toUpperCase();
    const results = [];
    function walk(n) {
      if (n.nodeType === 1 && n.tagName === target) {
        results.push(n);
      }
      if (n.childNodes) {
        n.childNodes.forEach(walk);
      }
    }
    if (this.childNodes) {
      this.childNodes.forEach(walk);
    }
    return results;
  }

  get innerHTML() {
    return this.childNodes
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
    return Object.prototype.hasOwnProperty.call(this.attributes, lower)
      ? this.attributes[lower]
      : null;
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
    this.childNodes.forEach((c) => {
      c.parentElement = null;
    });
    this.childNodes = [];
    if (text !== '') {
      this.appendChild(new TextNode(text));
    }
  }

  closest(selector) {
    const targets = selector.split(',').map((s) => s.trim().toUpperCase());
    let curr = this;
    while (curr) {
      if (targets.includes(curr.tagName)) return curr;
      curr = curr.parentElement;
    }
    return null;
  }

  get innerHTML() {
    return super.innerHTML;
  }

  set innerHTML(html) {
    this.childNodes.forEach((c) => {
      c.parentElement = null;
    });
    this.childNodes = [];
    if (!html) return;
    const parsed = parseMockHtml(html);
    parsed.childNodes.slice().forEach((c) => this.appendChild(c));
  }
}

function parseMockHtml(html) {
  const frag = new DocumentFragment();
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
      currentParent.appendChild(new TextNode(textContent));
    } else if (isClosing) {
      if (stack.length > 1) {
        stack.pop();
        currentParent = stack[stack.length - 1];
      }
    } else {
      const elem = new Element(tagName);
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
      if (!['img', 'br', 'hr', 'input'].includes(tagName.toLowerCase())) {
        stack.push(elem);
        currentParent = elem;
      }
    }
  }
  return frag;
}

const window = {
  location: {
    href: 'http://localhost:8080/app/home/',
    pathname: '/app/home/',
    origin: 'http://localhost:8080',
    protocol: 'http:',
  },
  ConveyIcons: {
    svg: (name) => `<svg data-icon="${name}"></svg>`,
  },
  addEventListener: () => {},
  removeEventListener: () => {},
};

const document = {
  body: new Element('body'),
  createElement(tag) {
    return new Element(tag);
  },
  createTextNode(text) {
    return new TextNode(text);
  },
  createDocumentFragment() {
    return new DocumentFragment();
  },
  addEventListener: () => {},
  removeEventListener: () => {},
  getElementById: (id) => null,
  querySelector: () => null,
  querySelectorAll: () => [],
  referrer: '',
  createTreeWalker(root, _whatToShow) {
    const textNodes = [];
    function collect(n) {
      if (n.nodeType === 3) {
        textNodes.push(n);
      } else if (n.childNodes) {
        n.childNodes.forEach(collect);
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
};

const marked = require(path.join(crateDir, 'assets/static/vendor/marked/marked.min.js'));
const DOMPurify = {
  hooks: {},
  addHook(name, fn) {
    this.hooks[name] = this.hooks[name] || [];
    this.hooks[name].push(fn);
  },
  sanitize(html, _options) {
    const frag = parseMockHtml(html);
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
      if (n.childNodes) {
        n.childNodes.slice().forEach(checkNode);
      }
    }
    checkNode(frag);
    return frag.innerHTML;
  }
};

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

const sourceLinkBackSrc = fs.readFileSync(
  path.join(crateDir, 'assets/static/source_link_back.js'),
  'utf8'
);
vm.runInContext(sourceLinkBackSrc, context, { filename: 'source_link_back.js' });

let caseCount = 0;

// 1. Test Sanitizer hook cases from corpus
for (const sc of corpus.sanitizer_cases) {
  caseCount++;
  const rendered = window.AppServices.renderMarkdown(sc.markdown);
  if (sc.id === 'sanitizer-href-rewritten') {
    assert.ok(rendered.includes(`href="${sc.expected_href}"`), `${sc.id}: expected href attribute in ${rendered}`);
  } else if (sc.id === 'sanitizer-src-stripped' || sc.id === 'sanitizer-markdown-image') {
    assert.ok(!rendered.includes('src="sol://'), `${sc.id}: src should be stripped: ${rendered}`);
    assert.ok(!rendered.includes('/source?ref='), `${sc.id}: src should not be rewritten: ${rendered}`);
  } else if (sc.id === 'sanitizer-non-href-denied') {
    assert.ok(!rendered.includes('/source?ref='), `${sc.id}: non-href should not be rewritten: ${rendered}`);
  }
}

// 2. Test Render bounds cases from corpus
for (const rc of corpus.render_cases) {
  caseCount++;
  const actualSpans = window.AppServices.findSolSourceSpans(rc.markdown);
  assert.strictEqual(
    actualSpans.length,
    rc.spans.length,
    `${rc.id}: spans count mismatch: got ${actualSpans.length}, expected ${rc.spans.length}`
  );
  for (let i = 0; i < actualSpans.length; i++) {
    const actual = actualSpans[i];
    const expected = rc.spans[i];
    assert.strictEqual(actual.start, expected.start, `${rc.id} span ${i} start`);
    assert.strictEqual(actual.end, expected.end, `${rc.id} span ${i} end`);
    assert.strictEqual(actual.reference, expected.reference, `${rc.id} span ${i} reference`);
    const actualHref = window.AppServices.buildSolSourceHref(actual.reference);
    assert.strictEqual(actualHref, expected.href, `${rc.id} span ${i} href`);
  }

  const rendered = window.AppServices.renderMarkdown(rc.markdown);
  for (const span of rc.spans) {
    const attr = `href="${span.href}"`;
    if (span.linked) {
      assert.ok(rendered.includes(attr), `${rc.id}: expected linked attr ${attr} in ${rendered}`);
    } else {
      assert.ok(!rendered.includes(attr), `${rc.id}: unlinked span must not have attr ${attr} in ${rendered}`);
    }
  }
  if (rc.id === 'render-bounds-adjacent-http') {
    assert.ok(rendered.includes('https://example.com'), 'http link preserved');
  }
  if (rc.id === 'render-bounds-trailing-punct') {
    const matches = (rendered.match(/>moment</g) || []).length;
    assert.strictEqual(matches, 3, `expected 3 >moment< in ${rendered}`);
    assert.ok(!rendered.includes('moment 1'), `expected no moment 1 in ${rendered}`);
  }
}

// 3. Test sourceLinkBackHref function
caseCount++;
const pageUrl = 'http://localhost:8080/source';
assert.strictEqual(context.sourceLinkBackHref('http://localhost:8080/app/news/work/20260901', pageUrl), 'back');
assert.strictEqual(context.sourceLinkBackHref('http://localhost:8080/app/home/', pageUrl), 'back');
assert.strictEqual(context.sourceLinkBackHref('', pageUrl), '/app/home/');
assert.strictEqual(context.sourceLinkBackHref(null, pageUrl), '/app/home/');
assert.strictEqual(context.sourceLinkBackHref('https://external.example/path', pageUrl), '/app/home/');
assert.strictEqual(context.sourceLinkBackHref('not a valid url', pageUrl), '/app/home/');

// 4. Test renderMarkdown autolinks and hrefs across corpus categories
for (const [catName, cases] of Object.entries(corpus)) {
  if (catName === 'render_cases' || catName === 'sanitizer_cases') continue;
  for (const item of cases) {
    if (!item.ref || !item.ref.startsWith('sol://')) continue;
    if (item.ref.split('').some((c) => c.charCodeAt(0) < 32)) continue;
    caseCount++;
    const rendered = window.AppServices.renderMarkdown('<' + item.ref + '>');
    let expectedText = 'source';
    if (item.status !== 400) {
      if (catName === 'segment_cases' || catName === 'activity_cases') {
        expectedText = 'moment';
      } else if (catName === 'newsletter_cases') {
        expectedText = 'newsletter';
      }
    }
    const expectedHref = '/source?ref=' + encodeURIComponent(item.ref);
    assert.ok(
      rendered.includes(`>${expectedText}<`),
      `${catName} ${item.id}: expected >${expectedText}< in autolink rendering: ${rendered}`
    );
    assert.ok(
      rendered.includes(`href="${expectedHref}"`),
      `${catName} ${item.id}: expected href="${expectedHref}" in autolink rendering: ${rendered}`
    );
  }
}

// 5. Test renderMarkdown classified labels and repeated-label runs
caseCount++;
const singleMoment = window.AppServices.renderMarkdown('See sol://20260901/100000_300 for details.');
assert.ok(singleMoment.includes('>moment<'), `single moment label in ${singleMoment}`);

caseCount++;
const separateMomentRuns = window.AppServices.renderMarkdown('Check sol://20260901/100000_300 and sol://20260901/100500_300.');
assert.ok(separateMomentRuns.includes('>moment<'), `unrepeated moment in separated runs: ${separateMomentRuns}`);
assert.ok(!separateMomentRuns.includes('moment 1'), `no moment 1 in separated runs: ${separateMomentRuns}`);

caseCount++;
const parentheticalMoments = window.AppServices.renderMarkdown('(sol://20260901/100000_300; sol://20260901/100500_300)');
assert.ok(parentheticalMoments.includes('>moment 1<'), `repeated moment 1 in ${parentheticalMoments}`);
assert.ok(parentheticalMoments.includes('>moment 2<'), `repeated moment 2 in ${parentheticalMoments}`);

caseCount++;
const parentheticalNewsletters = window.AppServices.renderMarkdown(
  'Newsletters: (sol://facets/montague/news/20260310; sol://facets/verona/news/20260310) and sol://20260901/100000_300'
);
assert.ok(parentheticalNewsletters.includes('>newsletter 1<'), `newsletter 1 in ${parentheticalNewsletters}`);
assert.ok(parentheticalNewsletters.includes('>newsletter 2<'), `newsletter 2 in ${parentheticalNewsletters}`);
assert.ok(parentheticalNewsletters.includes('>moment<'), `trailing lone moment in ${parentheticalNewsletters}`);
assert.ok(!parentheticalNewsletters.includes('moment 1'), `trailing lone moment not numbered in ${parentheticalNewsletters}`);

caseCount++;
const mixedLabels = window.AppServices.renderMarkdown('Check sol://20260901/100000_300 and sol://facets/work/news/20260901.');
assert.ok(mixedLabels.includes('>moment<'), `unrepeated moment in mixed: ${mixedLabels}`);
assert.ok(mixedLabels.includes('>newsletter<'), `unrepeated newsletter in mixed: ${mixedLabels}`);

caseCount++;
const autolink = window.AppServices.renderMarkdown('<sol://20260901/100000_300>');
assert.ok(autolink.includes('>moment<'), `autolink classified label in ${autolink}`);

caseCount++;
const autolinkBracket = window.AppServices.renderMarkdown('[sol://20260901/100000_300](sol://20260901/100000_300)');
assert.ok(autolinkBracket.includes('>moment<'), `bracket autolink anchor text is moment in ${autolinkBracket}`);

caseCount++;
const customLink = window.AppServices.renderMarkdown('[Custom Title](sol://20260901/100000_300)');
assert.ok(customLink.includes('>Custom Title<'), `custom markdown link label in ${customLink}`);

console.log(`DOM CASES: ${caseCount} passed`);
