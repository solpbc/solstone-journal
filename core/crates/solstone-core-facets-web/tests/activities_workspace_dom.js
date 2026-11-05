// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The activities page under plain Node with a stand-in DOM: the panel's
// "that's not right" corrections, their posted bodies, and history remove.
// Cases assert copy keys and request bodies, never copy text.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');

const pageHtml = fs.readFileSync(path.join(crateDir, 'assets/activities/workspace.html'), 'utf8');
const markup = pageHtml.slice(pageHtml.indexOf('</style>') + '</style>'.length, pageHtml.indexOf('<script>'));
const pageScript = pageHtml.slice(pageHtml.indexOf('<script>') + '<script>'.length, pageHtml.lastIndexOf('</script>'));
const staticDir = path.join(crateDir, '../solstone-core-convey-shell/assets/static');
const apiSource = fs.readFileSync(path.join(staticDir, 'api.js'), 'utf8');
const drawerSource = fs.readFileSync(path.join(staticDir, 'drawer.js'), 'utf8');

class ClassList {
  constructor(element) { this.element = element; this.values = new Set(); }
  setFromString(value) { this.values = new Set(String(value || '').split(/\s+/).filter(Boolean)); }
  sync() { this.element.attributes.class = Array.from(this.values).join(' '); }
  add(...names) { names.forEach((n) => this.values.add(n)); this.sync(); }
  remove(...names) { names.forEach((n) => this.values.delete(n)); this.sync(); }
  toggle(name, force) {
    const present = force === undefined ? !this.values.has(name) : Boolean(force);
    if (present) this.values.add(name); else this.values.delete(name);
    this.sync();
    return present;
  }
  contains(name) { return this.values.has(name); }
}

function selectorMatches(element, selector) {
  const attributes = [...selector.matchAll(/\[([^\]=]+)(?:=["']?([^\]"']*)["']?)?\]/g)];
  const simple = selector.replace(/\[[^\]]+\]/g, '');
  const tag = simple.match(/^[a-zA-Z][\w-]*/);
  const id = simple.match(/#([\w-]+)/);
  const classes = [...simple.matchAll(/\.([\w-]+)/g)];
  if (tag && element.tagName !== tag[0].toUpperCase()) return false;
  if (id && element.getAttribute('id') !== id[1]) return false;
  if (classes.some((m) => !element.classList.contains(m[1]))) return false;
  return attributes.every((m) => element.hasAttribute(m[1]) && (m[2] === undefined || element.getAttribute(m[1]) === m[2]));
}

function matchesSelector(element, selector) {
  return selector.split(',').some((part) => selectorMatches(element, part.trim()));
}

function queryAll(root, selector) {
  if (selector.includes(',')) {
    const seen = new Set();
    return selector.split(',').flatMap((part) => queryAll(root, part.trim())).filter((el) => !seen.has(el) && seen.add(el));
  }
  let candidates = [root];
  for (const piece of selector.trim().split(/\s+/)) {
    const next = [];
    for (const candidate of candidates) {
      const visit = (element) => {
        for (const child of element.children) {
          if (matchesSelector(child, piece) && !next.includes(child)) next.push(child);
          visit(child);
        }
      };
      visit(candidate);
    }
    candidates = next;
  }
  return candidates;
}

const VOID = new Set(['input', 'br', 'hr', 'img', 'source']);

function parseHtml(html, doc) {
  const frag = new Element('div', doc);
  const stack = [frag];
  const tagRegex = /<(\/)?([a-zA-Z0-9]+)([^>]*)>|([^<]+)/g;
  let match;
  while ((match = tagRegex.exec(html)) !== null) {
    const parent = stack[stack.length - 1];
    if (match[4]) {
      parent._text += match[4];
    } else if (match[1]) {
      if (stack.length > 1) stack.pop();
    } else {
      const elem = new Element(match[2], doc);
      const attrRegex = /([a-zA-Z0-9_\-:]+)(?:=(?:"([^"]*)"|'([^']*)'|([^>\s]+)))?/g;
      let a;
      while ((a = attrRegex.exec(match[3] || '')) !== null) elem.setAttribute(a[1], a[2] ?? a[3] ?? a[4] ?? '');
      parent.appendChild(elem);
      if (!VOID.has(match[2].toLowerCase()) && !/\/\s*$/.test(match[3] || '')) stack.push(elem);
    }
  }
  return frag.children.slice();
}

