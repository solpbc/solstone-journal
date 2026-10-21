// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The map workspace under plain Node with a stand-in DOM and a stand-in
// Leaflet that records what is drawn.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');
const html = fs.readFileSync(path.join(crateDir, 'assets', 'workspace.html'), 'utf8');
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((match) => match[1]);
assert.strictEqual(scripts.length, 1, 'one inline workspace script');
const source = scripts[0];

const VOID_TAGS = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr']);
const BOOLEAN_ATTRS = new Set(['hidden', 'checked', 'selected', 'disabled', 'readonly', 'required', 'autofocus', 'multiple']);

class DOMNode {
  constructor(tagName = 'DIV', id = '') {
    this.tagName = tagName.toUpperCase();
    this.id = id;
    this.attributes = {};
    if (id) this.attributes.id = id;
    this.children = [];
    this.parentNode = null;
    this._innerHTML = '';
    this.listeners = {};
    this.hidden = false;
    this.isConnected = true;
    this.clientWidth = 640;
    this.classList = {
      add: (cls) => {
        const classes = (this.attributes['class'] || '').split(/\s+/).filter(Boolean);
        if (!classes.includes(cls)) classes.push(cls);
        this.attributes['class'] = classes.join(' ');
      },
      remove: (cls) => {
        const classes = (this.attributes['class'] || '').split(/\s+/).filter(Boolean);
        this.attributes['class'] = classes.filter((c) => c !== cls).join(' ');
      },
      contains: (cls) => {
        const classes = (this.attributes['class'] || '').split(/\s+/).filter(Boolean);
        return classes.includes(cls);
      },
    };
  }

  get dataset() {
    const ds = {};
    for (const [k, v] of Object.entries(this.attributes)) {
      if (k.startsWith('data-')) {
        const camel = k.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
        ds[camel] = v;
      }
    }
    return ds;
  }

  get textContent() {
    let text = '';
    for (const child of this.children) {
      if (child.nodeType === 3) text += child.text;
      else text += child.textContent;
    }
    return text;
  }

  set textContent(val) {
    this._innerHTML = escapeHtml(val);
    this.children = [{ nodeType: 3, text: String(val ?? ''), parentNode: this }];
  }

  get firstElementChild() {
    for (const c of this.children) {
      if (c.nodeType !== 3) return c;
    }
    return null;
  }

  getBoundingClientRect() {
    return { width: 640, height: 400, top: 0, left: 0, bottom: 400, right: 640 };
  }

  getAttribute(name) {
    return this.attributes[name] ?? null;
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'id') this.id = String(value);
    if (name === 'hidden') this.hidden = true;
  }

  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'id') this.id = '';
    if (name === 'hidden') this.hidden = false;
  }

  hasAttribute(name) {
    return name in this.attributes;
  }

  appendChild(child) {
    child.parentNode = this;
    this.children.push(child);
    return child;
  }

  get className() {
    return this.getAttribute('class') || '';
  }

  set className(val) {
    this.setAttribute('class', val);
  }

  get elements() {
    const elMap = {};
    for (const child of this.querySelectorAll('input, select, textarea, button')) {
      const name = child.getAttribute('name');
      if (name) elMap[name] = child;
    }
    return elMap;
  }

  get value() {
    return this.attributes['value'] ?? '';
  }

  set value(val) {
    this.attributes['value'] = String(val);
  }

  get checked() {
    return 'checked' in this.attributes;
  }

  set checked(val) {
    if (val) this.attributes['checked'] = '';
    else delete this.attributes['checked'];
  }

  get tabIndex() {
    return parseInt(this.attributes['tabindex'] ?? '-1', 10);
  }

  set tabIndex(val) {
    this.attributes['tabindex'] = String(val);
  }

  focus() {}

  addEventListener(type, handler) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push(handler);
  }

  click() {
    const event = {
      type: 'click',
      target: this,
      clientX: 0,
      clientY: 0,
      defaultPrevented: false,
      preventDefault() { this.defaultPrevented = true; },
      stopPropagation() { this._stopped = true; },
    };
    let cur = this;
    while (cur) {
      if (cur.listeners && cur.listeners['click']) {
        for (const fn of [...cur.listeners['click']]) {
          fn.call(cur, event);
          if (event._stopped) return;
        }
      }
      cur = cur.parentNode;
    }
  }

  matches(selector) {
    return matchSelector(this, selector);
  }

  closest(selector) {
    let cur = this;
    while (cur) {
      if (cur.matches && cur.matches(selector)) return cur;
      cur = cur.parentNode;
    }
    return null;
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }

  querySelectorAll(selector) {
    const parts = selector.trim().split(/\s+/);
    let current = [this];
    for (const part of parts) {
      const next = [];
      for (const node of current) {
        collectDescendants(node, part, next);
      }
      current = next;
    }
    return current;
  }

  get innerHTML() {
    if (!this.children || this.children.length === 0) {
      return this._innerHTML;
    }
    let html = '';
    for (const child of this.children) {
      if (child.nodeType === 3) {
        html += child.text;
      } else {
        const tag = child.tagName.toLowerCase();
        let attrs = '';
        for (const [k, v] of Object.entries(child.attributes)) {
          if (BOOLEAN_ATTRS.has(k)) attrs += ` ${k}`;
          else attrs += ` ${k}="${escapeHtml(v)}"`;
        }
        if (VOID_TAGS.has(tag)) {
          html += `<${tag}${attrs}>`;
        } else {
          html += `<${tag}${attrs}>${child.innerHTML}</${tag}>`;
        }
      }
    }
    return html;
  }

  set innerHTML(val) {
    this._innerHTML = String(val ?? '');
    try {
      this.children = parseHTML(this._innerHTML, this);
    } catch (e) {
      this.children = [];
    }
  }
}

function matchSelector(node, sel) {
  if (!node || node.nodeType === 3) return false;
  const regex = /([a-zA-Z0-9\-]+)|\.([a-zA-Z0-9\-_]+)|#([a-zA-Z0-9\-_]+)|\[([a-zA-Z0-9\-_]+)(?:=(?:"([^"]*)"|'([^']*)'|([^\]]+)))?\]/g;
  let match;
  let count = 0;
  while ((match = regex.exec(sel)) !== null) {
    count++;
    const [_, tag, cls, id, attr, dq, sq, raw] = match;
    if (tag) {
      if (node.tagName.toLowerCase() !== tag.toLowerCase()) return false;
    } else if (cls) {
      if (!node.classList.contains(cls)) return false;
    } else if (id) {
      if (node.id !== id && node.attributes['id'] !== id) return false;
    } else if (attr) {
      const val = dq ?? sq ?? raw;
      if (val === undefined) {
        if (!(attr in node.attributes)) return false;
      } else {
        if (node.attributes[attr] !== val) return false;
      }
    }
  }
  return count > 0;
}

