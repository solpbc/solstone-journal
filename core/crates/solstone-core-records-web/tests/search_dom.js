// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const manifestDir = process.argv[2];
assert.ok(manifestDir, 'manifest directory argument is required');

const dateFormatSource = fs.readFileSync(
  path.resolve(manifestDir, '../solstone-core-convey-shell/assets/static/date_format.js'),
  'utf8'
);
const searchHtml = fs.readFileSync(
  path.join(manifestDir, 'assets/search.html'),
  'utf8'
);

class Node {
  constructor(tagName) {
    this.tagName = tagName ? tagName.toUpperCase() : '';
    this.children = [];
    this.attributes = {};
    this._textContent = '';
    this.nodeType = tagName ? 1 : 3;
    this.childNodes = this.children;
  }
  get textContent() {
    if (this.children.length > 0) return this.children.map((c) => c.textContent).join('');
    return this._textContent;
  }
  set textContent(v) {
    this.children = [];
    this._textContent = String(v);
  }
  get className() {
    return this.attributes.class || '';
  }
  set className(v) {
    this.attributes.class = String(v);
  }
  setAttribute(k, v) {
    this.attributes[k] = String(v);
  }
  getAttribute(k) {
    return Object.prototype.hasOwnProperty.call(this.attributes, k) ? this.attributes[k] : null;
  }
  append(...nodes) {
    for (const n of nodes) {
      if (typeof n === 'string') {
        const textNode = new Node('');
        textNode.textContent = n;
        this.children.push(textNode);
      } else if (n && n.nodeType === 11) {
        this.children.push(...n.children);
      } else if (n) {
        this.children.push(n);
      }
    }
  }
  appendChild(child) {
    this.append(child);
    return child;
  }
  querySelectorAll(sel) {
    const results = [];
    const walk = (el) => {
      for (const child of el.children) {
        const isTag = child.tagName && child.tagName.toLowerCase() === sel.toLowerCase();
        const isClass =
          sel.startsWith('.') &&
          child.attributes.class &&
          child.attributes.class.split(' ').includes(sel.slice(1));
        if (isTag || isClass) results.push(child);
        walk(child);
      }
    };
    walk(this);
    return results;
  }
  querySelector(sel) {
    return this.querySelectorAll(sel)[0] || new Node('div');
  }
  addEventListener() {}
}

let cases = 0;
function test(name, fn) {
  cases += 1;
  try {
    fn();
  } catch (err) {
    console.error(`FAILED: ${name}`);
    throw err;
  }
}

function createSearchEnvironment() {
  const location = { search: '' };
  const windowObj = { location, addEventListener() {} };
  const doc = {
    createElement: (tag) => new Node(tag),
    createTextNode: (t) => {
      const n = new Node('');
      n.textContent = t;
      return n;
    },
    createDocumentFragment: () => {
      const f = new Node('');
      f.nodeType = 11;
      return f;
    },
    getElementById: () => new Node('div'),
  };

  class DOMParser {
    parseFromString(str) {
      const parsedDoc = { body: new Node('body') };
      parsedDoc.body.textContent = str.replace(/<[^>]+>/g, '');
      return parsedDoc;
    }
  }

  const context = vm.createContext({
    window: windowObj,
    document: doc,
    DOMParser,
    URLSearchParams,
    location,
    console,
    Math,
    Number,
    String,
    Object,
    Array,
    Boolean,
  });

  vm.runInContext(dateFormatSource, context);

  const scriptStart = searchHtml.indexOf('<script>') + '<script>'.length;
  const scriptEnd = searchHtml.indexOf('</script>', scriptStart);
  let script = searchHtml.slice(scriptStart, scriptEnd);
  script = script.replace(
    'function renderHit(hit,dayHref){',
    'window.renderHit = renderHit;\n function renderHit(hit,dayHref){'
  );
  vm.runInContext(script, context);

  return { context, window: windowObj };
}

// Seeded labels are synthetic and never contain the stream name; what a
// label says comes from the table's parts, the way the journal serves it.
function seededSearch() {
  const env = createSearchEnvironment();
  env.window.JournalFormat.installStreamLabels({
    status: 'complete',
    labels: { ja1r: 'LABEL-ONE', phone_audio: 'LABEL-TWO', 'import.oura': 'LABEL-THREE' },
    says: {
      ja1r: { what: 'screen', imported: false },
      phone_audio: { what: 'audio', imported: false },
      'import.oura': { what: null, imported: true },
    },
  });
  return env;
}

function metaSpans(env, hit) {
  const card = env.window.renderHit({ text: 'synthetic text', ts: 1775199600000, ...hit }, '/app/transcripts/20260403');
  return card.querySelector('.search-meta').querySelectorAll('span');
}

test('renderHit renders seeded stream label with raw name in title attribute', () => {
  const spans = metaSpans(seededSearch(), { stream: 'ja1r', facet_title: 'personal' });
  const streamSpan = spans.find((s) => s.getAttribute('title') === 'ja1r');
  assert.ok(streamSpan, 'stream span carries the stream name as its title');
  assert.strictEqual(streamSpan.textContent, 'LABEL-ONE');
});

test('renderHit drops an agent label the stream label already says, and keeps any other', () => {
  const env = seededSearch();
  const agentShown = (hit) => metaSpans(env, hit).some((s) => s.textContent === hit.agent_label);
  // The indexed agent is a file's basename: "screen", "audio", "imported",
  // or a talent such as "audio_transcript" or "event_transcript".
  assert.strictEqual(agentShown({ stream: 'ja1r', agent_label: 'Screen' }), false);
  assert.strictEqual(agentShown({ stream: 'phone_audio', agent_label: 'Audio' }), false);
  assert.strictEqual(agentShown({ stream: 'import.oura', agent_label: 'Imported' }), false);
  assert.strictEqual(agentShown({ stream: 'ja1r', agent_label: 'Audio' }), true);
  assert.strictEqual(agentShown({ stream: 'phone_audio', agent_label: 'Audio Transcript' }), true);
  assert.strictEqual(agentShown({ stream: 'import.oura', agent_label: 'Event Transcript' }), true);
  assert.strictEqual(agentShown({ stream: 'ja1r', agent_label: 'Describe' }), true);
});

test('screen-text continuation hits retain their segment citation and direct link', () => {
  const env = seededSearch();
  for (const prefix of ['', 'chronicle/']) {
    const card = env.window.renderHit({
      path: prefix + '20260403/ja1r/090000_300/talents/screen_text.md',
      stream: 'ja1r', segment: '090000_300', minute: '02:05',
      text: 'a continuation chunk without its original heading',
    }, '/app/transcripts/20260403');
    assert.strictEqual(card.querySelector('.search-citation').textContent, '090000_300 · 02:05');
    const links = card.querySelectorAll('a');
    assert.strictEqual(links.length, 1);
    assert.strictEqual(links[0].href, '/app/transcripts/20260403?stream=ja1r#090000_300');
  }
  const ordinary = env.window.renderHit({
    path: '20260403/ja1r/090000_300/screen.jsonl', stream: 'ja1r', text: 'ordinary',
  }, '/app/transcripts/20260403');
  assert.strictEqual(ordinary.querySelectorAll('.search-citation').length, 0);
  assert.strictEqual(ordinary.querySelectorAll('a')[0].href, '/app/transcripts/20260403?stream=ja1r#090000_300');
});

console.log(`DOM CASES: ${cases} passed`);