class Element {
  constructor(tagName, doc) {
    this.tagName = tagName.toUpperCase();
    this.ownerDocument = doc;
    this.attributes = {};
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.classList = new ClassList(this);
    this.style = {};
    this._text = '';
    this.currentTime = 0;
  }
  get id() { return this.getAttribute('id') || ''; }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(v) { if (v) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  get disabled() { return this.hasAttribute('disabled'); }
  set disabled(v) { if (v) this.setAttribute('disabled', ''); else this.removeAttribute('disabled'); }
  get checked() { return this._checked !== undefined ? this._checked : this.hasAttribute('checked'); }
  set checked(v) { this._checked = Boolean(v); }
  get open() { return this._open !== undefined ? this._open : this.hasAttribute('open'); }
  set open(v) { this._open = Boolean(v); }
  get value() {
    if (this._value !== undefined) return this._value;
    return this.tagName === 'TEXTAREA' ? this.textContent : (this.getAttribute('value') ?? '');
  }
  set value(v) { this._value = String(v); }
  get textContent() { return this._text + this.children.map((c) => c.textContent).join(''); }
  // Replacing content detaches the old children, as in a browser.
  detachChildren() { this.children.forEach((c) => { c.parentElement = null; }); this.children = []; }
  set textContent(v) { this.detachChildren(); this._text = String(v); }
  get innerHTML() { return this._html ?? this.textContent; }
  set innerHTML(v) {
    this._html = String(v);
    this.detachChildren();
    this._text = '';
    parseHtml(String(v), this.ownerDocument).forEach((child) => this.appendChild(child));
  }
  get nextElementSibling() {
    if (!this.parentElement) return null;
    const siblings = this.parentElement.children;
    return siblings[siblings.indexOf(this) + 1] || null;
  }
  setAttribute(name, value) { this.attributes[name] = String(value); if (name === 'class') this.classList.setFromString(value); }
  getAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null; }
  hasAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attributes, name); }
  removeAttribute(name) { delete this.attributes[name]; if (name === 'class') this.classList.setFromString(''); }
  appendChild(child) { child.parentElement = this; this.children.push(child); return child; }
  remove() {
    if (!this.parentElement) return;
    this.parentElement.children.splice(this.parentElement.children.indexOf(this), 1);
    this.parentElement = null;
  }
  querySelector(sel) { return queryAll(this, sel)[0] || null; }
  querySelectorAll(sel) { return queryAll(this, sel); }
  matches(sel) { return matchesSelector(this, sel); }
  closest(sel) {
    for (let cur = this; cur && cur.matches; cur = cur.parentElement) if (cur.matches(sel)) return cur;
    return null;
  }
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
  removeEventListener(type, fn) { this.listeners[type] = (this.listeners[type] || []).filter((l) => l !== fn); }
  dispatchEvent(event) {
    // As in a browser, the path is fixed when dispatch starts: the event
    // still bubbles to the document, and composedPath() still names the
    // tapped element's ancestors, after a handler detaches it.
    const eventPath = [];
    for (let cur = this; cur; cur = cur.parentElement) eventPath.push(cur);
    if (eventPath[eventPath.length - 1] === this.ownerDocument.documentElement) eventPath.push(this.ownerDocument);
    event.target = this;
    event.composedPath = () => eventPath.slice();
    event.preventDefault ||= () => { event.defaultPrevented = true; };
    event.stopPropagation ||= () => { event.cancelBubble = true; };
    for (const node of event.bubbles ? eventPath : [this]) {
      if (event.cancelBubble) break;
      event.currentTarget = node;
      for (const fn of [...(node.listeners[event.type] || [])]) fn.call(node, event);
    }
    return !event.defaultPrevented;
  }
  focus() { this.ownerDocument.activeElement = this; }
  play() { this.playing = true; return Promise.resolve(); }
  pause() { this.playing = false; this.dispatchEvent({ type: 'pause' }); }
  scrollIntoView() {}
}

class Document {
  constructor() {
    this.readyState = 'complete';
    this.documentElement = new Element('html', this);
    this.body = new Element('body', this);
    this.documentElement.appendChild(this.body);
    this.activeElement = this.body;
    this.listeners = {};
  }
  createElement(tag) { return new Element(tag, this); }
  querySelector(sel) { return this.documentElement.querySelector(sel); }
  querySelectorAll(sel) { return this.documentElement.querySelectorAll(sel); }
  getElementById(id) { return this.querySelector('#' + id); }
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
  removeEventListener(type, fn) { this.listeners[type] = (this.listeners[type] || []).filter((l) => l !== fn); }
  dispatchEvent(event) {
    event.currentTarget = this;
    for (const fn of [...(this.listeners[event.type] || [])]) fn.call(this, event);
    return true;
  }
}