function collectDescendants(parent, selectorPart, results) {
  for (const child of parent.children) {
    if (child.nodeType !== 3) {
      if (matchSelector(child, selectorPart)) {
        results.push(child);
      }
      collectDescendants(child, selectorPart, results);
    }
  }
}

function parseHTML(str, parent) {
  const rootNodes = [];
  const stack = [parent];
  const tagOrText = /<!--[\s\S]*?-->|<(?:\/([a-zA-Z0-9\-]+)|([a-zA-Z0-9\-]+)([^>]*?)(\/?)>)|([^<]+)/g;
  let match;
  while ((match = tagOrText.exec(str)) !== null) {
    if (match[0].startsWith('<!--')) continue;
    const [full, closeTag, openTag, attrStr, selfClose, text] = match;
    const currentParent = stack[stack.length - 1];
    if (text) {
      const textNode = { nodeType: 3, text, parentNode: currentParent };
      if (currentParent === parent) rootNodes.push(textNode);
      else if (currentParent) currentParent.children.push(textNode);
    } else if (closeTag) {
      const tagLower = closeTag.toLowerCase();
      for (let i = stack.length - 1; i >= 1; i--) {
        if (stack[i].tagName.toLowerCase() === tagLower) {
          stack.length = i;
          break;
        }
      }
    } else if (openTag) {
      const node = new DOMNode(openTag);
      node.parentNode = currentParent;
      if (attrStr) {
        const attrRegex = /([a-zA-Z0-9\-_]+)(?:=(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
        let attrMatch;
        while ((attrMatch = attrRegex.exec(attrStr)) !== null) {
          const attrName = attrMatch[1];
          const attrVal = attrMatch[2] ?? attrMatch[3] ?? attrMatch[4] ?? '';
          node.setAttribute(attrName, attrVal);
        }
      }
      if (currentParent === parent) rootNodes.push(node);
      else if (currentParent) currentParent.children.push(node);

      const isVoid = VOID_TAGS.has(openTag.toLowerCase()) || selfClose === '/';
      if (!isVoid) {
        stack.push(node);
      }
    }
  }
  return rootNodes;
}

function element(id) {
  return new DOMNode('div', id);
}

class ResizeObserver {
  constructor(callback) {
    this.callback = callback;
  }
  observe(target) {
    if (this.callback) this.callback([{ target }]);
  }
  unobserve() {}
  disconnect() {}
}

function escapeHtml(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function leafletRecorder() {
  const calls = { polyline: [], polygon: [], circleMarker: [], circle: [], fitBounds: 0, tooltips: [] };
  const layer = (kind, args) => {
    const object = {
      kind,
      args,
      addTo() { return object; },
      bindTooltip(node) { calls.tooltips.push(node); return object; },
      on() { return object; },
      getBounds() { return {}; },
    };
    return object;
  };
  const L = {
    map() {
      return { fitBounds() { calls.fitBounds += 1; } };
    },
    control: { scale() { return { addTo() {} }; } },
    polyline(points, style) { calls.polyline.push(points); return layer('polyline', [points, style]); },
    circleMarker(point, style) { calls.circleMarker.push(point); return layer('circleMarker', [point, style]); },
    circle(point, style) { calls.circle.push(point); return layer('circle', [point, style]); },
    polygon(rings, style) { calls.polygon.push(rings); return layer('polygon', [rings, style]); },
    featureGroup() { return layer('group', []); },
  };
  return { L, calls };
}

async function run({ pathname, responses, withLeaflet = false }) {
  const elements = {
    'map-shell': element('map-shell'),
    'map-view-root': element('map-view-root'),
    'map-state-gate': element('map-state-gate'),
    'map-canvas': element('map-canvas'),
  };
  const replaced = [];
  const fetched = [];
  const leaflet = withLeaflet ? leafletRecorder() : null;
  const window = {
    AppServices: { escapeHtml },
    SurfaceState: {
      loading: ({ text }) => `<loading>${escapeHtml(text)}</loading>`,
      empty: ({ heading, desc }) => `<empty><h>${escapeHtml(heading)}</h><p>${escapeHtml(desc)}</p></empty>`,
      error: ({ heading }) => `<error>${escapeHtml(heading)}</error>`,
    },
    ConveyIcons: { svg: () => '' },
    CONVEY_COPY: {},
    solPathContext: () => ({ day: /^\/app\/map\/(\d{8})$/.exec(pathname)?.[1] || null }),
    apiJson: async (url) => {
      fetched.push(url);
      if (!(url in responses)) throw new Error(`unexpected fetch ${url}`);
      return responses[url];
    },
    location: { pathname, replace: (url) => replaced.push(url) },
    getComputedStyle: () => ({ getPropertyValue: () => '' }),
    L: leaflet ? leaflet.L : undefined,
    addEventListener: (type, handler) => {
      if (!window._listeners[type]) window._listeners[type] = [];
      window._listeners[type].push(handler);
    },
    _listeners: {},
  };
  const document = {
    getElementById: (id) => elements[id] || null,
    documentElement: {},
    createElement: (tag = 'div') => new DOMNode(tag),
    addEventListener: (type, handler) => {
      if (!document._listeners[type]) document._listeners[type] = [];
      document._listeners[type].push(handler);
    },
    _listeners: {},
    querySelector: (sel) => document.querySelectorAll(sel)[0] || null,
    querySelectorAll: (sel) => {
      const results = [];
      for (const el of Object.values(elements)) {
        if (el.matches(sel)) results.push(el);
        results.push(...el.querySelectorAll(sel));
      }
      return results;
    },
  };
  const context = vm.createContext({
    window,
    document,
    console,
    encodeURIComponent,
    decodeURIComponent,
    Number,
    String,
    Array,
    Date,
    Promise,
    ResizeObserver,
    setTimeout,
    clearTimeout,
  });
  vm.runInContext(source, context);
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
  return { elements, replaced, fetched, calls: leaflet?.calls, workspace: window.__mapWorkspace };
}

const time = (clock, day = '20261002') => ({ utc: 'x', clock, day });
const day = {
  day: '20261002',
  devices: [{ phrase: 'your iphone', word: 'iphone', platform: 'ios', tier: 'light', readings: 2, visits: 2,
    first: time('09:00'), last: time('09:05'), interrupted_segments: 0, precision_reduced: false }],
  entries: [
    { kind: 'visit', device: 0, area: { label: 'near Denver, Colorado' }, arrival: time('18:14', '20261001'),
      departure: time('07:44'), point: [39.75, -105.0, 40],
      segments: [{ url: '/app/transcripts/20261002?stream=s#074500_300', segment: '074500_300' }] },
    { kind: 'readings', device: 0, area: { label: 'near <b>Golden</b>, Colorado' }, precision: 'precise',
      first: time('09:00'), last: time('09:05'), count: 2, points: [[39.75, -105.22, 10], [39.76, -105.22, 10]],
      segments: [
        { url: '/app/transcripts/20261002?stream=s#090000_300', segment: '090000_300' },
        { url: '/app/transcripts/20261002?stream=s#090500_300', segment: '090500_300' },
      ] },
    { kind: 'visit', device: 0, area: { label: 'near Denver, Colorado' }, arrival: null, departure: null,
      point: [39.74, -104.99, 40],
      segments: [{ url: '/app/transcripts/20261002?stream=s#180000_300', segment: '180000_300' }] },
    { kind: 'unreadable', device: 0, at: time('11:00'), lines: 1, whole_file: false,
      segments: [{ url: '/app/transcripts/20261002?stream=s#110000_300', segment: '110000_300' }] },
  ],
  map: {
    bounds: [-105.3, 39.7, -104.9, 39.8],
    towns: [{ name: 'Denver', lat: 39.74, lon: -104.98, population: 1 }],
    regions: [{ name: 'Colorado', lines: [[[41, -105], [41, -104]]] }],
    countries: [],
  },
  journal_has_location: true,
  nearby: { previous: '20261001', next: '20261003' },
};

const tests = [];
const test = (name, body) => tests.push({ name, body });

test('every supporting segment is linked from its record', async () => {
  const { elements } = await run({ pathname: '/app/map/20261002', responses: { '/app/map/api/day/20261002': day } });
  const markup = elements['map-view-root'].innerHTML;
  for (const entry of day.entries) {
    for (const segment of entry.segments) {
      assert.ok(markup.includes(escapeHtml(segment.url)), `missing link ${segment.url}`);
    }
  }
  assert.strictEqual((markup.match(/data-map-entry=/g) || []).length, day.entries.length);
});

test('a visit with no known ends shows neither end', async () => {
  const { elements } = await run({ pathname: '/app/map/20261002', responses: { '/app/map/api/day/20261002': day } });
  const markup = elements['map-view-root'].innerHTML;
  assert.ok(markup.includes('…–​…'), 'both ends unknown');
  assert.ok(markup.includes('Oct 1 18:14'), 'an arrival on the day before carries its day');
});

test('names are escaped, never markup', async () => {
  const { elements } = await run({ pathname: '/app/map/20261002', responses: { '/app/map/api/day/20261002': day } });
  const markup = elements['map-view-root'].innerHTML;
  assert.ok(!markup.includes('<b>Golden</b>'));
  assert.ok(markup.includes('&lt;b&gt;Golden&lt;/b&gt;'));
});

test('the map draws every record, a precise run as one dot, and never a line between readings', async () => {
  const { calls } = await run({ pathname: '/app/map/20261002', responses: { '/app/map/api/day/20261002': day }, withLeaflet: true });
  const boundaryLines = day.map.regions.reduce((sum, region) => sum + region.lines.length, 0);
  assert.strictEqual(calls.polyline.length, boundaryLines, 'only borders are lines');
  const recordPoints = day.entries.reduce((sum, entry) => sum + (entry.kind === 'readings'
    ? (entry.precision === 'precise' ? 1 : entry.points.length)
    : entry.kind === 'visit' ? 1 : 0), 0);
  const townPoints = day.map.towns.length;
  assert.strictEqual(calls.circleMarker.length, recordPoints + townPoints);
  assert.strictEqual(calls.fitBounds, 1);
  assert.deepStrictEqual(calls.tooltips.map((node) => node.textContent), ['Denver'], 'town labels are set as text');
});

test('a day without location offers the nearest days that have it', async () => {
  const empty = { day: '20261002', devices: [], entries: [], map: null, journal_has_location: true, nearby: { previous: '20261001', next: null } };
  const { elements } = await run({ pathname: '/app/map/20261002', responses: { '/app/map/api/day/20261002': empty } });
  assert.ok(elements['map-state-gate'].innerHTML.includes('<empty>'));
  assert.strictEqual(elements['map-view-root'].hidden, true);
});

test('the bare app opens the latest day with location, or says there is none', async () => {
  const withDays = await run({ pathname: '/app/map/', responses: { '/app/map/api/index': { coverage: { start: '20260702', end: '20261005' }, months: {} } } });
  assert.deepStrictEqual(withDays.replaced, ['/app/map/20261005']);
  const none = await run({ pathname: '/app/map/', responses: { '/app/map/api/index': { coverage: null, months: {} } } });
  assert.deepStrictEqual(none.replaced, []);
  assert.ok(none.elements['map-state-gate'].innerHTML.includes('<empty>'));
  assert.ok('data-map-empty' in none.elements['map-shell'].attributes);
});

const named = {
  ...day,
  entries: [
    { kind: 'visit', device: 0,
      area: { label: 'in Lower Downtown, Denver', town: 'Denver', town_km: 2, within: { neighbourhood: 'Lower Downtown', locality: 'Denver' } },
      place: { margin_m: 40,
        at: { name: 'Union Station', layer: 'building', class: 'train_station', inside: 'Wynkoop Plaza', outline: [[[39.75, -105.0], [39.76, -105.0], [39.76, -104.99]]] },
        near: [{ name: 'Pigtrain <Coffee>', category: 'coffee shop', distance_m: 31 }] },
      arrival: time('08:58'), departure: time('12:06'), point: [39.7527, -105.0003, 40],
      segments: [{ url: '/app/transcripts/20261002?stream=s#085500_300', segment: '085500_300' }] },
    { kind: 'visit', device: 0, area: { label: 'near Denver, Colorado', town: 'Denver', town_km: 2.1 },
      place: { margin_m: 25, at: null, near: [{ name: 'Blue Bottle', category: 'coffee shop', distance_m: 12 }, { name: 'Tattered Cover', category: '', distance_m: 20 }] },
      arrival: time('13:00'), departure: null, point: [39.75, -104.99, 10],
      segments: [{ url: '/app/transcripts/20261002?stream=s#130000_300', segment: '130000_300' }] },
  ],
  accuracy: { offered: { version: 'v1', size_bytes: 1.4e9, files: ['venues.slpk', 'containment.slpk'] },
    installed: { version: 'v1', size_bytes: 1.4e9, venues: true, containment: true }, current: true,
    attributions: ['Places from Overture Maps Foundation'], job: { state: 'idle' } },
};

test('a stop named by the download says at and near, and why', async () => {
  const { elements, calls } = await run({ pathname: '/app/map/20261002', responses: { '/app/map/api/day/20261002': named }, withLeaflet: true });
  const markup = elements['map-view-root'].innerHTML;
  assert.ok(markup.includes('<p class="map-row-title">at Union Station</p>'));
  assert.ok(markup.includes('in Wynkoop Plaza · in Lower Downtown, Denver'));
  assert.ok(markup.includes('near Pigtrain &lt;Coffee&gt;'), 'venue names are escaped');
  assert.ok(markup.includes('your phone placed you within 40 m, and all of that is inside Union Station.'));
  assert.ok(markup.includes('Pigtrain &lt;Coffee&gt; (coffee shop) is 31 m away; your phone placed you within 40 m.'));
  assert.ok(markup.includes('<p class="map-row-title">near Blue Bottle and Tattered Cover</p>'));
  assert.ok(markup.includes('Tattered Cover is 20 m away'), 'no empty category');
  assert.strictEqual(calls.polygon.length, 1, 'the outline it was inside is drawn');
  assert.ok(markup.includes('Overture Maps'), 'the download is credited');
});

test('the accuracy card offers, follows and removes the download', async () => {
  const card = (accuracy) => {
    const context = { ...day, accuracy };
    return run({ pathname: '/app/map/20261002', responses: { '/app/map/api/day/20261002': context } })
      .then(({ elements }) => elements['map-view-root'].innerHTML);
  };
  const offered = { version: 'v1', size_bytes: 1.4e9, files: ['venues.slpk'] };
  const fresh = await card({ offered, installed: null, current: false, attributions: [], job: { state: 'idle' } });
  assert.ok(fresh.includes('data-accuracy="download">download 1.4 GB</button>'));
  assert.ok(!fresh.includes('building, park or campus'), 'no outlines promised without the outline file');
  const running = await card({ offered, installed: null, current: false, attributions: [], job: { state: 'downloading', received: 4.12e8, total: 1.4e9 } });
  assert.ok(running.includes('downloading location accuracy: 412 MB of 1.4 GB'));
  const refreshing = await card({ offered, installed: { version: 'old', size_bytes: 1.4e9, venues: true }, current: false, attributions: [], job: { state: 'downloading', source: 'automatic', received: 0, total: 1.4e9 } });
  assert.ok(refreshing.includes('data-accuracy="remove">remove location accuracy</button>'));
  const done = await card({ offered, installed: { version: 'v1', size_bytes: 1.4e9, venues: true, containment: false }, current: true, attributions: [], job: { state: 'idle' } });
  assert.ok(done.includes('data-accuracy="remove">remove location accuracy</button>'));
  const full = await card({ offered: null, installed: null, current: false, attributions: [], job: { state: 'idle' } });
  assert.ok(!full.includes('aria-label="location accuracy"'), 'nothing to offer shows no card');
  const failed = await card({ offered, installed: null, current: false, attributions: [], job: { state: 'failed', reason_code: 'not_enough_space' } });
  assert.ok(failed.includes("there isn&#39;t enough free space"));
});

const stayDay = {
  ...day,
  entries: [
    { kind: 'stay', device: 0, count: 18, first: time('08:30'), last: time('09:01'), point: [39.75, -105.0, 30],
      area: { label: 'near Denver, Colorado', town: 'Denver', town_km: 2 },
      place: { margin_m: 30, at: null, near: [{ name: 'Pigtrain Coffee', category: 'coffee shop', distance_m: 12 }] },
      place_ref: { id: '0123456789ab', owner_name: 'the reading room', hidden: false },
      segments: [{ url: '/app/transcripts/20261002?stream=s#083000_300', segment: '083000_300' }] },
    { kind: 'visit', device: 0, area: { label: 'near Denver, Colorado' }, arrival: time('18:00'), departure: null,
      point: [39.74, -104.99, 20], place_ref: { id: 'abcdef012345', owner_name: null, hidden: true }, on_map: false,
      segments: [{ url: '/app/transcripts/20261002?stream=s#180000_300', segment: '180000_300' }] },
    { kind: 'readings', device: 0, area: { label: 'near Denver, Colorado' }, precision: 'approximate',
      first: time('19:00'), last: time('19:30'), count: 3,
      points: [[39.74, -104.99, 900], [39.741, -104.99, 900], [39.80, -104.95, 900]],
      map_points: [[39.80, -104.95, 900]],
      segments: [{ url: '/app/transcripts/20261002?stream=s#190000_300', segment: '190000_300' }] },
  ],
};

test('a worked-out stay says so, and a hidden place stays off the map', async () => {
  const { elements, calls } = await run({ pathname: '/app/map/20261002', responses: { '/app/map/api/day/20261002': stayDay }, withLeaflet: true });
  const markup = elements['map-view-root'].innerHTML;
  assert.ok(markup.includes('<p class="map-row-title">at the reading room</p>'), 'the owner name leads');
  assert.ok(markup.includes('a stay your journal worked out from readings from your iphone · 18 readings'));
  assert.ok(markup.includes('href="/app/map/place/0123456789ab"'));
  assert.ok(markup.includes('hidden from the map'));
  assert.ok(markup.includes('href="/app/map/week/20261002"'), 'the day links to its week');
  const townPoints = day.map.towns.length;
  assert.strictEqual(calls.circleMarker.length, 1 + 1 + townPoints, 'nothing near the hidden place is drawn');
  assert.ok(markup.includes('3 readings'), 'the list still counts every reading');
});

test('the week, places and a place page render from their own data', async () => {
  const week = { start: '20260928', previous: '20260921', next: '20261005', days: [
    { day: '20260928', readings: 0, devices: [], items: [] },
    { day: '20260929', readings: 3, devices: ['iphone'], items: [stayDay.entries[0]] },
  ] };
  const weekPage = await run({ pathname: '/app/map/week/20260929', responses: { '/app/map/api/week/20260929': week } });
  const weekMarkup = weekPage.elements['map-view-root'].innerHTML;
  assert.ok(weekMarkup.includes('the week of Monday, September 28'));
  assert.ok(weekMarkup.includes('no location kept'));
  assert.ok(weekMarkup.includes('data-kind="stay"'));
  assert.ok('data-map-page' in weekPage.elements['map-shell'].attributes, 'the day navigator is hidden');
  const place = { id: '0123456789ab', label: 'the reading room', owner_name: 'the reading room', name: 'the reading room',
    alias: 'home', hidden: false, area: { label: 'near Denver, Colorado' }, words: null, dates: 9, first: '20260929',
    last: '20261009', night_dates: 9, stays: [{ day: '20261009', start: '21:00', end: '07:00', kind: 'reported' }] };
  const places = await run({ pathname: '/app/map/places', responses: { '/app/map/api/places': { places: [place] } } });
  const placesMarkup = places.elements['map-view-root'].innerHTML;
  assert.ok(placesMarkup.includes('visits or stays here on 9 dates, Sep 29 to Oct 9.'));
  assert.ok(placesMarkup.includes('on 9 nights, a visit or stay here covers at least two hours between 22:00 and 06:00.'));
  const empty = await run({ pathname: '/app/map/places', responses: { '/app/map/api/places': { places: [] } } });
  assert.ok(empty.elements['map-view-root'].innerHTML.includes('no places yet.'));
  const page = await run({ pathname: '/app/map/place/0123456789ab', responses: { '/app/map/api/place/0123456789ab': place } });
  const pageMarkup = page.elements['map-view-root'].innerHTML;
  assert.ok(pageMarkup.includes('<option value="home" selected>home</option>'));
  assert.ok(pageMarkup.includes('hide from the map'));
  assert.ok(pageMarkup.includes('21:00–07:00 · a visit your phone reported'));
});

function makeMonth(itemsByDay = {}, opts = {}) {
  const days = [];
  for (let d = 1; d <= 31; d++) {
    const dStr = `202610${String(d).padStart(2, '0')}`;
    const dayData = itemsByDay[dStr];
    if (dayData) {
      days.push({
        day: dStr,
        has_location: dayData.has_location !== undefined ? dayData.has_location : true,
        items: dayData.items || [],
        devices: dayData.devices || [{ key: 'iphone', word: 'iphone', phrase: 'your iphone', platform: 'ios', readings: 1 }],
      });
    } else {
      days.push({
        day: dStr,
        has_location: false,
        items: [],
        devices: [],
      });
    }
  }
  return {
    month: '202610',
    previous: '202609',
    next: '202611',
    journal_has_location: opts.journal_has_location !== undefined ? opts.journal_has_location : true,
    today: opts.today || '20261031',
    before: opts.before || null,
    after: opts.after || null,
    places: opts.places || {},
    towns: opts.towns || [],
    days,
  };
}

test('arrival-only plus completed visit merge', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  // 1. Within 3 min, about 200m -> 1 stop
  const p1 = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: null, point: [39.75, -105.0, 40] },
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:03:00Z', clock: '10:03', day: '20261001' }, departure: { utc: '2026-10-01T12:00:00Z', clock: '12:00', day: '20261001' }, point: [39.7518, -105.0, 40] },
      ],
    },
  });
  const m1 = workspace.monthModel(p1);
  assert.strictEqual(m1.stops.length, 1);
  assert.strictEqual(m1.stops[0].openEnded, false);

  // 2. 4 minutes apart -> 2 stops, arrival-only is openEnded
  const p2 = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: null, point: [39.75, -105.0, 40] },
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:04:00Z', clock: '10:04', day: '20261001' }, departure: { utc: '2026-10-01T12:00:00Z', clock: '12:00', day: '20261001' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const m2 = workspace.monthModel(p2);
  assert.strictEqual(m2.stops.length, 2);
  assert.strictEqual(m2.stops[0].openEnded, true);

  // 3. 400m apart within 3 minutes -> 2 stops, arrival-only is openEnded
  const p3 = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: null, point: [39.75, -105.0, 40] },
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:02:00Z', clock: '10:02', day: '20261001' }, departure: { utc: '2026-10-01T12:00:00Z', clock: '12:00', day: '20261001' }, point: [39.754, -105.0, 40] },
      ],
    },
  });
  const m3 = workspace.monthModel(p3);
  assert.strictEqual(m3.stops.length, 2);
  assert.strictEqual(m3.stops[0].openEnded, true);
});

