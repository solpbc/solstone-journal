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

  console.log(`DOM CASES: ${cases} passed`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