const DAY = '20261029';
const AT = '2026-10-30T09:00:00Z';

function activity(overrides = {}) {
  return {
    id: 'standup',
    facet: 'work',
    title: 'Standup',
    description: 'Daily standup',
    details: '',
    playback: false,
    corrected: false,
    updating: false,
    kind: 'detected',
    activity_label: 'meeting',
    start_min: 600,
    end_min: 615,
    start: '10:00',
    end: '10:15',
    segments: [{ key: '100000_300', start: '10:00', start_min: 600, seconds: 300, stream: 'room' }],
    stream: 'room',
    mixed: false,
    concurrent: false,
    hidden: false,
    cancelled: false,
    found_at: AT,
    edited_by_owner: false,
    summary: 'They agreed to cancel the contract.',
    people: ['Benvolio'],
    calendar: [],
    place: null,
    edits: [],
    ...overrides,
  };
}

function view(items) {
  return { items, facets: [{ name: 'work', title: 'Work', color: '', count: items.length, muted: false }], hidden_count: 0 };
}

function audioLine(sentence, words, extra = {}) {
  return {
    type: 'audio',
    time: `00:00:0${sentence}`,
    timestamp: Date.UTC(2026, 9, 29, 10, 0, sentence),
    markdown: words,
    sentence_id: sentence,
    speaker_source: 'audio',
    audio_url: '/app/transcripts/api/serve_file/20261029/room/100000_300/audio.flac',
    audio_origin: Date.UTC(2026, 9, 29, 10, 0, 0),
    ...extra,
  };
}

const SPOKEN = {
  speaker_labels: { present: true, loaded: true, source: 'audio', ambiguous: false, who_spoke_present: true },
  warning_details: [],
  chunks: [
    audioLine(1, 'we should cancel the vendor contract today', { speaker_label: { name: 'you', entity_id: 'owner', is_owner: true, confidence_state: 'high' } }),
    audioLine(2, 'let me get a second quote first', { speaker_label: { name: 'Benvolio', entity_id: 'benvolio', is_owner: false, confidence_state: 'high' } }),
    { type: 'screen', time: '00:00:03', timestamp: 0, markdown: 'a slide' },
  ],
};

const SCREEN_ONLY = { speaker_labels: { present: false, loaded: false, source: null, ambiguous: false }, chunks: [{ type: 'screen', markdown: 'code' }] };

const PEOPLE = {
  entities: [
    { id: 'romeo', entity_id: 'romeo', name: 'Romeo', type: 'Person', aka: [], is_principal: false, blocked: false },
    { id: 'me', entity_id: 'me', name: 'Me', type: 'Person', aka: [], is_principal: true, blocked: false },
    { id: 'tybalt', entity_id: 'tybalt', name: 'Tybalt', type: 'Person', aka: [], is_principal: false, blocked: true },
    { id: 'verona', entity_id: 'verona', name: 'Verona', type: 'Place', aka: [], is_principal: false, blocked: false },
  ],
};

function respond(status, body) {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
}

async function settle() {
  for (let i = 0; i < 6; i += 1) await new Promise((r) => setTimeout(r, 0));
}

