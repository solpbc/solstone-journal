// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Photos of people on the entities page: which panel each state draws, that
// a suggestion is labelled as one, that each action sends the face it names,
// that every refusal offers a next step, and that only a chosen photo reaches
// a list. Wording and look are voice-check's and design QA's, not this file's.

const { journalContext, readCopyTable } = require('./journal_catalog_dom.js');
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

class Node {
  constructor(doc, tag) {
    this.ownerDocument = doc;
    this.tagName = String(tag || '').toUpperCase();
    this.children = [];
    this.parentElement = null;
    this.dataset = {};
    this.attributes = {};
    this.hidden = false;
    this.disabled = false;
    this.className = '';
    this.id = '';
    this._text = '';
    this.style = {};
    const node = this;
    this.classList = {
      add(name) { node.className = `${node.className} ${name}`.trim(); },
      contains(name) { return node.className.split(/\s+/).includes(name); },
    };
  }
  get textContent() {
    return this._text + this.children.map((child) => child.textContent).join('');
  }
  set textContent(value) {
    this._text = String(value);
    this.children = [];
  }
  get childNodes() { return this.children; }
  get parentNode() { return this.parentElement; }
  get nextSibling() {
    const siblings = this.parentElement ? this.parentElement.children : [];
    return siblings[siblings.indexOf(this) + 1] || null;
  }
  insertBefore(node, ref) {
    node.parentElement = this;
    const at = ref ? this.children.indexOf(ref) : -1;
    if (at < 0) this.children.push(node);
    else this.children.splice(at, 0, node);
    return node;
  }
  append(...nodes) {
    nodes.forEach((node) => {
      const child = typeof node === 'string' ? Object.assign(new Node(this.ownerDocument, '#text'), {_text: node}) : node;
      child.parentElement = this;
      this.children.push(child);
    });
  }
  prepend(node) { node.parentElement = this; this.children.unshift(node); }
  appendChild(node) { this.append(node); return node; }
  replaceChildren(...nodes) { this.children = []; this._text = ''; this.append(...nodes); }
  remove() {
    if (!this.parentElement) return;
    this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
    this.parentElement = null;
  }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return Object.hasOwn(this.attributes, name) ? this.attributes[name] : null; }
  // Walked in a loop, not a callback that closes over the matches: while the
  // engine compiles such a closure it keeps the matches, and through
  // `parentElement` the tree they came from.
  all(predicate) {
    const out = [];
    const pending = [this];
    while (pending.length) {
      const node = pending.pop();
      if (node !== this && predicate(node)) out.push(node);
      for (let i = node.children.length - 1; i >= 0; i -= 1) pending.push(node.children[i]);
    }
    return out;
  }
  action(name) { return this.all((node) => node.dataset.facesAction === name); }
  click() { if (!this.disabled && this.onclick) return this.onclick({preventDefault() {}}); return undefined; }
  scrollIntoView() {}
}

function page() {
  const doc = {
    nodes: new Map(),
    createElement(tag) { return new Node(doc, tag); },
    createTextNode(text) { return Object.assign(new Node(doc, '#text'), {_text: text}); },
    getElementById(id) { return doc.nodes.get(id) || null; },
    querySelectorAll(selector) {
      const out = [];
      doc.nodes.forEach((root) => {
        const match = (node) => selector === 'img[data-photo-for]' && node.tagName === 'IMG' && node.dataset.photoFor;
        if (match(root)) out.push(root);
        out.push(...root.all(match));
      });
      return out;
    },
  };
  const section = new Node(doc, 'section');
  section.id = 'journal-detail-photo';
  section.hidden = true;
  doc.nodes.set(section.id, section);
  const list = new Node(doc, 'div');
  doc.nodes.set('list', list);
  return {doc, section, list};
}

function extract(workspace) {
  const start = workspace.indexOf('// --- photos of people:');
  const end = workspace.indexOf('// --- end photos of people ---');
  assert.ok(start > 0 && end > start, 'the photos block is marked in the workspace');
  return workspace.slice(start, end);
}

const settle = () => new Promise((resolve) => setImmediate(resolve));

