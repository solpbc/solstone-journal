// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

let cases = 0;
function check(condition, message) {
  cases += 1;
  assert.ok(condition, message);
}

class ClassList {
  constructor() {
    this.values = new Set();
  }
  add(...names) { names.forEach((n) => this.values.add(n)); }
  remove(...names) { names.forEach((n) => this.values.delete(n)); }
  contains(name) { return this.values.has(name); }
}

class Style {
  constructor() {
    this.values = new Map();
    this.display = '';
  }
  setProperty(name, value) { this.values.set(name, value); }
  removeProperty(name) { this.values.delete(name); }
  getPropertyValue(name) { return this.values.get(name) || ''; }
}

class Element {
  constructor(document, id = '', tagName = 'div') {
    this.ownerDocument = document;
    this.id = id;
    this.tagName = tagName.toUpperCase();
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.attributes = {};
    this.dataset = {};
    this.classList = new ClassList();
    this.style = new Style();
    this.disabled = false;
    this.src = '';
    this.currentTime = 0;
    this._innerHTML = '';
  }

  get innerHTML() {
    return this._innerHTML;
  }

  set innerHTML(html) {
    this._innerHTML = html;
    this.children = parseHtmlToTree(this.ownerDocument, html, this);
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  remove() {
    if (!this.parentElement) return;
    this.parentElement.children = this.parentElement.children.filter((c) => c !== this);
    this.parentElement = null;
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name.startsWith('data-')) {
      const key = name.slice(5).replace(/-([a-z])/g, (_, g) => g.toUpperCase());
      this.dataset[key] = String(value);
    }
    if (name === 'id') this.id = String(value);
    if (name === 'src') this.src = String(value);
    if (name === 'disabled') this.disabled = true;
  }

  getAttribute(name) {
    return Object.hasOwn(this.attributes, name) ? this.attributes[name] : null;
  }

  addEventListener(name, listener) {
    (this.listeners[name] ||= []).push(listener);
  }

  emit(name, event = {}) {
    (this.listeners[name] || []).forEach((l) => l(event));
  }

  play() {
    this._playing = true;
  }

  insertAdjacentHTML(position, html) {
    const nodes = parseHtmlToTree(this.ownerDocument, html, this.parentElement || this);
    if (position === 'beforeend') {
      nodes.forEach((n) => this.appendChild(n));
    }
  }

  querySelector(selector) {
    return querySelectorInternal(this, selector);
  }

  querySelectorAll(selector) {
    const results = [];
    querySelectorAllInternal(this, selector, results);
    return results;
  }
}