async function createPage({ items, url = `https://journal.example/app/activities/${DAY}#work/standup`, transcripts = {}, post = () => respond(200, { record: {}, markdown: '', asked: true }), narrow = false }) {
  const doc = new Document();
  parseHtml(markup, doc).forEach((el) => doc.body.appendChild(el));
  const requests = [];
  const env = { view: view(items), requests, doc };
  const winListeners = {};
  const win = { createViewReady: () => () => {},
    addEventListener(type, fn) { (winListeners[type] ||= []).push(fn); },
    removeEventListener() {},
    dispatchEvent(event) { (winListeners[event.type] || []).forEach((fn) => fn(event)); },
    matchMedia: (query) => ({ matches: narrow && query.includes('max-width'), addEventListener() {} }),
    JournalClock: {
      today: () => '20261031',
      parts: (d = new Date()) => ({
        year: String(d.getUTCFullYear()), month: String(d.getUTCMonth() + 1).padStart(2, '0'), day: String(d.getUTCDate()).padStart(2, '0'),
        hour: String(d.getUTCHours()).padStart(2, '0'), minute: String(d.getUTCMinutes()).padStart(2, '0'), second: String(d.getUTCSeconds()).padStart(2, '0'),
      }),
    },
    JournalFormat: { hasStreamLabel: () => false, stream: (s) => s },
    ConveyIcons: { svg: (name) => `<svg data-icon="${name}"></svg>` },
    SurfaceState: { loading: () => '<div></div>', error: () => '<div></div>', replaceLoading() {} },
    CONVEY_COPY: {},
  };
  let href = url;
  win.location = new URL(href);
  win.history = {
    pushState(_s, _t, next) { win.location = new URL(next, win.location.href); },
    replaceState(_s, _t, next) { win.location = new URL(next, win.location.href); },
  };
  const fetchMock = async (target, opts = {}) => {
    const u = String(target);
    const method = opts.method || 'GET';
    const headers = opts.headers;
    const record = {
      url: u,
      method,
      actor: headers && typeof headers.get === 'function' ? headers.get('x-solstone-actor') : null,
      body: opts.body ? JSON.parse(opts.body) : null,
    };
    requests.push(record);
    if (method === 'POST') return post(record);
    if (u.startsWith(`/app/activities/api/day/${DAY}/view`)) return respond(200, env.view);
    if (u.startsWith('/app/transcripts/api/segment/')) {
      const key = decodeURIComponent(u.split('/').pop());
      return respond(200, transcripts[key] || SCREEN_ONLY);
    }
    if (u === '/app/entities/api/journal') return respond(200, PEOPLE);
    return respond(404, { error: 'not found' });
  };
  const sandbox = {
    window: win, document: doc, fetch: fetchMock, Headers, Response, URL, URLSearchParams, console,
    // Long timers (the updating poll) never fire in a case; short ones run.
    setTimeout: (fn, ms) => (ms && ms >= 1000 ? 0 : setTimeout(fn, ms)),
    clearTimeout: () => {},
    Promise, JSON, Math, Date, Number, String, Array, Object, Set, Map, Error, encodeURIComponent, decodeURIComponent, isNaN,
  };
  win.window = win;
  vm.createContext(sandbox);
  vm.runInContext(apiSource, sandbox);
  vm.runInContext(drawerSource, sandbox);
  vm.runInContext(pageScript, sandbox);
  await settle();
  env.win = win;
  env.panel = () => doc.querySelector('[data-panel-area]');
  env.click = async (el) => { assert.ok(el, 'clickable element exists'); el.dispatchEvent({ type: 'click', bubbles: true }); await settle(); };
  env.posts = () => requests.filter((r) => r.method === 'POST');
  env.viewReads = () => requests.filter((r) => r.method === 'GET' && r.url.includes('/view')).length;
  return env;
}

const keyed = (root, key) => root.querySelector(`[data-copy-key="${key}"]`);

async function openLines(env) {
  await env.click(env.panel().querySelector('[data-action-correct]'));
  await env.click(env.panel().querySelector('[data-correct-answer="who"]'));
  return env.panel().querySelector('[data-correct-step="who"]');
}

function tick(env, sentence) {
  const box = env.panel().querySelector(`[data-line-key="room|100000_300|audio|${sentence}"]`);
  assert.ok(box, `line ${sentence} offered`);
  box.checked = true;
  box.dispatchEvent({ type: 'change' });
}

const LINE_BODY = (speaker) => ({
  lines: [
    { stream: 'room', segment: '100000_300', source: 'audio', sentence_id: 1 },
    { stream: 'room', segment: '100000_300', source: 'audio', sentence_id: 2 },
  ],
  speaker,
});

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

test('the chooser offers its three answers when the activity has spoken lines', async () => {
  const env = await createPage({ items: [activity()], transcripts: { '100000_300': SPOKEN } });
  await env.click(env.panel().querySelector('[data-action-correct]'));
  const chooser = env.panel().querySelector('[data-correct-step="choose"]');
  assert.ok(keyed(chooser, 'correct-title'));
  for (const answer of ['who', 'what', 'playback']) {
    assert.ok(chooser.querySelector(`[data-correct-answer="${answer}"]`), answer);
  }
  assert.ok(keyed(chooser, 'correct-who') && keyed(chooser, 'correct-what') && keyed(chooser, 'correct-playback'));
});