test('watch arrival-only and iPhone completed, 1 minute apart -> two stops', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261001': {
      devices: [
        { key: 'watch', word: 'watch', phrase: 'your watch', platform: 'watchos', readings: 1 },
        { key: 'iphone', word: 'iphone', phrase: 'your iphone', platform: 'ios', readings: 1 },
      ],
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: null, point: [39.75, -105.0, 40] },
        { kind: 'visit', device: 1, arrival: { utc: '2026-10-01T10:01:00Z', clock: '10:01', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.stops.length, 2);
});

test('day N devices [watch, iphone], day N+1 devices [iphone, watch] overnight iPhone visit is one stop', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261001': {
      devices: [{ key: 'watch' }, { key: 'iphone' }],
      items: [
        { kind: 'visit', device: 1, arrival: { utc: '2026-10-01T22:00:00Z', clock: '22:00', day: '20261001' }, departure: null, point: [39.75, -105.0, 40] },
      ],
    },
    '20261002': {
      devices: [{ key: 'iphone' }, { key: 'watch' }],
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T22:00:00Z', clock: '22:00', day: '20261001' }, departure: { utc: '2026-10-02T07:00:00Z', clock: '07:00', day: '20261002' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.stops.length, 1);
  assert.strictEqual(m.stops[0].device, 'iphone');
});