module.exports = async function facesCases(manifestDir) {
  let cases = 0;
  const check = (condition, message) => { cases += 1; assert.ok(condition, message); };
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/entities/workspace.html'), 'utf8');
  const copy = readCopyTable(path.join(manifestDir, '../solstone-core-entities/src/data/entities_copy.json'));
  const block = extract(workspace);

  function mount(responses) {
    const {doc, section, list} = page();
    const calls = [];
    const context = {
      document: doc,
      ENT_COPY: copy,
      currentDetailEntity: {id: 'ada'},
      console,
      parseYYYYMMDD(day) { return new Date(Number(day.slice(0, 4)), Number(day.slice(4, 6)) - 1, Number(day.slice(6, 8))); },
      selectEntityScope() {},
      apiJson(url, init = {}) {
        calls.push({url, method: init.method || 'GET', body: init.body ? JSON.parse(init.body) : null});
        const answer = responses(url, init);
        return answer instanceof Error ? Promise.reject(answer) : Promise.resolve(answer);
      },
    };
    journalContext(context);
    const copyHelpers = workspace.slice(workspace.indexOf('function entityWindowArgs('), workspace.indexOf('// Every entities refusal'));
    vm.runInContext(copyHelpers, context);
    const linkHelpers = workspace.slice(workspace.indexOf("// A recording's key"), workspace.indexOf('function activeObservationGridDays('));
    vm.runInContext(linkHelpers, context);
    vm.runInContext(`${block}\nthis.api = {openFacesPanel, resetFacesPanel, facesPanel, entityPhotoThumb, facesMergeReason, renderFacetPhotoHint};`, context, {filename: 'entities-faces.js'});
    return {context, section, list, calls, api: context.api, doc};
  }
  const face = (n, extra = {}) => ({
    key: {day: '20261103', segment_key: `09000${n}_300`, frame_id: n, face_idx: 0},
    crop_url: `/crop/${n}`, source: n % 2 ? 'box' : 'anchored', day: '20261103', category: 'meeting', quality: 1 - n / 10, ...extra,
  });
  const refused = (code, status = 409) => Object.assign(new Error(code), {reasonCode: code, status});

  // Off, or nothing to choose from: the page shows nothing and reads nothing.
  {
    const m = mount(() => { throw new Error('no read expected'); });
    m.api.openFacesPanel({id: 'ada', name: 'Ada', photo_state: 'none'});
    await settle();
    check(m.section.hidden, 'a person with no photos to offer gets no panel');
    check(m.calls.length === 0, 'and no face read goes out for them');
  }

  // Suggested: one face is asked about, labelled as a suggestion, and yes
  // sends that face's key; nothing else is pre-chosen.
  {
    const panel = {state: 'suggested', faces: [face(1), face(2), face(3)], default: face(2).key, looks: 5, last_action: null};
    const m = mount((url) => (url.endsWith('/faces') ? panel : {token: 'tok-1'}));
    m.api.openFacesPanel({id: 'ada', name: 'Ada', photo_state: 'suggested'});
    await settle();
    check(!m.section.hidden && m.section.dataset.facesState === 'suggested', 'a suggested person shows the panel');
    const labels = m.section.all((node) => node.dataset.facesLabel === 'suggested');
    check(labels.length === 1 && labels[0].textContent === copy.ENT_FACES_SUGGESTED_BADGE, 'the asked-about face carries the suggested label');
    check(m.section.textContent.includes(copy.ENT_FACES_SUGGESTED_TITLE.replace('{name}', 'Ada')), 'it asks whether the face is them');
    check(m.section.all((node) => node.tagName === 'IMG').length === 1, 'only the suggested face shows until the owner asks for others');
    m.section.action('pick')[0].click();
    check(m.section.all((node) => node.tagName === 'IMG').length === 3, 'picking another shows the rest');
    check(m.section.action('choose').length === 3, 'every face shown can be chosen');
    await m.section.action('choose')[0].click();
    await settle();
    const chose = m.calls.find((call) => call.method === 'POST');
    check(chose.url.endsWith('/faces/choose') && JSON.stringify(chose.body.key) === JSON.stringify(face(2).key), 'yes sends the suggested face key as it came');
    const status = m.section.all((node) => node.dataset.facesLine === 'status')[0];
    check(status && status.textContent.includes(copy.ENT_FACES_DONE_CHOSEN.replace('{name}', 'Ada')), 'the choice is acknowledged');
    await status.children.find((node) => node.dataset.facesAction === 'undo').click();
    await settle();
    const undo = m.calls.filter((call) => call.method === 'POST').pop();
    check(undo.url.endsWith('/faces/undo') && undo.body.token === 'tok-1', 'undo sends the token the action returned');
  }

  // Possible: faces offered, nothing pre-chosen or labelled; none of these dismisses.
  {
    const panel = {state: 'possible', faces: [face(1), face(2), face(3)], default: null, looks: 3, last_action: null};
    const m = mount((url) => (url.endsWith('/faces') ? panel : {token: 'tok-2'}));
    m.api.openFacesPanel({id: 'ada', name: 'Ada', photo_state: 'possible'});
    await settle();
    check(m.section.all((node) => node.dataset.facesLabel).length === 0, 'possible faces carry no suggestion and none is pre-chosen');
    check(m.section.action('choose').length === 3 && m.section.action('reject').length === 3, 'each face can be chosen or turned down');
    await m.section.action('reject')[1].click();
    await settle();
    const rejected = m.calls.find((call) => call.method === 'POST');
    check(rejected.url.endsWith('/faces/reject') && JSON.stringify(rejected.body.key) === JSON.stringify(face(2).key), 'not them sends the face it is on');
    await m.section.action('dismiss')[0].click();
    await settle();
    check(m.calls.filter((call) => call.method === 'POST').pop().url.endsWith('/faces/dismiss'), 'none of these dismisses');
  }

  // Confirmed: the owner's own photo, from the photo route; dismissed offers the way back.
  {
    const m = mount(() => ({state: 'confirmed', faces: [face(1), face(2)], default: face(1).key, looks: 5, last_action: 'tok-c'}));
    m.api.openFacesPanel({id: 'ada', name: 'Ada', photo_state: 'confirmed'});
    await settle();
    const imgs = m.section.all((node) => node.tagName === 'IMG');
    check(imgs.length === 1 && imgs[0].src.includes('/entity/ada/photo'), 'a confirmed person shows the chosen photo from the photo route');
    check(m.section.all((node) => node.dataset.facesLabel === 'suggested').length === 0, 'a chosen photo is never labelled a suggestion');
    const d = mount(() => ({state: 'dismissed', faces: [], default: null, looks: 5, last_action: 'tok-d'}));
    d.api.openFacesPanel({id: 'ada', name: 'Ada', photo_state: 'dismissed'});
    await settle();
    await d.section.action('undo')[0].click();
    await settle();
    check(d.calls.filter((call) => call.method === 'POST').pop().body.token === 'tok-d', 'show these again undoes the latest action');
  }

  // Refusals are next steps: busy and failure retry the same act, a stale page
  // refreshes, faces off points to settings.
  for (const [code, key, next] of [
    ['faces_busy', 'ENT_FACES_BUSY', 'retry'],
    ['faces_read_failed', 'ENT_FACES_FAILED', 'retry'],
    ['face_action_changed', 'ENT_FACES_STALE', 'refresh'],
    ['face_not_found', 'ENT_FACES_GONE', 'refresh'],
    ['faces_disabled', 'ENT_FACES_DISABLED', 'settings'],
  ]) {
    let posts = 0;
    const panel = {state: 'suggested', faces: [face(1)], default: face(1).key, looks: 5, last_action: null};
    const m = mount((url, init) => {
      if (init.method !== 'POST') return panel;
      posts += 1;
      return refused(code, code === 'faces_read_failed' ? 500 : 409);
    });
    m.api.openFacesPanel({id: 'ada', name: 'Ada', photo_state: 'suggested'});
    await settle();
    await m.section.action('choose')[0].click();
    await settle();
    const line = m.section.all((node) => node.dataset.facesLine === 'error')[0];
    check(line && line.dataset.facesError === key, `${code} explains itself`);
    if (next === 'settings') {
      check(line.all((node) => node.tagName === 'A' && node.href.includes('/app/settings/')).length === 1, `${code} links to the setting`);
    } else {
      const button = line.children.find((node) => node.dataset.facesAction === next);
      check(Boolean(button), `${code} offers ${next}`);
      await button.click();
      await settle();
      if (next === 'retry') check(posts === 2, `${code} retries the same action`);
    }
  }

  // A panel that can't be read says so and can be read again.
  {
    let reads = 0;
    const m = mount(() => { reads += 1; return reads === 1 ? refused('faces_read_failed', 500) : {state: 'possible', faces: [face(1), face(2), face(3)], looks: 3}; });
    m.api.openFacesPanel({id: 'ada', name: 'Ada', photo_state: 'possible'});
    await settle();
    const line = m.section.all((node) => node.dataset.facesLine === 'error')[0];
    check(line && line.dataset.facesError === 'ENT_FACES_LOAD_FAILED', 'an unreadable panel says so');
    await line.children.find((node) => node.dataset.facesAction === 'retry').click();
    await settle();
    check(m.section.dataset.facesState === 'possible' && reads === 2, 'and try again reads it again');
  }

  // A face's day opens the meeting it came from: its day in transcripts, at
  // its recording when the face names one on that day, else the day alone.
  {
    const named = face(1, {key: {day: '20261103', stream: 'desk', stream_layout: 'named', segment_key: '090001_300', frame_id: 1, face_idx: 0}});
    const direct = face(2, {key: {day: '20261103', stream: 'desk', stream_layout: 'direct', segment_key: '090002_300', frame_id: 2, face_idx: 0}});
    const bare = face(3);
    const panel = {state: 'possible', faces: [named, direct, bare], default: null, looks: 3, last_action: null};
    const m = mount(() => panel);
    m.api.openFacesPanel({id: 'ada', name: 'Ada', photo_state: 'possible'});
    await settle();
    const days = m.section.all((node) => node.tagName === 'A' && node.className === 'entity-face-day');
    check(days.length === 3, 'every face with a day links it');
    check(days[0].href === '/app/transcripts/20261103?stream=desk#090001_300', 'a face from a named stream opens its recording');
    check(days[1].href === '/app/transcripts/20261103?stream=_default#090002_300', 'a face recorded into the day itself opens its recording');
    check(days[2].href === '/app/transcripts/20261103', 'a face that names no recording opens its day');
    check(days.every((node) => node.textContent && !node.textContent.includes('20261103')), 'the link names its day as a date');
  }

  // Lists: only a chosen photo reaches a row; a row without one is unchanged.
  {
    const m = mount(() => ({}));
    check(m.api.entityPhotoThumb({id: 'ada', has_photo: false}) === null, 'no chosen photo, no thumbnail');
    const thumb = m.api.entityPhotoThumb({id: 'ada', has_photo: true});
    check(thumb && thumb.src.endsWith('/entity/ada/photo') && thumb.dataset.photoFor === 'ada', 'a chosen photo is read from the photo route');
  }

  // Facet page: points to where a photo is chosen; shows a chosen one.
  {
    const m = mount(() => ({}));
    const indicators = new Node(m.doc, 'div');
    const header = new Node(m.doc, 'div');
    header.append(indicators);
    m.doc.nodes.set('detail-indicators', indicators);
    m.api.renderFacetPhotoHint({id: 'ada', name: 'Ada', photo_state: 'suggested', has_photo: false});
    const hint = header.children.find((node) => node.id === 'detail-photo-hint');
    check(hint && !hint.hidden && hint.all((node) => node.dataset.facesLabel === 'choose').length === 1, 'a facet page points to the photo choice');
    m.doc.nodes.set('detail-photo-hint', hint);
    m.api.renderFacetPhotoHint({id: 'ada', name: 'Ada', photo_state: 'none', has_photo: false});
    check(hint.hidden, 'and says nothing when there is nothing to choose');
  }

  // A face-based merge suggestion says why, and labels a face that isn't a chosen photo.
  {
    const m = mount((url) => (url.includes('/entity/a/') ? {state: 'suggested', faces: [face(1)], default: face(1).key} : {state: 'confirmed', faces: [], default: face(2).key}));
    const reason = m.api.facesMergeReason({pair: ['a', 'b'], names: ['A', 'B']});
    await settle();
    await settle();
    check(reason.dataset.mergeBasis === 'meeting-face' && reason.textContent.includes(copy.ENT_FACES_MERGE_REASON), 'a face-based pair says faces suggested it');
    const unconfirmed = reason.all((node) => node.dataset.facesLabel === 'unconfirmed');
    check(unconfirmed.length === 1, 'only the face that is not a chosen photo is marked not confirmed');
  }

  return cases;
};

module.exports.Node = Node;