test('the chooser leaves out "who said it" when the activity has no spoken lines', async () => {
  const env = await createPage({ items: [activity()], transcripts: { '100000_300': SCREEN_ONLY } });
  await env.click(env.panel().querySelector('[data-action-correct]'));
  const chooser = env.panel().querySelector('[data-correct-step="choose"]');
  assert.strictEqual(chooser.querySelector('[data-correct-answer="who"]'), null);
  assert.ok(chooser.querySelector('[data-correct-answer="what"]'));
  assert.ok(chooser.querySelector('[data-correct-answer="playback"]'));
});

test('lines no answer can be given for leave "who said it" out too', async () => {
  const disagree = JSON.parse(JSON.stringify(SPOKEN));
  disagree.speaker_labels.ambiguous = true;
  const env = await createPage({ items: [activity()], transcripts: { '100000_300': disagree } });
  await env.click(env.panel().querySelector('[data-action-correct]'));
  assert.strictEqual(env.panel().querySelector('[data-correct-answer="who"]'), null);
  assert.ok(env.panel().querySelector('[data-correct-answer="what"]'));
});

test('the owner\'s lines come first and the rest are folded away, each with a player', async () => {
  const env = await createPage({ items: [activity()], transcripts: { '100000_300': SPOKEN } });
  const step = await openLines(env);
  assert.ok(keyed(step, 'lines-yours'));
  const others = keyed(step, 'lines-others');
  assert.ok(others.querySelector('details.drawer'));
  assert.strictEqual(others.querySelector('details.drawer').hasAttribute('open'), false);
  assert.ok(others.querySelector('[data-line-key="room|100000_300|audio|2"]'));
  assert.strictEqual(step.querySelectorAll('[data-line-play]').length, 2);
  const players = step.querySelectorAll('[data-line-play]');
  assert.ok(players.every((p) => p.getAttribute('data-copy-key') === 'line-play-at'));
  assert.notStrictEqual(players[0].getAttribute('aria-label'), players[1].getAttribute('aria-label'), 'each player names its own line');
  const player = step.querySelector('[data-line-play="room|100000_300|audio|2"]');
  await env.click(player);
  const audio = env.panel().querySelector('[data-lines-audio]');
  assert.strictEqual(audio.getAttribute('src'), SPOKEN.chunks[1].audio_url);
  assert.strictEqual(audio.currentTime, 2);
  assert.ok(audio.playing);
  assert.strictEqual(player.getAttribute('aria-pressed'), 'true');
  assert.strictEqual(players[0].getAttribute('aria-pressed'), 'false');
  await env.click(player);
  assert.strictEqual(audio.playing, false, 'the playing line\'s button pauses it');
  assert.strictEqual(player.getAttribute('aria-pressed'), 'false');
});

test('a line\'s first words leave out a leading source note', async () => {
  const transcripts = { '100000_300': { ...SPOKEN, chunks: [audioLine(1, '(mic) we should cancel the vendor contract today'), SPOKEN.chunks[2]] } };
  const env = await createPage({ items: [activity()], transcripts });
  const step = await openLines(env);
  const words = step.querySelector('.act-line-words').textContent;
  assert.ok(words.startsWith('we should'), words);
});

test('"who said it" asks about the ticked lines by count and refuses an answer with none ticked', async () => {
  const env = await createPage({ items: [activity()], transcripts: { '100000_300': SPOKEN } });
  const step = await openLines(env);
  assert.ok(step.querySelector('[data-lines-question][data-copy-key="lines-question"]'));
  const lineOnly = keyed(step, 'lines-only');
  assert.ok(lineOnly && lineOnly.querySelector('details.drawer'), 'the line-only note shows, folded');
  assert.ok(keyed(lineOnly, 'lines-only-body'));
  assert.strictEqual(lineOnly.querySelector('a[data-copy-key="lines-only-link"]').getAttribute('href'), '/app/speakers/');
  const me = step.querySelector('[data-speaker="owner"]');
  assert.strictEqual(me.disabled, false, 'speakers stay enabled');
  await env.click(me);
  assert.strictEqual(env.posts().length, 0);
  assert.ok(keyed(env.panel(), 'refusal-line-empty'));
  tick(env, 1);
  assert.ok(env.panel().querySelector('[data-lines-question][data-copy-key="lines-question-one"]'));
  assert.strictEqual(keyed(env.panel(), 'refusal-line-empty'), null, 'ticking a line clears the refusal');
  tick(env, 2);
  assert.ok(env.panel().querySelector('[data-lines-question][data-copy-key="lines-question-n"]'));
});