test('two identical visits -> one stop', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] },
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.stops.length, 1);
});

test('arrival-only 19:00 on 20261031 plus after departing 07:00 on 20261101 -> one stop', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth(
    {
      '20261031': {
        items: [
          { kind: 'visit', device: 0, arrival: { utc: '2026-10-31T19:00:00Z', clock: '19:00', day: '20261031' }, departure: null, point: [39.75, -105.0, 40] },
        ],
      },
    },
    {
      after: {
        devices: [{ key: 'iphone' }],
        items: [
          { kind: 'visit', device: 0, arrival: { utc: '2026-10-31T19:00:00Z', clock: '19:00', day: '20261031' }, departure: { utc: '2026-11-01T07:00:00Z', clock: '07:00', day: '20261101' }, point: [39.75, -105.0, 40] },
        ],
      },
    }
  );
  const m = workspace.monthModel(p);
  assert.strictEqual(m.stops.length, 1);
  assert.deepEqual(m.stops[0].days, ['20261031']);
  assert.strictEqual(m.stops[0].openEnded, false);
  assert.strictEqual(m.stops[0].clipped, true);
  assert.ok(m.choices[0].areas[0].nightDays.includes('20261031'));
});

test('completed after report arriving 20261031 22:00 departing 20261101 without arrival-only partner -> one stop on 20261031', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth(
    {},
    {
      after: {
        devices: [{ key: 'iphone' }],
        items: [
          { kind: 'visit', device: 0, arrival: { utc: '2026-10-31T22:00:00Z', clock: '22:00', day: '20261031' }, departure: { utc: '2026-11-01T04:00:00Z', clock: '04:00', day: '20261101' }, point: [39.75, -105.0, 40] },
        ],
      },
    }
  );
  const m = workspace.monthModel(p);
  assert.strictEqual(m.stops.length, 1);
  assert.deepEqual(m.stops[0].days, ['20261031']);
});