function parseHtmlToTree(document, html, parent) {
  const elements = [];
  const tagRegex = /<([a-zA-Z0-9\-]+)([^>]*)>([\s\S]*?)<\/\1>|<([a-zA-Z0-9\-]+)([^>]*)\/?>/g;
  let match;
  while ((match = tagRegex.exec(html)) !== null) {
    const tagName = match[1] || match[4];
    const attrsStr = match[2] || match[5] || '';
    const inner = match[3] || '';
    const el = new Element(document, '', tagName);
    el.parentElement = parent;

    const attrRegex = /([a-zA-Z0-9\-]+)(?:=(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
    let attrMatch;
    while ((attrMatch = attrRegex.exec(attrsStr)) !== null) {
      const attrName = attrMatch[1];
      const attrVal = attrMatch[2] ?? attrMatch[3] ?? attrMatch[4] ?? '';
      el.setAttribute(attrName, attrVal);
    }

    if (inner) {
      el.children = parseHtmlToTree(document, inner, el);
      el._innerHTML = inner;
    }
    elements.push(el);
  }
  return elements;
}

function querySelectorInternal(root, selector) {
  const all = [];
  querySelectorAllInternal(root, selector, all);
  return all[0] || null;
}

function querySelectorAllInternal(node, selector, results) {
  for (const child of node.children) {
    if (matchesSelector(child, selector)) {
      results.push(child);
    }
    querySelectorAllInternal(child, selector, results);
  }
}

function matchesSelector(el, selector) {
  if (selector.startsWith('#')) {
    return el.id === selector.slice(1);
  }
  if (selector.startsWith('.')) {
    const className = selector.slice(1);
    const classes = (el.getAttribute('class') || '').split(/\s+/);
    return classes.includes(className);
  }
  if (selector.startsWith('[') && selector.endsWith(']')) {
    const attrMatch = selector.slice(1, -1).match(/([a-zA-Z0-9\-]+)(?:="([^"]*)")?/);
    if (attrMatch) {
      const attrName = attrMatch[1];
      const attrVal = attrMatch[2];
      if (attrVal !== undefined) {
        return el.getAttribute(attrName) === attrVal;
      }
      return el.getAttribute(attrName) !== null;
    }
  }
  return el.tagName.toLowerCase() === selector.toLowerCase();
}

function findById(root, id) {
  if (!root) return null;
  if (root.id === id) return root;
  for (const child of root.children) {
    const found = findById(child, id);
    if (found) return found;
  }
  return null;
}

// The source of one named function from the workspace, for exercising it alone.
function functionSource(workspace, name) {
  const match = new RegExp(`(?:async )?function ${name}\\(`).exec(workspace);
  if (!match) throw new Error(`function ${name} not found`);
  let depth = 0;
  for (let index = workspace.indexOf('{', match.index); index < workspace.length; index += 1) {
    if (workspace[index] === '{') depth += 1;
    if (workspace[index] === '}') {
      depth -= 1;
      if (depth === 0) return workspace.slice(match.index, index + 1);
    }
  }
  throw new Error(`function ${name} is not closed`);
}

// Teaching draws on recent days, so a quiet today still finds the owner's
// statements from earlier in the week.
async function teachingReachesPastDays(workspace) {
  const context = {
    day: '20261011',
    OWNER_TEACH_RECENT_DAYS: 14,
    OWNER_TEACH_TARGET_STATEMENTS: 40,
    window: { logError() {} },
    todayDayKey: () => '20261011',
    asked: [],
  };
  context.ensureOwnerTeachSegments = async (dayKey) => {
    context.asked.push(dayKey);
    return dayKey === '20261008' ? [{ key: '090000_300', sources: ['audio'] }] : [];
  };
  context.loadOwnerTeachReviews = async (dayKey, segments) => segments.map(() => ({
    items: Array.from({ length: 40 }, (_, id) => ({ day: dayKey, id })),
    failed_count: 0,
  }));
  vm.createContext(context);
  for (const name of ['recentDayKeys', 'ownerTeachPoolCounts', 'collectOwnerTeachReviews']) {
    vm.runInContext(functionSource(workspace, name), context);
  }
  check(
    JSON.stringify(vm.runInContext('recentDayKeys(3)', context)) === JSON.stringify(['20261011', '20261010', '20261009']),
    'recent days count back from today'
  );
  const results = await vm.runInContext('collectOwnerTeachReviews(() => true)', context);
  const items = results.flatMap((result) => result.items);
  check(items.length === 40 && items.every((item) => item.day === '20261008'), 'statements from a past day are offered');
  check(JSON.stringify(context.asked) === JSON.stringify(['20261011', '20261010', '20261009', '20261008']), 'teaching stops once it has enough');
}

async function overviewFailuresStayUnavailable(workspace) {
  const calls = [];
  const failed = new Error('read failed');
  const context = {
    apiJson: async () => { throw failed; },
    window: { SurfaceState: {
      error: () => 'unavailable', serverMessageFrom: () => '',
    }, CONVEY_COPY: { RELOAD_HINT: 'reload' } },
    qualityRequestSeq: 0,
    qualityPanel: { innerHTML: '' },
    COPY: { SPK_OVERVIEW_QUALITY_ERROR_HEADING: '' },
    OWNER_STATUS_ROUTING: { candidate: 'candidate', confirmed: 'confirmed' },
    ownerStatusError: error => calls.push(['error', error]),
    ownerNotReady: () => calls.push(['not-ready']),
    ownerCandidate: () => calls.push(['candidate']),
    ownerConfirmed: () => calls.push(['confirmed']),
    pollRelabel: () => calls.push(['poll']),
    renderQuality: () => calls.push(['quality']),
  };
  vm.createContext(context);
  for (const name of ['loadOwner', 'loadQuality']) {
    vm.runInContext(functionSource(workspace, name), context);
  }
  await vm.runInContext('loadOwner()', context);
  check(calls.length === 1 && calls[0][0] === 'error' && calls[0][1] === failed,
    'a failed owner read never becomes an unconfirmed owner or starts polling');
  const result = await vm.runInContext('loadQuality()', context);
  check(result === false && context.qualityPanel.innerHTML === 'unavailable' && calls.length === 1,
    'a failed quality read never becomes zero progress');
}

// The overview resolves a statement handoff from what the mount reads off its
// own address alone: the layout decoder it calls is the script's own, shared
// by both modes, not a name on the page.
async function statementHandoffResolvesFromThePageContext(workspace) {
  const requested = [];
  const page = {
    apiJson: async (url) => { requested.push(url); return { status: 'miss' }; },
    URLSearchParams,
  };
  vm.createContext(page);
  for (const name of ['decodeSegmentLayout', 'speakersContext', 'resolveStatementHandoff']) {
    vm.runInContext(functionSource(workspace, name), page);
  }
  page.route = { pathname: '/app/speakers/', search: '?voice_day=20261022&voice_stream=mic&voice_segment_key=100000_300&voice_source=audio&voice_sentence_id=2', day: null };
  vm.runInContext('var context = speakersContext(route);', page);
  await vm.runInContext('resolveStatementHandoff(context.statementHandoff)', page);
  const resolves = requested.filter((url) => url.startsWith('/app/speakers/api/discovery/resolve-statement?'));
  const params = new URLSearchParams(resolves[0]?.split('?')[1] || '');
  check(resolves.length === 1, 'a complete handoff asks which voice holds its statement');
  check(params.get('voice_stream_layout') === 'named' && params.get('voice_sentence_id') === '2',
    'the handoff carries its line and a named layout when the link names none');
  vm.runInContext("context.statementHandoff.streamLayout = 'sideways';", page);
  assert.throws(() => vm.runInContext('resolveStatementHandoff(context.statementHandoff)', page),
    /invalid segment layout/);
  check(true, 'a handoff with an unknown layout is refused before it is asked');
}

// Where a voice appears reads each stream's provenance label once, joined,
// with the stream names behind each label kept in its title.
async function voiceStreamsReadAsLabels(workspace) {
  const format = {};
  vm.runInNewContext(
    fs.readFileSync(path.join(__dirname, '../assets/static/date_format.js'), 'utf8'),
    { window: format },
  );
  format.JournalFormat.installStreamLabels({
    status: 'complete',
    labels: { macos_screen: 'LABEL-ONE', macos_audio: 'LABEL-ONE', ios_audio: 'LABEL-TWO' },
  });
  const ownerPanel = { innerHTML: '' };
  const context = {
    window: format,
    COPY: {},
    TODAY: '20261016',
    ownerPanel,
    document: { getElementById: () => null },
    escapeHtml: (value) => String(value ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;'),
    relabelHtml: () => '',
    ownerSamplesText: () => '',
    cohesion: () => '',
    absoluteDate: () => '',
    ownerUndoAsk: () => {},
  };
  vm.createContext(context);
  vm.runInContext(functionSource(workspace, 'ownerConfirmed'), context);
  vm.runInContext(`ownerConfirmed(${JSON.stringify({ centroid_metadata: { streams: ['macos_screen', 'macos_audio', 'ios_audio'] } })})`, context);
  const spans = [...ownerPanel.innerHTML.matchAll(/<span title="([^"]*)">([^<]*)<\/span>/g)];
  check(spans.length === 2, 'repeated labels are shown once');
  check(spans[0][2] === 'LABEL-ONE' && spans[0][1] === 'macos_screen, macos_audio', 'a label keeps every stream name behind it');
  check(spans[1][2] === 'LABEL-TWO' && spans[1][1] === 'ios_audio', 'each label keeps its own stream name');
  check(!ownerPanel.innerHTML.replace(/title="[^"]*"/g, '').includes('macos_'), 'no stream name is shown as text');
}

// A voice sample with no setting is named by its stream's label, not its id.
function sampleFallsBackToTheLabel() {
  const source = fs.readFileSync(path.join(__dirname, '../assets/speakers/who_is_this.js'), 'utf8');
  const method = functionSource(source.replace('    sampleLabel(sample) {', '    function sampleLabel(sample) {'), 'sampleLabel');
  const format = {};
  vm.runInNewContext(
    fs.readFileSync(path.join(__dirname, '../assets/static/date_format.js'), 'utf8'),
    { window: format },
  );
  format.JournalFormat.installStreamLabels({ status: 'complete', labels: { device_9e92ab54: 'LABEL-ONE' } });
  const context = { window: format };
  vm.createContext(context);
  vm.runInContext(method, context);
  const self = { formatDateShort: () => 'DAY' };
  const label = (sample) => vm.runInContext('sampleLabel', context).call(self, sample);
  check(label({ day: '20261016', stream: 'device_9e92ab54' }) === 'DAY · LABEL-ONE', 'a sample with no setting reads its label');
  check(label({ day: '20261016', stream: 'device_9e92ab54', setting: 'kitchen' }) === 'DAY · kitchen', 'a setting still wins');
}

// ---- a pick from a select, on a phone that blurs it late

// The contract cases' page (a document with focus and events, a clock the
// cases move, the view contract and its read store), loaded apart from them.
function contractPage() {
  const file = path.join(__dirname, 'speakers_contract_dom.js');
  const body = `${fs.readFileSync(file, 'utf8')}\nreturn { makePage, settle, journal, review, DAY, ORIGIN, Element };`;
  const kit = new Function('require', 'module', 'exports', '__dirname', '__filename', body)(require, { exports: {} }, {}, __dirname, file);
  if (!Object.getOwnPropertyDescriptor(kit.Element.prototype, 'nextElementSibling')) {
    Object.defineProperty(kit.Element.prototype, 'nextElementSibling', {
      get() {
        const kids = this.parentNode ? this.parentNode.children : [];
        const at = kids.indexOf(this);
        return at >= 0 ? kids[at + 1] || null : null;
      },
    });
  }
  return kit;
}

async function pickCase(name, fn) {
  try {
    await fn();
  } catch (error) {
    error.message = `${name}: ${error.message}`;
    throw error;
  }
  cases += 1;
}

// iOS, after a pick: the change with the select still focused...
function pick(p, select, value) {
  select.focus();
  assert.ok(p.doc.activeElement === select, 'the select has focus before the pick');
  select.value = value;
  p.doc.dispatch(select, { type: 'change' });
}

// ...then, as its menu closes, a blur with nothing pressed in the page.
function lateBlur(p, select, { moveFocus = true } = {}) {
  if (moveFocus) p.doc.activeElement = p.doc.body;
  select.fire({ type: 'blur', target: select, bubbles: false, preventDefault() {}, stopPropagation() {} });
  p.doc.dispatch(select, { type: 'focusout' });
}

// The document's capture listeners of the kinds a wait listens for.
function captureListeners(p) {
  let n = 0;
  for (const type of ['pointerdown', 'touchstart', 'mousedown', 'keydown']) {
    n += (p.doc.listeners.get(type) || []).filter((l) => l.capture).length;
  }
  return n;
}

async function selectLandingCases(manifestDir) {
  const kit = contractPage();
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/speakers/workspace.html'), 'utf8');
  const script = workspace.slice(workspace.lastIndexOf('<script>') + '<script>'.length, workspace.lastIndexOf('</script>'));
  const markup = workspace.slice(0, workspace.indexOf('<script src="/app/speakers/static/who_is_this.js">'))
    .replace(/<style>[\s\S]*?<\/style>/g, '')
    .replace(/<script[^>]*><\/script>/g, '')
    .replace(/<link[^>]*>/g, '');
  const whoIsThis = fs.readFileSync(path.join(manifestDir, 'assets/speakers/who_is_this.js'), 'utf8');
  const copy = JSON.parse(fs.readFileSync(path.join(manifestDir, 'assets/speakers/copy.json'), 'utf8'));
  const focusJs = fs.readFileSync(path.join(manifestDir, 'assets/static/focus.js'), 'utf8');
  const env = { manifestDir, script, markup, whoIsThis, copy };
  const sorts = copy.SPK_OVERVIEW_KNOWN_VOICES_SORTS;
  assert.ok(sorts.length >= 2, 'there are sorts to pick between');

  // Known voices to sort, so the sort stays shown.
  const known = { speakers: [{ entity_id: 'ada', name: 'Ada', embedding_count: 3, segment_count: 2 }] };
  const answers = (url, opts) => (new URL(url, kit.ORIGIN).pathname === '/app/speakers/api/speakers/known' ? known : kit.journal(copy)(url, opts));

  // The page as it ships: the shell's focus helpers, on the page's clock, before the view.
  async function boot(address) {
    const p = kit.makePage(env, address, { answers });
    vm.runInContext(focusJs, p.context, { filename: 'focus.js' });
    p.loadScripts();
    p.place();
    await p.mount();
    p.startRouter();
    await kit.settle();
    return p;
  }
  async function dayWithSentences() {
    const p = await boot(`/app/speakers/${kit.DAY}`);
    p.byId('spkSegmentList').querySelectorAll('.spk-segment')[0].click();
    await kit.settle();
    return p;
  }
  const daySelects = [['.spk-correct-select', 'a correction'], ['.spk-assign-select', 'an assignment']];

  for (const [selector, label] of daySelects) {
    await pickCase(`a day: ${label} picked on a phone keeps focus through the menu's late blur`, async () => {
      const p = await dayWithSentences();
      const select = p.q(selector);
      assert.ok(select, 'the select is drawn');
      pick(p, select, 'ada');
      p.clock.advance(865);
      lateBlur(p, select);
      p.clock.advance(0);
      assert.ok(p.doc.activeElement === select, 'focus is back on the same select');
    });

    await pickCase(`a day: ${label} picked where the select keeps focus leaves nothing behind`, async () => {
      const p = await dayWithSentences();
      const select = p.q(selector);
      const held = JSON.stringify(p.view.ctx.held());
      const capture = captureListeners(p);
      pick(p, select, 'ada');
      p.clock.advance(2000);
      await kit.settle();
      assert.ok(p.doc.activeElement === select, 'focus is untouched');
      assert.ok(JSON.stringify(p.view.ctx.held()) === held, 'the view holds what it held before the pick');
      assert.ok(captureListeners(p) === capture, 'no capture listener is left on the document');
    });
  }

  await pickCase('a day step while a pick waits lands nothing and leaves nothing of the wait', async () => {
    const p = await dayWithSentences();
    const capture = captureListeners(p);
    const select = p.q('.spk-correct-select');
    pick(p, select, 'ada');
    p.window.solRouter.navigate('/app/speakers/20261111');
    await kit.settle();
    assert.ok(!select.isConnected, 'the old day is gone');
    const after = p.doc.activeElement;
    lateBlur(p, select, { moveFocus: false });
    p.clock.advance(2000);
    await kit.settle();
    assert.ok(p.doc.activeElement === after, 'focus stays where the step put it');
    assert.ok(captureListeners(p) === capture, 'no capture listener of the wait is left');
  });

  const sortReads = (p, sort) => p.reads(`/app/speakers/api/speakers/known?sort=${encodeURIComponent(sort)}`).length;

  await pickCase('the overview: a sort picked on a phone keeps focus through the menu\'s late blur', async () => {
    const p = await boot('/app/speakers/');
    const sortSelect = p.byId('spkKnownSort');
    assert.ok(sortSelect && !sortSelect.hidden, 'the sort is shown');
    pick(p, sortSelect, sorts[1]);
    p.clock.advance(865);
    lateBlur(p, sortSelect);
    p.clock.advance(0);
    assert.ok(p.doc.activeElement === sortSelect, 'focus is back on the sort');
  });

  await pickCase('the overview: a sort picked where the select keeps focus leaves nothing behind', async () => {
    const p = await boot('/app/speakers/');
    const sortSelect = p.byId('spkKnownSort');
    const held = JSON.stringify(p.view.ctx.held());
    const capture = captureListeners(p);
    pick(p, sortSelect, sorts[1]);
    p.clock.advance(2000);
    await kit.settle();
    assert.ok(p.doc.activeElement === sortSelect, 'focus is untouched');
    assert.ok(JSON.stringify(p.view.ctx.held()) === held, 'the view holds what it held before the pick');
    assert.ok(captureListeners(p) === capture, 'no capture listener is left on the document');
  });

  // The reads are counted before the landing is asserted, so a page that
  // doesn't land still shows how many it sent.
  await pickCase('the overview: the list drawn again before the late blur, the landing finds the same sort, and one read per pick', async () => {
    const p = await boot('/app/speakers/');
    const sortSelect = p.byId('spkKnownSort');
    const landed = [];
    for (const sort of [sorts[1], sorts[0]]) {
      const before = sortReads(p, sort);
      const card = p.q('.spk-known-card');
      pick(p, sortSelect, sort);
      await kit.settle();
      assert.ok(sortReads(p, sort) === before + 1, 'the pick reads its sort once');
      assert.ok(card && p.q('.spk-known-card') && p.q('.spk-known-card') !== card, 'the list is drawn again');
      p.clock.advance(865);
      lateBlur(p, sortSelect);
      p.clock.advance(0);
      landed.push(p.doc.activeElement === sortSelect);
      p.clock.advance(2000);
      await kit.settle();
      assert.ok(sortReads(p, sort) === before + 1, 'the landing reads nothing');
    }
    assert.ok(landed[0] && landed[1], 'each landing finds the same sort');
  });

  await pickCase('the overview: the list drawn again after the late blur keeps focus on the same sort, and one read per pick', async () => {
    const p = await boot('/app/speakers/');
    const sortSelect = p.byId('spkKnownSort');
    const before = sortReads(p, sorts[1]);
    p.holds.push((call) => call.url.startsWith('/app/speakers/api/speakers/known'));
    pick(p, sortSelect, sorts[1]);
    await kit.settle();
    const out = p.calls.find((call) => call.url.startsWith('/app/speakers/api/speakers/known') && !call.settled);
    assert.ok(out, 'the sort\'s read is out');
    p.clock.advance(865);
    lateBlur(p, sortSelect);
    p.clock.advance(0);
    const landed = p.doc.activeElement === sortSelect;
    const card = p.q('.spk-known-card');
    p.holds.length = 0;
    out.answer(known);
    await kit.settle();
    assert.ok(card && p.q('.spk-known-card') !== card, 'the list is drawn again');
    p.clock.advance(2000);
    await kit.settle();
    assert.ok(sortReads(p, sorts[1]) === before + 1, 'one read for the pick, none from the landing');
    assert.ok(landed, 'focus is back on the sort');
    assert.ok(p.doc.activeElement === sortSelect, 'the list drawn again leaves focus on the same sort');
  });
}

async function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) throw new Error('manifest directory required');
  const workspacePath = path.join(manifestDir, 'assets/speakers/workspace.html');
  const workspace = fs.readFileSync(workspacePath, 'utf8');

  check(workspace.length > 0, 'workspace.html exists and is readable');

  const scriptStart = workspace.lastIndexOf('<script>') + '<script>'.length;
  const scriptEnd = workspace.lastIndexOf('</script>');
  check(scriptStart >= '<script>'.length && scriptEnd > scriptStart, 'workspace contains script tags');

  const scriptContent = workspace.slice(scriptStart, scriptEnd);
  new vm.Script(scriptContent, { filename: 'speakers-workspace.js' });
  check(true, 'the complete Speakers workspace script parses without syntax error');

  // Verify key contract behaviors in the script source:
  check(workspace.includes('SPK_ACTION_SET_ASIDE'), 'workspace references SPK_ACTION_SET_ASIDE');
  check(workspace.includes('SPK_PLAY_FROM_HERE_LABEL'), 'workspace references SPK_PLAY_FROM_HERE_LABEL');
  check(workspace.includes('/app/speakers/api/owner/set-aside'), 'workspace references set-aside endpoint');
  check(workspace.includes('/app/speakers/api/owner/confirm'), 'workspace references confirm endpoint');
  check(workspace.includes('/app/speakers/api/owner/reject'), 'workspace references reject endpoint');
  check(workspace.includes('revokeObjectURL'), 'workspace handles audio error cleanup with revokeObjectURL');
  check(workspace.includes('/app/speakers/api/owner/undo'), 'workspace references undo endpoint');
  check(workspace.includes('/app/speakers/api/owner/rebuild'), 'workspace references rebuild endpoint for redo');

  const dayScript = functionSource(workspace, 'runDay');
  check(dayScript.includes('function recentDayKeys'), 'the day script holds the owner-voice flow');
  new vm.Script(dayScript, { filename: 'speakers-day.js' });
  check(true, 'the Speakers day script parses without syntax error');
  await teachingReachesPastDays(workspace);
  await overviewFailuresStayUnavailable(workspace);
  await voiceStreamsReadAsLabels(workspace);
  await statementHandoffResolvesFromThePageContext(workspace);
  sampleFallsBackToTheLabel();
  await selectLandingCases(manifestDir);
  cases += await require('./speakers_contract_dom.js')(manifestDir);

  console.log(`DOM CASES: ${cases} passed`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