test('back from the account step keeps what was typed', async () => {
  const env = await createPage({ items: [activity({ details: 'first account' })] });
  await env.click(env.panel().querySelector('[data-action-correct]'));
  await env.click(env.panel().querySelector('[data-correct-answer="what"]'));
  env.panel().querySelector('textarea[name="details"]').value = 'a better account';
  await env.click(env.panel().querySelector('[data-correct-back]'));
  await env.click(env.panel().querySelector('[data-correct-answer="what"]'));
  assert.strictEqual(env.panel().querySelector('textarea[name="details"]').value, 'a better account');
});

for (const [choice, speaker] of [['owner', { kind: 'owner' }], ['unknown', { kind: 'unknown' }]]) {
  test(`ticking lines and choosing ${choice} posts those lines with that speaker`, async () => {
    const env = await createPage({ items: [activity()], transcripts: { '100000_300': SPOKEN } });
    await openLines(env);
    tick(env, 1);
    tick(env, 2);
    const reads = env.viewReads();
    await env.click(env.panel().querySelector(`[data-speaker="${choice}"]`));
    const [post] = env.posts();
    assert.strictEqual(post.url, `/app/activities/api/day/${DAY}/record/standup/lines?facet=work`);
    assert.strictEqual(post.actor, 'owner');
    assert.deepStrictEqual(post.body, LINE_BODY(speaker));
    assert.strictEqual(env.viewReads(), reads + 1, 'the day view is read again after the save');
    assert.strictEqual(env.panel().querySelector('[data-correct-step]'), null);
  });
}

test('choosing a person searches the journal\'s people and posts that person', async () => {
  const env = await createPage({ items: [activity()], transcripts: { '100000_300': SPOKEN } });
  await openLines(env);
  tick(env, 1);
  tick(env, 2);
  await env.click(env.panel().querySelector('[data-speaker="person"]'));
  const offered = env.panel().querySelectorAll('[data-person-id]').map((b) => b.getAttribute('data-person-id'));
  assert.deepStrictEqual(offered, ['romeo'], 'only admitted people who are not the owner');
  const search = env.panel().querySelector('[data-person-search]');
  search.value = 'zzz';
  search.dispatchEvent({ type: 'input' });
  assert.ok(keyed(env.panel(), 'person-none'));
  search.value = 'rom';
  search.dispatchEvent({ type: 'input' });
  await env.click(env.panel().querySelector('[data-person-id="romeo"]'));
  assert.deepStrictEqual(env.posts()[0].body, LINE_BODY({ kind: 'person', id: 'romeo' }));
});

test('a refusal shows its copy, and a source refusal offers the owner\'s account instead', async () => {
  const env = await createPage({
    items: [activity()],
    transcripts: { '100000_300': SPOKEN },
    post: () => respond(409, { error: 'refused', reason_code: 'owner_line_source_unsupported' }),
  });
  await openLines(env);
  tick(env, 1);
  await env.click(env.panel().querySelector('[data-speaker="owner"]'));
  const step = env.panel().querySelector('[data-correct-step="who"]');
  assert.ok(keyed(step, 'refusal-line-source'));
  assert.ok(step.querySelector('[data-correct-answer="what"]'));
  await env.click(step.querySelector('[data-correct-answer="what"]'));
  assert.ok(env.panel().querySelector('[data-correct-step="what"]'));
});

test('any other refusal shows its own copy key', async () => {
  const env = await createPage({
    items: [activity()],
    transcripts: { '100000_300': SPOKEN },
    post: () => respond(409, { error: 'refused', reason_code: 'owner_line_not_found' }),
  });
  await openLines(env);
  tick(env, 2);
  await env.click(env.panel().querySelector('[data-speaker="unknown"]'));
  assert.ok(keyed(env.panel(), 'refusal-line-changed'));
  assert.strictEqual(env.panel().querySelector('[data-correct-answer="what"]'), null);
});