test('after item wholly on 20261101 is absent from stops', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth(
    {},
    {
      after: {
        devices: [{ key: 'iphone' }],
        items: [
          { kind: 'visit', device: 0, arrival: { utc: '2026-11-01T09:00:00Z', clock: '09:00', day: '20261101' }, departure: { utc: '2026-11-01T11:00:00Z', clock: '11:00', day: '20261101' }, point: [39.75, -105.0, 40] },
        ],
      },
    }
  );
  const m = workspace.monthModel(p);
  assert.strictEqual(m.stops.length, 0);
});

test('run of far drawable days is one trip; hidden far stop makes no trip', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p1 = makeMonth({
    '20261001': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] }] },
    '20261002': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-02T10:00:00Z', clock: '10:00', day: '20261002' }, departure: { utc: '2026-10-02T11:00:00Z', clock: '11:00', day: '20261002' }, point: [40.5, -105.0, 40] }] },
    '20261003': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-03T10:00:00Z', clock: '10:00', day: '20261003' }, departure: { utc: '2026-10-03T11:00:00Z', clock: '11:00', day: '20261003' }, point: [40.5, -105.0, 40] }] },
    '20261004': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-04T10:00:00Z', clock: '10:00', day: '20261004' }, departure: { utc: '2026-10-04T11:00:00Z', clock: '11:00', day: '20261004' }, point: [39.75, -105.0, 40] }] },
    '20261005': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-05T10:00:00Z', clock: '10:00', day: '20261005' }, departure: { utc: '2026-10-05T11:00:00Z', clock: '11:00', day: '20261005' }, point: [39.75, -105.0, 40] }] },
  });
  const m1 = workspace.monthModel(p1);
  assert.strictEqual(m1.choices.length, 2);
  assert.strictEqual(m1.choices[1].trip, true);
  assert.deepEqual(m1.choices[1].days, ['20261002', '20261003']);

  const p2 = makeMonth({
    '20261001': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] }] },
    '20261002': { items: [{ kind: 'visit', device: 0, on_map: false, arrival: { utc: '2026-10-02T10:00:00Z', clock: '10:00', day: '20261002' }, departure: { utc: '2026-10-02T11:00:00Z', clock: '11:00', day: '20261002' }, point: [40.5, -105.0, 40] }] },
    '20261004': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-04T10:00:00Z', clock: '10:00', day: '20261004' }, departure: { utc: '2026-10-04T11:00:00Z', clock: '11:00', day: '20261004' }, point: [39.75, -105.0, 40] }] },
    '20261005': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-05T10:00:00Z', clock: '10:00', day: '20261005' }, departure: { utc: '2026-10-05T11:00:00Z', clock: '11:00', day: '20261005' }, point: [39.75, -105.0, 40] }] },
  });
  const m2 = workspace.monthModel(p2);
  assert.strictEqual(m2.choices.length, 1);
  assert.strictEqual(m2.choices[0].trip, false);
});

