// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const manifestDir = process.argv[2];
assert.ok(manifestDir, 'manifest directory argument is required');
const statePath = process.argv[3];
const summaryPath = process.argv[4];
assert.ok(statePath, 'state path argument is required');
assert.ok(summaryPath, 'summary path argument is required');

const dateFormatSource = fs.readFileSync(
  path.resolve(manifestDir, '../solstone-core-convey-shell/assets/static/date_format.js'),
  'utf8'
);
const curationEvidenceSource = fs.readFileSync(
  path.join(manifestDir, 'assets/curation/curation_evidence.js'),
  'utf8'
);

const workspaceHtml = fs.readFileSync(path.join(manifestDir, 'assets/curation/workspace.html'), 'utf8');
const workspaceScript = workspaceHtml.slice(
  workspaceHtml.lastIndexOf('<script>') + '<script>'.length,
  workspaceHtml.lastIndexOf('</script>')
);
const corpus = JSON.parse(fs.readFileSync(
  path.resolve(manifestDir, '../../fixtures/convey_facets_corpus.json'),
  'utf8'
));
const populatedState = corpus.phases.populated.find(
  (record) => record.path === '/app/curation/api/state' && record.status === 200
).json;

function escapeForTest(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function dataKey(attribute) {
  return attribute.slice(5).replace(/-([a-z])/g, (_, character) => character.toUpperCase());
}

function parseAttributes(source, element) {
  const pattern = /([^\s=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g;
  let match;
  while ((match = pattern.exec(source))) {
    element.setAttribute(match[1], match[2] ?? match[3] ?? match[4] ?? '');
  }
}

function parseHtml(html, root) {
  const stack = [root];
  const tags = /<\/?[^>]+>/g;
  const voidTags = new Set(['BR', 'HR', 'IMG', 'INPUT', 'SOURCE']);
  let previous = 0;
  let match;
  const text = (element, value) => {
    const trimmed = value.replace(/\s+/g, ' ').trim();
    const decoded = trimmed.replace(/&(amp|lt|gt|quot|#39);/g, (_, entity) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" })[entity]);
    if (trimmed) element.ownText = (element.ownText ? element.ownText + ' ' : '') + decoded;
  };
  while ((match = tags.exec(html))) {
    text(stack[stack.length - 1], html.slice(previous, match.index));
    previous = tags.lastIndex;
    const token = match[0];
    if (token.startsWith('<!')) continue;
    if (token.startsWith('</')) {
      const index = stack.map((element) => element.tagName).lastIndexOf(token.slice(2, -1).trim().toUpperCase());
      if (index > 0) stack.length = index;
      continue;
    }
    const parts = /^<\s*([^\s/>]+)([\s\S]*?)\/?>$/.exec(token);
    if (!parts) continue;
    const child = new Element(parts[1]);
    parseAttributes(parts[2], child);
    stack[stack.length - 1].appendChild(child);
    if (!token.endsWith('/>') && !voidTags.has(child.tagName)) stack.push(child);
  }
  text(stack[stack.length - 1], html.slice(previous));
}

function selectorMatches(element, selector) {
  const attributes = [...selector.matchAll(/\[([^\]=]+)(?:=["']?([^\]"']+)["']?)?\]/g)];
  const simple = selector.replace(/\[[^\]]+\]/g, '');
  const tag = simple.match(/^[a-zA-Z][\w-]*/);
  const classes = [...simple.matchAll(/\.([\w-]+)/g)];
  if (tag && element.tagName !== tag[0].toUpperCase()) return false;
  if (classes.some((match) => !element.className.split(/\s+/).includes(match[1]))) return false;
  return attributes.every((match) => element.hasAttribute(match[1])
    && (match[2] === undefined || element.getAttribute(match[1]) === match[2]));
}

function queryAll(root, selector) {
  if (selector.includes(',')) return selector.split(',').flatMap((part) => queryAll(root, part));
  let candidates = [root];
  for (const piece of selector.trim().split(/\s+/).filter((part) => part !== '>')) {
    const next = [];
    const visit = (element) => element.children.forEach((child) => {
      if (selectorMatches(child, piece)) next.push(child);
      visit(child);
    });
    candidates.forEach(visit);
    candidates = next;
  }
  return candidates;
}

class Element {
  constructor(tagName) {
    this.tagName = tagName.toUpperCase();
    this.attributes = {};
    this.children = [];
    this.parentElement = null;
    this.dataset = {};
    this.ownText = '';
    this.disabled = false;
  }
  get className() { return this.getAttribute('class') || ''; }
  set className(value) { this.setAttribute('class', value); }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(value) { if (value) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  get textContent() { return [this.ownText, ...this.children.map((child) => child.textContent)].filter(Boolean).join(' '); }
  set textContent(value) { this.children = []; this.ownText = String(value); }
  set innerHTML(html) { this.children = []; this.ownText = ''; parseHtml(String(html), this); }
  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name.startsWith('data-')) this.dataset[dataKey(name)] = String(value);
  }
  getAttribute(name) {
    if (name.startsWith('data-') && this.dataset[dataKey(name)] !== undefined) return this.dataset[dataKey(name)];
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }
  hasAttribute(name) { return this.getAttribute(name) !== null; }
  removeAttribute(name) {
    delete this.attributes[name];
    if (name.startsWith('data-')) delete this.dataset[dataKey(name)];
  }
  appendChild(child) { child.parentElement = this; this.children.push(child); return child; }
  remove() {
    if (!this.parentElement) return;
    this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
    this.parentElement = null;
  }
  matches(selector) { return selector.split(',').some((part) => selectorMatches(this, part.trim())); }
  closest(selector) {
    let current = this;
    while (current && !current.matches(selector)) current = current.parentElement;
    return current;
  }
  querySelector(selector) { return queryAll(this, selector)[0] || null; }
  querySelectorAll(selector) { return queryAll(this, selector); }
  addEventListener() {}
}

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

async function renderWorkspace(payload) {
  const root = new Element('section');
  root.setAttribute('data-curation-root', '');
  const windowObj = {
    AppServices: { escapeHtml: escapeForTest },
    Drawer: { render: () => '', preserveOpen: (_root, render) => render() },
    apiJson: async () => JSON.parse(JSON.stringify(payload)),
  };
  const context = vm.createContext({
    window: windowObj,
    document: { querySelector: (selector) => (selector === '[data-curation-root]' ? root : null) },
    console,
  });
  vm.runInContext(dateFormatSource, context);
  vm.runInContext(curationEvidenceSource, context);
  vm.runInContext(workspaceScript, context);
  for (let i = 0; i < 5; i += 1) await flush();
  return root;
}

// The served state still carries entity merge suggestions; the page sets them
// aside for the entities app. The rest of the state is what a populated journal
// returns, with the groups the page pages through filled in.
function stateWithEveryGroup() {
  return {
    ...populatedState,
    ambiguity_groups: [{
      candidate_ids: ['jordan-vance'],
      revision: 'r1',
      candidates: [{ id: 'jordan-vance', name: 'Jordan Vance' }],
      members: [{
        ambiguity_id: 'amb-1',
        normalized_query: 'jordan',
        query: 'Jordan',
        occurrence_count: 2,
        scope: { kind: 'journal' },
        origins: [],
      }],
    }],
    set_aside_items: [
      {
        kind: 'entity_merge_set_aside',
        key: 'work|golf|hotel',
        restore_id: 'work|golf|hotel',
        restore: { target: 'merge_candidate', facet: 'work', source_slug: 'golf', target_slug: 'hotel' },
        source: 'Golf',
        source_slug: 'golf',
        target: 'merge_candidate',
        target_name: 'Hotel',
        target_slug: 'hotel',
        reason: 'stale',
      },
      {
        kind: 'entity_ambiguity_set_aside',
        key: 'amb-2',
        restore_id: 'amb-2',
        restore: { target: 'ambiguity', ambiguity_id: 'amb-2' },
        target: 'ambiguity',
        query: 'Sam',
        reason: 'typo',
      },
    ],
    totals: { entity: 75, ambiguity: 1, set_aside: 2, pair: 1 },
  };
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

const asyncCases = [];
function asyncTest(name, fn) {
  asyncCases.push(async () => {
    cases += 1;
    try {
      await fn();
    } catch (err) {
      console.error(`FAILED: ${name}`);
      throw err;
    }
  });
}

function createCurationEnvironment() {
  const windowObj = {
    AppServices: { escapeHtml: (s) => String(s ?? '') },
    location: { search: '' },
    addEventListener() {},
  };
  const context = vm.createContext({
    window: windowObj,
    console,
    Math,
    Number,
    String,
    Object,
    Array,
    Boolean,
  });

  vm.runInContext(dateFormatSource, context);
  vm.runInContext(curationEvidenceSource, context);

  return { context, window: windowObj };
}

test('curation evidence linked sample renders seeded stream label with raw name in title attribute', () => {
  const env = createCurationEnvironment();
  env.window.JournalFormat.installStreamLabels({
    status: 'complete',
    labels: { ja1r: 'LABEL-ONE' },
  });

  const props = env.window.CurationEvidence.buildEvidenceDrawerProps({
    kind: 'activity',
    key: 'act-1',
    evidence: {
      count: 1,
      samples: [
        { day: '20260403', segment: '120000_300', stream: 'ja1r' },
      ],
    },
  });

  assert.ok(props, 'props object returned');
  const spanMatch = props.bodyHtml.match(/<span[^>]*class="ev-meta"[^>]*title="([^"]*)"[^>]*>([^<]*)<\/span>/);
  assert.ok(spanMatch, 'ev-meta span found in bodyHtml');
  assert.strictEqual(spanMatch[1], 'ja1r', 'title is raw stream name');
  assert.strictEqual(spanMatch[2], 'LABEL-ONE', 'span text is seeded label');
});

test('curation evidence plain sample renders seeded stream label with raw name in title attribute', () => {
  const env = createCurationEnvironment();
  env.window.JournalFormat.installStreamLabels({
    status: 'complete',
    labels: { ja1r: 'LABEL-ONE' },
  });

  const props = env.window.CurationEvidence.buildEvidenceDrawerProps({
    kind: 'entity',
    key: 'ent-1',
    evidence: {
      count: 1,
      samples: [
        { day: '20260403', stream: 'ja1r' }, // missing segment -> plainRow
      ],
    },
  });

  assert.ok(props, 'props object returned');
  const spanMatch = props.bodyHtml.match(/<span[^>]*title="([^"]*)"[^>]*>([^<]*)<\/span>/);
  assert.ok(spanMatch, 'span with title found in bodyHtml');
  assert.strictEqual(spanMatch[1], 'ja1r', 'title is raw stream name');
  assert.strictEqual(spanMatch[2], 'LABEL-ONE', 'span text is seeded label');
});

test('a speaker pair sample reads its label and keeps the raw stream name in technical details', () => {
  const html = fs.readFileSync(path.join(manifestDir, 'assets/curation/workspace.html'), 'utf8');
  const start = html.indexOf('  function speakerCandidatePairSampleHtml(sample) {');
  assert.ok(start > 0, 'test exercises the production sample renderer');
  let depth = 0;
  let end = html.indexOf('{', start);
  for (; end < html.length; end += 1) {
    if (html[end] === '{') depth += 1;
    if (html[end] === '}' && --depth === 0) break;
  }
  const env = createCurationEnvironment();
  env.window.JournalFormat.installStreamLabels({ status: 'complete', labels: { ja1r: 'LABEL-ONE' } });
  env.context.escapeHtml = (value) => String(value ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');
  env.context.sampleAudioHtml = () => '';
  vm.runInContext(html.slice(start, end + 1), env.context);
  const out = vm.runInContext('speakerCandidatePairSampleHtml({ day: "20260403", stream: "ja1r", segment_key: "120000_300" })', env.context);
  assert.ok(out.includes('<span title="ja1r">LABEL-ONE</span>'), 'the readable line uses the label');
  assert.ok(out.includes('<li>stream: ja1r</li>'), 'technical details keep the raw stream name');
});

asyncTest('entity merge suggestions do not render; every other group does', async () => {
  const state = stateWithEveryGroup();
  assert.ok(state.entity_items.length > 0, 'fixture carries entity merge suggestions');
  const root = await renderWorkspace(state);
  assert.strictEqual(root.querySelector('[data-review-group="merge"]'), null, 'no merge group');
  assert.deepStrictEqual(root.querySelectorAll('[data-kind="entity_merge"]'), [], 'no entity merge item');
  assert.deepStrictEqual(root.querySelectorAll('[data-curation-batch]'), [], 'no merge batch toolbar');
  const groups = root.querySelectorAll('[data-review-group]').map((group) => group.dataset.reviewGroup);
  assert.deepStrictEqual(groups, ['facet', 'ambiguity', 'speaker', 'pair', 'set_aside']);
  assert.strictEqual(root.querySelectorAll('[data-kind="facet_candidate"]').length, 2);
  assert.strictEqual(root.querySelectorAll('[data-kind="ambiguity_group"]').length, 1);
  assert.strictEqual(root.querySelectorAll('[data-kind="speaker_name_variant"]').length, 1);
  assert.strictEqual(root.querySelectorAll('[data-kind="speaker_candidate_pair"]').length, 1);
  const setAside = root.querySelector('[data-review-group="set_aside"]');
  assert.deepStrictEqual(
    setAside.querySelectorAll('[data-review-history]').map((item) => item.dataset.kind),
    ['entity_merge_set_aside', 'entity_ambiguity_set_aside'],
    'set-aside merge suggestions still offer restore'
  );
  assert.strictEqual(root.querySelector('.curation-empty'), null);
  assert.strictEqual(
    root.querySelector('[data-review-total]').textContent,
    '5 suggestions to review. choose a group to begin.',
    'the page total leaves entity merge suggestions out'
  );
});

asyncTest('a journal whose only open suggestions are entity merges has nothing to review', async () => {
  const state = {
    ...populatedState,
    facet_items: [],
    speaker_items: [],
    speaker_candidate_pair_items: [],
    totals: { entity: 75, ambiguity: 0, set_aside: 0, pair: 0 },
  };
  assert.ok(state.entity_items.length > 0, 'fixture carries entity merge suggestions');
  const root = await renderWorkspace(state);
  assert.deepStrictEqual(root.querySelectorAll('[data-review-group]'), []);
  assert.strictEqual(root.querySelector('.curation-empty').textContent, populatedState.copy.CUR_EMPTY_STATE);
  assert.strictEqual(root.querySelector('[data-review-total]').textContent, '');
});

asyncTest('suggestions count matches view total on seeded journal', async () => {
  const state = JSON.parse(fs.readFileSync(statePath, 'utf8'));
  const summary = JSON.parse(fs.readFileSync(summaryPath, 'utf8'));
  assert.ok(summary.to_review > 0, 'the seeded journal must have suggestions to review');
  const root = await renderWorkspace(state);
  const totalText = root.querySelector('[data-review-total]')?.textContent || '';
  const match = totalText.match(/^(\d+)/);
  assert.ok(match, 'view total must have leading integer');
  const viewCount = parseInt(match[1], 10);
  assert.strictEqual(viewCount, summary.to_review);
});

(async () => {
  for (const run of asyncCases) await run();
  console.log(`DOM CASES: ${cases} passed`);
})().catch((err) => {
  console.error(err);
  process.exit(1);
});