test('"what actually happened" starts from the current account and saves it as details', async () => {
  const env = await createPage({ items: [activity({ details: 'nothing was decided' })], transcripts: { '100000_300': SPOKEN } });
  await env.click(env.panel().querySelector('[data-action-correct]'));
  await env.click(env.panel().querySelector('[data-correct-answer="what"]'));
  const form = env.panel().querySelector('[data-account-form]');
  assert.ok(keyed(form, 'label-details'));
  assert.ok(keyed(form, 'account-hint'));
  const box = form.querySelector('textarea[name="details"]');
  assert.strictEqual(box.value, 'nothing was decided');
  box.value = 'we talked about cancelling; nothing was decided';
  form.dispatchEvent({ type: 'submit' });
  await settle();
  const [post] = env.posts();
  assert.strictEqual(post.url, `/app/activities/api/day/${DAY}/record/standup/update?facet=work`);
  assert.strictEqual(post.actor, 'owner');
  assert.deepStrictEqual(post.body, { patch: { details: 'we talked about cancelling; nothing was decided' } });
});

test('the edit form labels details as the owner\'s account', async () => {
  const env = await createPage({ items: [activity()] });
  await env.click(env.panel().querySelector('[data-action-edit]'));
  const form = env.panel().querySelector('[data-edit-form]');
  assert.ok(keyed(form, 'label-details'));
});

test('"this was a recording or video playing" confirms, then patches playback', async () => {
  const env = await createPage({ items: [activity()] });
  await env.click(env.panel().querySelector('[data-action-correct]'));
  await env.click(env.panel().querySelector('[data-correct-answer="playback"]'));
  assert.ok(keyed(env.panel(), 'playback-confirm-body'));
  assert.strictEqual(env.posts().length, 0, 'nothing is saved before the confirm');
  await env.click(env.panel().querySelector('[data-playback-confirm]'));
  assert.deepStrictEqual(env.posts()[0].body, { patch: { playback: true } });
});

test('an activity already marked as playing does not offer it again', async () => {
  const env = await createPage({ items: [activity({ playback: true, corrected: true })] });
  await env.click(env.panel().querySelector('[data-action-correct]'));
  assert.strictEqual(env.panel().querySelector('[data-correct-answer="playback"]'), null);
});

test('"updating…" shows in the summary while the view says the activity is updating', async () => {
  const env = await createPage({ items: [activity({ updating: true, corrected: true })] });
  assert.ok(keyed(env.panel(), 'updating'));
  assert.ok(env.panel().querySelector('[data-previous-summary]'), 'the summary it had stays under it');
  assert.strictEqual(keyed(env.panel(), 'corrected-by-you'), null);
  env.view = view([activity({ updating: false, corrected: true, summary: 'Nothing was decided.' })]);
  await env.click(env.panel().querySelector('[data-action-correct]'));
  await env.click(env.panel().querySelector('[data-correct-answer="playback"]'));
  await env.click(env.panel().querySelector('[data-playback-confirm]'));
  assert.strictEqual(keyed(env.panel(), 'updating'), null);
  assert.ok(keyed(env.panel(), 'corrected-by-you'));
});

test('"corrected by you" shows on a corrected activity and not on one never corrected', async () => {
  const corrected = await createPage({ items: [activity({ corrected: true })] });
  assert.ok(keyed(corrected.panel(), 'corrected-by-you'));
  const plain = await createPage({ items: [activity()] });
  assert.strictEqual(keyed(plain.panel(), 'corrected-by-you'), null);
  const lines = await createPage({ items: [activity({ edits: [{ kind: 'owner_lines', lines: 2, speaker: { kind: 'owner' }, timestamp: AT, actor: 'owner:lines', fields: [], position: 0, removable: true, removed: false }] })] });
  assert.ok(keyed(lines.panel(), 'corrected-by-you'));
});