test('far day whose only stop has margin 2000 gives no trip, stop is in wideStops', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261001': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] }] },
    '20261002': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-02T10:00:00Z', clock: '10:00', day: '20261002' }, departure: { utc: '2026-10-02T11:00:00Z', clock: '11:00', day: '20261002' }, point: [40.5, -105.0, 2000] }] },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.choices.length, 1);
  assert.strictEqual(m.choices[0].trip, false);
  assert.deepEqual(m.choices[0].notDrawn.wideStops, [1]);
  assert.ok(m.choices[0].days.includes('20261002'));
  assert.ok(m.days.find((d) => d.day === '20261002').inChoices.includes('home'));
});

test('every stop margin 2000 or null gives anchorId null, choices empty, notDrawn.wide === stops.length', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p1 = makeMonth({
    '20261001': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 2000] }] },
  });
  const m1 = workspace.monthModel(p1);
  assert.strictEqual(m1.anchorId, null);
  assert.deepEqual(m1.choices, []);
  assert.strictEqual(m1.notDrawn.wide, m1.stops.length);

  const p2 = makeMonth({
    '20261001': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, null] }] },
  });
  const m2 = workspace.monthModel(p2);
  assert.strictEqual(m2.anchorId, null);
  assert.deepEqual(m2.choices, []);
  assert.strictEqual(m2.notDrawn.wide, m2.stops.length);
});

test('two visits at one place on different days -> one area, dayCount 2 with places as object or array', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const items = {
    '20261001': { items: [{ kind: 'visit', device: 0, place_ref: { id: 'p1' }, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.7501, -105.0001, 30] }] },
    '20261002': { items: [{ kind: 'visit', device: 0, place_ref: { id: 'p1' }, arrival: { utc: '2026-10-02T10:00:00Z', clock: '10:00', day: '20261002' }, departure: { utc: '2026-10-02T11:00:00Z', clock: '11:00', day: '20261002' }, point: [39.7502, -105.0002, 30] }] },
  };
  const pObj = makeMonth(items, { places: { p1: { id: 'p1', lat: 39.75, lon: -105.0 } } });
  const mObj = workspace.monthModel(pObj);
  assert.strictEqual(mObj.choices[0].areas.length, 1);
  assert.strictEqual(mObj.choices[0].areas[0].dayCount, 2);

  const pArr = makeMonth(items, { places: [{ id: 'p1', lat: 39.75, lon: -105.0 }] });
  const mArr = workspace.monthModel(pArr);
  assert.strictEqual(mArr.choices[0].areas.length, 1);
  assert.strictEqual(mArr.choices[0].areas[0].dayCount, 2);
  assert.strictEqual(mArr.choices[0].areas[0].id, mObj.choices[0].areas[0].id);
});

test('three on_map: false visits within 30 m, margin 80, no place_ref -> hiddenPlaces 1', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, on_map: false, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 80] },
        { kind: 'visit', device: 0, on_map: false, arrival: { utc: '2026-10-01T12:00:00Z', clock: '12:00', day: '20261001' }, departure: { utc: '2026-10-01T13:00:00Z', clock: '13:00', day: '20261001' }, point: [39.7501, -105.0, 80] },
        { kind: 'visit', device: 0, on_map: false, arrival: { utc: '2026-10-01T14:00:00Z', clock: '14:00', day: '20261001' }, departure: { utc: '2026-10-01T15:00:00Z', clock: '15:00', day: '20261001' }, point: [39.7502, -105.0, 80] },
      ],
    },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.notDrawn.hiddenPlaces, 1);
});

