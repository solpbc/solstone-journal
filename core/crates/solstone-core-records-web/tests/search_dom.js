// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { loadWindowCatalog } = require('../../solstone-core-convey-shell/tests/journal_catalog_dom.js');

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
  // Walked in a loop, not a closure: while the engine compiles a closure in
  // the background it keeps the closure's matches, and through `parentNode`
  // the tree they came from.
  querySelectorAll(sel) {
    const results = [];
    const pending = [this];
    while (pending.length) {
      const child = pending.pop();
      if (child !== this) {
        const isTag = child.tagName && child.tagName.toLowerCase() === sel.toLowerCase();
        const isClass =
          sel.startsWith('.') &&
          child.attributes.class &&
          child.attributes.class.split(' ').includes(sel.slice(1));
        if (isTag || isClass) results.push(child);
      }
      const kids = child.children;
      for (let i = kids.length - 1; i >= 0; i -= 1) pending.push(kids[i]);
    }
    return results;
  }
  querySelector(sel) {
    return this.querySelectorAll(sel)[0] || new Node('div');
  }
  addEventListener() {}
}

let cases = 0;
async function asyncTest(name, fn) {
  cases += 1;
  try {
    await fn();
  } catch (err) {
    console.error(`FAILED: ${name}`);
    throw err;
  }
}
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
  // The script only registers on the view contract; these cases call its
  // helpers directly.
  const windowObj = { SolViews: { register() {} }, location, addEventListener() {} };
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
    Intl,
    location,
    console,
    Math,
    Number,
    String,
    Object,
    Array,
    Boolean,
  });

  loadWindowCatalog(context);
  vm.runInContext(dateFormatSource, context);

  const scriptStart = searchHtml.indexOf('<script>') + '<script>'.length;
  const scriptEnd = searchHtml.indexOf('</script>', scriptStart);
  let script = searchHtml.slice(scriptStart, scriptEnd);
  script = script.replace(
    'function renderHit(hit,dayHref){',
    'window.renderHit = renderHit; window.matchesStatus = matchesStatus;\n function renderHit(hit,dayHref){'
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

test('a filed note replaces file-derived metadata and keeps the matched entry action', () => {
  const env = seededSearch();
  const hit = {
    day: '20260403', stream: 'import.text', path: '20260403/import.text/090000_300/note_transcript.md',
    agent_label: 'FILE-LABEL', segment: '090000_300', time: '09:00:00',
    text: 'filed text', ts: 1775199600000, entry_id: 21, idx: 0,
  };
  const reading = env.window.renderHit(hit, '/app/transcripts/20260403');
  const baseline = metaSpans(env, hit).map((span) => span.textContent);
  const item = { kind: 'notes', source: 'imported' };
  const note = env.window.renderHit({ ...hit, item }, '/app/transcripts/20260403');
  const metadata = note.querySelector('.search-meta').querySelectorAll('span').map((span) => span.textContent);
  assert.ok(baseline.includes(hit.agent_label));
  assert.ok(baseline.includes(hit.time));
  const clock = env.window.JournalFormat.time(hit.ts);
  assert.ok(baseline.includes(clock));
  assert.ok(!metadata.includes(clock));
  assert.ok(!metadata.includes(hit.time));
  assert.ok(!metadata.includes(hit.agent_label));
  assert.ok(metadata.includes(env.window.JournalFormat.importItem(item)));
  assert.ok(metadata.includes(env.window.JournalFormat.filedDay(hit.day)));
  assert.strictEqual(note.querySelector('.search-excerpt').textContent, reading.querySelector('.search-excerpt').textContent);
  assert.strictEqual(note.querySelector('button').getAttribute('aria-expanded'), 'false');

  const transcript = { kind: 'transcripts', source: 'imported' };
  const timed = metaSpans(env, { ...hit, item: transcript }).map((span) => span.textContent);
  assert.ok(timed.includes(clock));
  assert.ok(timed.includes(hit.time));
  assert.ok(!timed.includes(hit.agent_label));
});

test('the count line takes the one form for one match and one day, and the many form otherwise', () => {
  const env = createSearchEnvironment();
  const words = (line) => line.replace(/[0-9,.]/g, ' ').split(/\s+/).filter(Boolean);
  const one = words(env.window.matchesStatus(1, 1));
  assert.ok(one.includes('match') && one.includes('day'), one.join(' '));
  assert.ok(!one.includes('matches') && !one.includes('days'), one.join(' '));
  const many = words(env.window.matchesStatus(1200, 3));
  assert.ok(many.includes('matches') && many.includes('days'), many.join(' '));
});

test('screen-text continuation hits retain their segment citation and direct link', () => {
  const env = seededSearch();
  for (const prefix of ['', 'chronicle/']) {
    const card = env.window.renderHit({
      path: prefix + '20260403/ja1r/090000_300/talents/screen_text.md',
      stream: 'ja1r', segment: '090000_300', minute: '02:05', time: '09:02:05',
      text: 'a continuation chunk without its original heading',
    }, '/app/transcripts/20260403');
    // The citation is the time the server names.
    assert.strictEqual(card.querySelector('.search-citation').textContent, '09:02:05');
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

test('a message hit names its item, who wrote it and to whom, and opens the reader at the passage', () => {
  const env = seededSearch();
  const conversation = { file: 'conversation_transcript.jsonl', day: '20261117', first_sent_at: '2026-11-18T01:44:00Z', last_sent_at: '2026-11-18T01:44:00Z', source: 'ChatGPT', content_kind: 'messages' };
  const turn = 'conversation_transcript.jsonl#0-b479';
  const card = env.window.renderHit({
    day: '20261117', stream: 'import.chatgpt', agent_label: 'import.chatgpt',
    path: '20261117/import.chatgpt/184400_300/conversation_transcript.jsonl', idx: 0, entry_id: 2,
    text: 'the <strong>shelves</strong> arrive', ts: null, conversation_item: conversation,
    message: { file: 'conversation_transcript.jsonl', turn_id: turn, sent_at: '2026-11-18T01:44:00Z', is_owner: true, role: 'user', offset: 10109 },
  }, '/app/transcripts/20261117');
  const meta = card.querySelector('.search-meta').querySelectorAll('span').map((span) => span.textContent);
  assert.ok(meta.includes(env.window.JournalFormat.conversationItem(conversation)), meta.join(' | '));
  assert.ok(meta.includes(env.window.JournalFormat.time('2026-11-18T01:44:00Z')), meta.join(' | '));
  assert.ok(meta.includes(env.window.JournalI18n.render('journal.window.search.message.you')), meta.join(' | '));
  assert.ok(!meta.includes('import.chatgpt'));
  const links = card.querySelectorAll('a');
  assert.strictEqual(links.length, 1);
  assert.strictEqual(links[0].textContent, env.window.JournalI18n.render('journal.window.search.entry.open'));
  const href = new URL(links[0].href, 'https://journal.example');
  assert.strictEqual(href.pathname, '/app/transcripts/20261117');
  assert.strictEqual(href.searchParams.get('stream'), 'import.chatgpt');
  assert.strictEqual(href.searchParams.get('turn'), turn);
  assert.strictEqual(href.searchParams.get('at'), '10109');
  assert.strictEqual(href.hash, '#184400_300/transcript');
  assert.strictEqual(card.querySelectorAll('button').length, 0, 'a message opens in the reader, not as a stored record');
});

test('a clock-only message hit reads by date, names recipients, and promises no passage without an offset', () => {
  const env = seededSearch();
  const conversation = { file: 'conversation_transcript.jsonl', day: '20261117', untimed: true, source: 'imported', content_kind: 'messages' };
  const card = env.window.renderHit({
    day: '20261117', stream: 'import.text', path: '20261117/import.text/100200_300/conversation_transcript.jsonl', idx: 1, entry_id: 5,
    text: 'check the <strong>vent</strong>', ts: null, conversation_item: conversation, item: { kind: 'transcripts', source: 'imported' },
    message: { file: 'conversation_transcript.jsonl', turn_id: 't1', author: 'Mira Alder', to: ['Everyone'] },
  }, '/app/transcripts/20261117');
  const meta = card.querySelector('.search-meta').querySelectorAll('span').map((span) => span.textContent);
  assert.ok(meta.includes(env.window.JournalFormat.filedDay('20261117')), meta.join(' | '));
  assert.ok(meta.includes(env.window.JournalI18n.render('journal.window.search.message.meta', [
    ['sender', 'Mira Alder'], ['recipients', env.window.JournalI18n.render('journal.window.search.message.everyone')],
  ])), meta.join(' | '));
  const href = new URL(card.querySelectorAll('a')[0].href, 'https://journal.example');
  assert.strictEqual(href.searchParams.get('at'), null);
  assert.strictEqual(href.searchParams.get('turn'), 't1');
});

test('a conversation the journal cannot place is labelled so and keeps the stored entry action', () => {
  const env = seededSearch();
  const conversation = { file: 'conversation_transcript.jsonl', day: '20261117', untimed: true, source: 'imported', conversation_state: 'unresolved', reason_code: 'insufficient_evidence' };
  const card = env.window.renderHit({
    day: '20261117', stream: 'import.text', path: '20261117/import.text/130400_300/conversation_transcript.jsonl', idx: 0, entry_id: 9,
    text: '**Mira Alder:** who has the <strong>cart</strong>', ts: null, conversation_item: conversation, item: { kind: 'transcripts', source: 'imported' },
  }, '/app/transcripts/20261117');
  const meta = card.querySelector('.search-meta').querySelectorAll('span').map((span) => span.textContent);
  assert.ok(meta.includes(env.window.JournalFormat.conversationItem(conversation)), meta.join(' | '));
  assert.strictEqual(card.querySelectorAll('button').length, 1);
  assert.strictEqual(card.querySelector('.search-speaker').textContent, 'Mira Alder:');
});

test('a spoken hit names its speaker, never stored Markdown or caption voice markup', () => {
  const env = seededSearch();
  const card = (text) => env.window.renderHit({ day: '20261117', stream: 'import.text', path: '20261117/import.text/100000_300/conversation_transcript.jsonl', text }, '/app/transcripts/20261117');
  // This harness's parser keeps no text nodes, so the speaker element and the
  // excerpt's remaining source are what it can see.
  for (const text of ['**Mira Alder:** The kiln <strong>shelves</strong> arrive', '&lt;v Mira Alder&gt;The kiln <strong>shelves</strong> arrive&lt;/v&gt;']) {
    const speakers = card(text).querySelectorAll('.search-speaker');
    assert.strictEqual(speakers.length, 1, text);
    assert.strictEqual(speakers[0].textContent, 'Mira Alder:');
  }
  assert.strictEqual(card('no speaker <strong>here</strong>').querySelectorAll('.search-speaker').length, 0);
});

require('./search_contract_dom.js')({ manifestDir, searchHtml, dateFormatSource, test: asyncTest })
  .then(() => console.log(`DOM CASES: ${cases} passed`))
  .catch((err) => { console.error(err); process.exit(1); });