test('history names each correction and remove posts that entry\'s position, timestamp and actor', async () => {
  const edits = [
    { kind: 'owner_update', fields: ['description'], timestamp: '2026-10-30T08:00:00Z', actor: 'owner:update', note: null, position: 0, removable: true, removed: false },
    { kind: 'owner_update', fields: ['details'], timestamp: '2026-10-30T09:00:00Z', actor: 'owner:update', note: null, position: 1, removable: true, removed: false },
    { kind: 'owner_update', fields: ['playback'], timestamp: '2026-10-30T10:00:00Z', actor: 'owner:update', note: null, position: 2, removable: false, removed: true },
    { kind: 'owner_lines', lines: 2, speaker: { kind: 'person', id: 'romeo' }, fields: [], timestamp: '2026-10-30T11:00:00Z', actor: 'owner:lines', note: null, position: 3, removable: true, removed: false },
    { kind: 'owner_lines', lines: 1, speaker: { kind: 'owner' }, fields: [], timestamp: '2026-10-30T12:00:00Z', actor: 'owner:lines', note: null, position: 4, removable: true, removed: false },
  ];
  const env = await createPage({ items: [activity({ edits })] });
  const history = env.panel();
  for (const key of ['history-you-changed', 'history-you-told', 'history-you-playback', 'history-you-lines-many', 'history-you-lines-one', 'history-removed']) {
    assert.ok(keyed(history, key), key);
  }
  assert.strictEqual(history.querySelectorAll('[data-history-remove]').length, 4, 'only removable entries offer remove');
  const labels = history.querySelectorAll('[data-history-remove]').map((b) => b.getAttribute('aria-label'));
  assert.ok(labels.every(Boolean) && new Set(labels).size === labels.length, 'each remove names its own entry');
  await env.click(history.querySelector('[data-history-remove="3"]'));
  const [post] = env.posts();
  assert.strictEqual(post.url, `/app/activities/api/day/${DAY}/record/standup/edits/remove?facet=work`);
  assert.strictEqual(post.actor, 'owner');
  assert.deepStrictEqual(post.body, { position: 3, timestamp: '2026-10-30T11:00:00Z', actor: 'owner:lines' });
});

test('a remove refused because the history moved on reads the day again and says so', async () => {
  const edits = [{ kind: 'owner_update', fields: ['title'], timestamp: AT, actor: 'owner:update', note: null, position: 0, removable: true, removed: false }];
  const env = await createPage({ items: [activity({ edits })], post: () => respond(409, { error: 'refused', reason_code: 'activity_edit_mismatch' }) });
  const reads = env.viewReads();
  await env.click(env.panel().querySelector('[data-history-remove="0"]'));
  assert.strictEqual(env.viewReads(), reads + 1);
  assert.ok(keyed(env.panel(), 'remove-stale'));
});

test('a link asking for corrections opens the activity with its chooser open', async () => {
  const env = await createPage({
    items: [activity()],
    url: `https://journal.example/app/activities/${DAY}?facet=work&correct=1#work/standup`,
    transcripts: { '100000_300': SPOKEN },
  });
  assert.ok(env.panel().querySelector('[data-correct-step="choose"]'));
  assert.ok(env.panel().querySelector('[data-correct-answer="who"]'));
  assert.strictEqual(env.win.location.searchParams.get('correct'), null, 'the ask is used once');
  assert.strictEqual(env.win.location.hash, '#work/standup');
});

test('an activity opened without that ask shows its panel as before', async () => {
  const env = await createPage({ items: [activity()] });
  assert.strictEqual(env.panel().querySelector('[data-correct-step]'), null);
  assert.ok(env.panel().querySelector('[data-action-correct]'));
  assert.ok(env.panel().querySelector('[data-action-edit]'));
});

test('on a narrow screen, taps that redraw the sheet keep it open', async () => {
  const env = await createPage({ items: [activity()], transcripts: { '100000_300': SPOKEN }, narrow: true });
  await env.click(env.panel().querySelector('[data-action-correct]'));
  assert.ok(env.panel().querySelector('[data-correct-step="choose"]'), 'the chooser opens');
  await env.click(env.panel().querySelector('[data-correct-answer="who"]'));
  assert.ok(env.panel().querySelector('[data-correct-step="who"]'), 'an answer opens its step');
  await env.click(env.panel().querySelector('[data-correct-back]'));
  assert.ok(env.panel().querySelector('[data-correct-step="choose"]'), 'back returns to the chooser');
  await env.click(env.panel().querySelector('[data-correct-cancel]'));
  await env.click(env.panel().querySelector('[data-action-edit]'));
  assert.ok(env.panel().querySelector('[data-edit-form]'), 'edit opens its form');
  assert.strictEqual(env.win.location.hash, '#work/standup', 'the activity stays selected');
});

test('on a narrow screen, a tap outside the sheet still closes it', async () => {
  const env = await createPage({ items: [activity()], narrow: true });
  assert.ok(env.panel().querySelector('[data-action-correct]'));
  await env.click(env.doc.body);
  assert.strictEqual(env.panel().querySelector('[data-action-correct]'), null);
  assert.strictEqual(env.win.location.hash, '');
});

(async () => {
  for (const t of tests) {
    try {
      await t.fn();
    } catch (err) {
      console.error(`FAILED: ${t.name}`);
      console.error(err);
      process.exit(1);
    }
  }
  console.log(`DOM CASES: ${tests.length} passed`);
})();