test('places contains p1, and the only stop is on_map: false, no place_ref, about 20 m from p1 -> hiddenPlaces is 1', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth(
    {
      '20261001': {
        items: [
          { kind: 'visit', device: 0, on_map: false, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75018, -105.0, 40] },
        ],
      },
    },
    { places: [{ id: 'p1', lat: 39.75, lon: -105.0 }] }
  );
  const m = workspace.monthModel(p);
  assert.strictEqual(m.notDrawn.hiddenPlaces, 1);
});

test('one visit 23:00-07:00 -> nights 1; two separate one-hour visits -> nights 0', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p1 = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T23:00:00Z', clock: '23:00', day: '20261001' }, departure: { utc: '2026-10-02T07:00:00Z', clock: '07:00', day: '20261002' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const m1 = workspace.monthModel(p1);
  assert.strictEqual(m1.choices[0].areas[0].nights, 1);

  const p2 = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T23:00:00Z', clock: '23:00', day: '20261001' }, departure: { utc: '2026-10-02T00:00:00Z', clock: '00:00', day: '20261002' }, point: [39.75, -105.0, 40] },
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-02T00:00:00Z', clock: '00:00', day: '20261002' }, departure: { utc: '2026-10-02T01:00:00Z', clock: '01:00', day: '20261002' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const m2 = workspace.monthModel(p2);
  assert.strictEqual(m2.choices[0].areas[0].nights, 0);
});

test('stop on 20261031 is in stops', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261031': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-31T10:00:00Z', clock: '10:00', day: '20261031' }, departure: { utc: '2026-10-31T11:00:00Z', clock: '11:00', day: '20261031' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.stops.length, 1);
  assert.deepEqual(m.stops[0].days, ['20261031']);
});

test('today 20261015: days 16-31 are future, m is 15, noLocation does not include them', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth(
    {
      '20261001': { items: [] },
      '20261020': { has_location: true, items: [], devices: [] },
    },
    { today: '20261015' }
  );
  const m = workspace.monthModel(p);
  assert.strictEqual(m.days.find((d) => d.day === '20261020').state, 'kept');
  assert.strictEqual(m.days.find((d) => d.day === '20261016').state, 'future');
  assert.strictEqual(m.days.find((d) => d.day === '20261031').state, 'future');
  assert.strictEqual(m.m, 16);

  const p2 = makeMonth({}, { today: '20261015' });
  const m2 = workspace.monthModel(p2);
  for (let d = 16; d <= 31; d++) {
    const dStr = `202610${d}`;
    assert.strictEqual(m2.days.find((x) => x.day === dStr).state, 'future');
  }
  assert.strictEqual(m2.m, 15);
  assert.strictEqual(m2.noLocation, 15);
});

test('unreadable-only has state unreadable, not no-stops', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261001': {
      devices: [{ key: 'iphone', readings: 0, unreadable_lines: 5 }],
      items: [],
    },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.days.find((d) => d.day === '20261001').state, 'unreadable');
});

test('readings on every day, no items, today 20261031: anchorId null, choices empty, every day no-stops', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const itemsByDay = {};
  for (let d = 1; d <= 31; d++) {
    const dStr = `202610${String(d).padStart(2, '0')}`;
    itemsByDay[dStr] = {
      devices: [{ key: 'iphone', readings: 3 }],
      items: [],
    };
  }
  const p = makeMonth(itemsByDay, { today: '20261031' });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.anchorId, null);
  assert.deepEqual(m.choices, []);
  assert.ok(m.days.every((d) => d.state === 'no-stops'));
});

test('one drawable stop, area.town null, area.label Wyoming -> home name around Wyoming, trip Wyoming', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, area: { town: null, label: 'Wyoming' }, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] },
      ],
    },
    '20261002': {
      items: [
        { kind: 'visit', device: 0, area: { town: null, label: 'Wyoming' }, arrival: { utc: '2026-10-02T10:00:00Z', clock: '10:00', day: '20261002' }, departure: { utc: '2026-10-02T11:00:00Z', clock: '11:00', day: '20261002' }, point: [40.5, -105.0, 40] },
      ],
    },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.choices[0].name, 'around Wyoming');
  assert.strictEqual(m.choices[1].name, 'Wyoming');
});

test('owner name cabin, area.label with no ", " -> words cabin, listLabel cabin, sharedRegion null', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, place_ref: { id: 'p1', owner_name: 'cabin' }, area: { label: 'Woods' }, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.stops[0].words, 'cabin');
  assert.strictEqual(m.choices[0].areas[0].listLabel, 'cabin');
  assert.strictEqual(m.choices[0].sharedRegion, null);
});

test('sharedRegion with comma labels', async () => {
  const { workspace } = await run({ pathname: '/app/map/month/202610', responses: {} });
  const p = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, area: { label: 'Library, Colorado' }, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] },
        { kind: 'visit', device: 0, area: { label: 'Park, Colorado' }, arrival: { utc: '2026-10-01T12:00:00Z', clock: '12:00', day: '20261001' }, departure: { utc: '2026-10-01T13:00:00Z', clock: '13:00', day: '20261001' }, point: [39.77, -105.0, 40] },
        { kind: 'visit', device: 0, area: { label: 'Wyoming' }, arrival: { utc: '2026-10-01T14:00:00Z', clock: '14:00', day: '20261001' }, departure: { utc: '2026-10-01T15:00:00Z', clock: '15:00', day: '20261001' }, point: [39.79, -105.0, 40] },
      ],
    },
  });
  const m = workspace.monthModel(p);
  assert.strictEqual(m.choices[0].sharedRegion, 'Colorado');
  const lib = m.choices[0].areas.find((a) => a.words === 'Library, Colorado');
  const park = m.choices[0].areas.find((a) => a.words === 'Park, Colorado');
  const wy = m.choices[0].areas.find((a) => a.words === 'Wyoming');
  assert.strictEqual(lib.listLabel, 'Library');
  assert.strictEqual(park.listLabel, 'Park');
  assert.strictEqual(wy.listLabel, 'Wyoming');
});

test('no-anchor month: every stop on_map: false shows "no stops to draw this month." and "a place you hid"', async () => {
  const p = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, on_map: false, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const { elements } = await run({ pathname: '/app/map/month/202610', responses: { '/app/map/api/month/202610': p } });
  assert.strictEqual(elements['map-view-root'].hidden, false);
  assert.ok(!elements['map-state-gate'].innerHTML.includes('<error>'));
  assert.ok(elements['map-view-root'].innerHTML.includes('no stops to draw this month.'));
  assert.ok(elements['map-view-root'].innerHTML.includes('a place you hid'));
  assert.strictEqual(elements['map-view-root'].querySelector('[data-map-month-notdrawn]').innerHTML, '');
});

test('no-anchor month: every margin 2000 shows "no stops to draw this month." and margin line', async () => {
  const p = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 2000] },
      ],
    },
  });
  const { elements } = await run({ pathname: '/app/map/month/202610', responses: { '/app/map/api/month/202610': p } });
  assert.strictEqual(elements['map-view-root'].hidden, false);
  assert.ok(!elements['map-state-gate'].innerHTML.includes('<error>'));
  assert.ok(elements['map-view-root'].innerHTML.includes('no stops to draw this month.'));
  assert.ok(elements['map-view-root'].innerHTML.includes('a margin of error over 500 m or not reported'));
  assert.strictEqual(elements['map-view-root'].querySelector('[data-map-month-notdrawn]').innerHTML, '');
});

test('no-anchor month: every margin null shows "no stops to draw this month." and margin line', async () => {
  const p = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, null] },
      ],
    },
  });
  const { elements } = await run({ pathname: '/app/map/month/202610', responses: { '/app/map/api/month/202610': p } });
  assert.strictEqual(elements['map-view-root'].hidden, false);
  assert.ok(!elements['map-state-gate'].innerHTML.includes('<error>'));
  assert.ok(elements['map-view-root'].innerHTML.includes('no stops to draw this month.'));
  assert.ok(elements['map-view-root'].innerHTML.includes('a margin of error over 500 m or not reported'));
  assert.strictEqual(elements['map-view-root'].querySelector('[data-map-month-notdrawn]').innerHTML, '');
});

test('more than eight areas: click [data-map-month-areas-more], then svg data-cell equals areas list data-cell', async () => {
  const items = [];
  for (let i = 0; i < 10; i++) {
    const h = String(10 + i).padStart(2, '0');
    items.push({
      kind: 'visit',
      device: 0,
      area: { label: `Area ${i}` },
      arrival: { utc: `2026-10-01T${h}:00:00Z`, clock: `${h}:00`, day: '20261001' },
      departure: { utc: `2026-10-01T${h}:30:00Z`, clock: `${h}:30`, day: '20261001' },
      point: [39.75 + i * 0.02, -105.0, 40],
    });
  }
  const p = makeMonth({ '20261001': { items } });
  const { elements } = await run({ pathname: '/app/map/month/202610', responses: { '/app/map/api/month/202610': p } });
  const moreBtn = elements['map-view-root'].querySelector('[data-map-month-areas-more]');
  assert.ok(moreBtn);
  moreBtn.click();

  const svgCells = elements['map-view-root'].querySelectorAll('[data-map-month-svg] [data-cell]').map((el) => el.getAttribute('data-cell')).sort();
  const listCells = elements['map-view-root'].querySelectorAll('[data-map-month-areas] [data-cell]').map((el) => el.getAttribute('data-cell')).sort();
  assert.strictEqual(svgCells.length, 10);
  assert.strictEqual(listCells.length, 10);
  assert.deepEqual(svgCells, listCells);
});

test('empty month with journal_has_location: true and apiJson throwing: root contains "no location kept for this month.", no nearest-month link', async () => {
  const p = makeMonth({}, { journal_has_location: true });
  const { elements } = await run({ pathname: '/app/map/month/202610', responses: { '/app/map/api/month/202610': p } });
  assert.ok(!elements['map-state-gate'].innerHTML.includes('<error>'));
  assert.ok(elements['map-view-root'].innerHTML.includes('no location kept for this month.'));
  const monthLinks = [...elements['map-view-root'].innerHTML.matchAll(/href="\/app\/map\/month\//g)];
  assert.strictEqual(monthLinks.length, 0);
});

test('day page contains this month link, week page contains this month link, month page contains nav links', async () => {
  const dayRes = await run({ pathname: '/app/map/20261002', responses: { '/app/map/api/day/20261002': day } });
  assert.ok(dayRes.elements['map-view-root'].innerHTML.includes('href="/app/map/month/202610"'));
  assert.ok(dayRes.elements['map-view-root'].innerHTML.includes('href="/app/map/week/20261002"'));

  const week = { start: '20260928', previous: '20260921', next: '20261005', days: [
    { day: '20260928', readings: 0, devices: [], items: [] },
  ] };
  const weekRes = await run({ pathname: '/app/map/week/20260928', responses: { '/app/map/api/week/20260928': week } });
  assert.ok(weekRes.elements['map-view-root'].innerHTML.includes('href="/app/map/month/202609"'));

  const p = makeMonth({
    '20261001': {
      items: [
        { kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] },
      ],
    },
  });
  const monthRes = await run({ pathname: '/app/map/month/202610', responses: { '/app/map/api/month/202610': p } });
  const markup = monthRes.elements['map-view-root'].innerHTML;
  assert.ok(markup.includes('href="/app/map/"'));
  assert.ok(markup.includes('href="/app/map/places"'));
  assert.ok(markup.includes('href="/app/map/month/202609"'));
  assert.ok(markup.includes('href="/app/map/month/202611"'));
});

test('calendar keydown End on month with today 20261015 and no later location lands on 20261015', async () => {
  const p = makeMonth(
    {
      '20261001': { items: [{ kind: 'visit', device: 0, arrival: { utc: '2026-10-01T10:00:00Z', clock: '10:00', day: '20261001' }, departure: { utc: '2026-10-01T11:00:00Z', clock: '11:00', day: '20261001' }, point: [39.75, -105.0, 40] }] },
    },
    { today: '20261015' }
  );
  const { elements } = await run({ pathname: '/app/map/month/202610', responses: { '/app/map/api/month/202610': p } });
  const calEl = elements['map-view-root'].querySelector('[data-map-month-cal]');
  assert.ok(calEl.listeners['keydown']?.length > 0);
  calEl.listeners['keydown'][0]({ key: 'End', preventDefault() {} });
  const zeroButtons = elements['map-view-root'].querySelectorAll('[data-map-month-cal] button[tabindex="0"]');
  assert.strictEqual(zeroButtons.length, 1);
  assert.strictEqual(zeroButtons[0].getAttribute('data-day'), '20261015');
});

(async () => {
  for (const { name, body } of tests) {
    try {
      await body();
    } catch (error) {
      console.error(`FAILED: ${name}\n${error.stack}`);
      process.exit(1);
    }
  }
  console.log(`DOM CASES: ${tests.length} passed`);
})();
