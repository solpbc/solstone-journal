// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The view contract, the read store and the in-page router, on a small
// document with a history, a simulated clock and a server the test answers:
// what a view leaves behind when it goes, what a failed mount takes back,
// which moves stay in the page and which load a new one, who owns each
// history entry, and what a screen reader hears.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');
const ORIGIN = 'http://journal.test';

function staticSource(name) {
  return fs.readFileSync(path.join(crateDir, 'assets/static', name), 'utf8');
}

async function flush() {
  for (let i = 0; i < 30; i += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

function makeClock() {
  let now = 0;
  let sequence = 0;
  const timers = new Map();
  const add = (fn, ms, repeat, args) => {
    sequence += 1;
    const delay = Math.max(0, Number(ms) || 0);
    timers.set(sequence, { at: now + delay, fn, args, every: repeat ? Math.max(1, delay) : 0 });
    return sequence;
  };
  return {
    now: () => now,
    count: () => timers.size,
    setTimeout: (fn, ms, ...args) => add(fn, ms, false, args),
    setInterval: (fn, ms, ...args) => add(fn, ms, true, args),
    clearTimeout: (id) => timers.delete(id),
    clearInterval: (id) => timers.delete(id),
    async advance(ms) {
      const end = now + ms;
      for (;;) {
        await flush();
        let next = null;
        for (const [id, timer] of timers) {
          if (timer.at <= end && (!next || timer.at < next[1].at)) next = [id, timer];
        }
        if (!next) break;
        const [id, timer] = next;
        now = timer.at;
        if (timer.every) timer.at += timer.every;
        else timers.delete(id);
        timer.fn(...(timer.args || []));
      }
      now = end;
      await flush();
    },
  };
}

function abortError() {
  return new DOMException('The operation was aborted.', 'AbortError');
}

// Every call is recorded. A route in `answers` is answered at once; anything
// else waits for the test.
function makeServer() {
  const calls = [];
  // Setting an answer also answers the calls already waiting for it.
  const answers = new (class extends Map {
    set(url, fn) {
      super.set(url, fn);
      if (fn) calls.filter((call) => !call.settled && call.url === url).forEach((call) => setImmediate(() => fn(call)));
      return this;
    }
  })();
  function fetch(input, init = {}) {
    const url = new URL(String(input), ORIGIN + '/');
    return new Promise((resolve, reject) => {
      const call = {
        url: url.pathname + url.search,
        method: String(init.method || 'GET').toUpperCase(),
        aborted: false,
        settled: false,
        respond(status, text = '', headers = {}) {
          if (call.settled) return;
          call.settled = true;
          resolve(new Response(text, { status, headers }));
        },
        failTransport() {
          if (call.settled) return;
          call.settled = true;
          reject(new TypeError('Failed to fetch'));
        },
      };
      const signal = init.signal;
      if (signal) {
        const onAbort = () => {
          call.aborted = true;
          if (!call.settled) {
            call.settled = true;
            reject(abortError());
          }
        };
        if (signal.aborted) onAbort();
        else signal.addEventListener('abort', onAbort, { once: true });
      }
      calls.push(call);
      const answer = answers.get(call.url) || answers.get(url.pathname);
      if (answer) setImmediate(() => answer(call));
    });
  }
  const json = (body, status = 200) => (call) => call.respond(status, JSON.stringify(body), { 'content-type': 'application/json' });
  const html = (body) => (call) => call.respond(200, body, { 'content-type': 'text/html; charset=utf-8' });
  return {
    fetch, calls, answers, json, html,
    of: (url) => calls.filter((call) => call.url === url),
    pending: (url) => calls.filter((call) => call.url === url && !call.settled),
  };
}

// ---- a small document

class FakeEventTarget {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, fn, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    const list = this.listeners.get(type) || [];
    if (list.some((entry) => entry.fn === fn && entry.capture === capture)) return;
    list.push({ fn, capture, once: !!options?.once });
    this.listeners.set(type, list);
  }
  removeEventListener(type, fn, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    const list = this.listeners.get(type) || [];
    this.listeners.set(type, list.filter((entry) => !(entry.fn === fn && entry.capture === capture)));
  }
  dispatchEvent(event) {
    event.currentTarget = this;
    (this.listeners.get(event.type) || []).slice().forEach((entry) => {
      if (entry.once) this.removeEventListener(event.type, entry.fn, entry.capture);
      entry.fn.call(this, event);
    });
    return !event.defaultPrevented;
  }
  listenerCount(type) {
    if (type) return (this.listeners.get(type) || []).length;
    let total = 0;
    this.listeners.forEach((list) => { total += list.length; });
    return total;
  }
}

class ClassList {
  constructor(element) { this.element = element; }
  get values() { return new Set((this.element.getAttribute('class') || '').split(/\s+/).filter(Boolean)); }
  write(values) {
    if (values.size) this.element.setAttribute('class', [...values].join(' '));
    else this.element.removeAttribute('class');
  }
  add(...names) { const v = this.values; names.forEach((n) => v.add(n)); this.write(v); }
  remove(...names) { const v = this.values; names.forEach((n) => v.delete(n)); this.write(v); }
  toggle(name, force) {
    const v = this.values;
    const on = force === undefined ? !v.has(name) : !!force;
    if (on) v.add(name); else v.delete(name);
    this.write(v);
    return on;
  }
  contains(name) { return this.values.has(name); }
}

const VOID = new Set(['link', 'meta', 'br', 'img', 'input']);

class FakeNode extends FakeEventTarget {
  constructor(document, tagName) {
    super();
    this.ownerDocument = document;
    this.tagName = tagName ? tagName.toUpperCase() : '#fragment';
    this.attributeMap = new Map();
    this.children = [];
    this.parent = null;
    this.text = '';
    this.classList = new ClassList(this);
    this.disabled = false;
  }
  get attributes() { return Array.from(this.attributeMap, ([name, value]) => ({ name, value })); }
  setAttribute(name, value) { this.attributeMap.set(name, String(value)); }
  getAttribute(name) { return this.attributeMap.has(name) ? this.attributeMap.get(name) : null; }
  hasAttribute(name) { return this.attributeMap.has(name); }
  removeAttribute(name) { this.attributeMap.delete(name); }
  get id() { return this.getAttribute('id') || ''; }
  get href() { return this.hasAttribute('href') ? new URL(this.getAttribute('href'), this.ownerDocument.location.href).href : ''; }
  get src() { return this.hasAttribute('src') ? new URL(this.getAttribute('src'), ORIGIN).href : ''; }
  get target() { return this.getAttribute('target') || ''; }
  get textContent() { return this.text + this.children.map((child) => child.textContent).join(''); }
  set textContent(value) { this.children = []; this.text = String(value); }
  get isConnected() {
    let node = this;
    while (node.parent) node = node.parent;
    return node === this.ownerDocument.root;
  }
  appendChild(child) {
    if (child.tagName === '#fragment') {
      child.children.slice().forEach((node) => this.appendChild(node));
      return child;
    }
    child.remove();
    child.parent = this;
    this.children.push(child);
    if (this.isConnected) this.ownerDocument.inserted?.(child);
    return child;
  }
  replaceChildren(...nodes) {
    this.children.slice().forEach((child) => child.remove());
    this.text = '';
    nodes.forEach((node) => this.appendChild(node));
  }
  remove() {
    if (!this.parent) return;
    const active = this.ownerDocument.activeElement;
    this.parent.children = this.parent.children.filter((child) => child !== this);
    this.parent = null;
    // As in a browser, focus on a node taken out falls to the body.
    if (active && this.walk().includes(active)) this.ownerDocument.activeElement = this.ownerDocument.body;
  }
  contains(other) { return this.walk().includes(other); }
  replaceWith(other) {
    const parent = this.parent;
    const index = parent.children.indexOf(this);
    other.remove();
    parent.children[index] = other;
    other.parent = parent;
    this.parent = null;
    if (parent.isConnected) this.ownerDocument.inserted?.(other);
  }
  cloneNode(deep) {
    const copy = new FakeNode(this.ownerDocument, this.tagName === '#fragment' ? null : this.tagName);
    this.attributeMap.forEach((value, name) => copy.setAttribute(name, value));
    copy.text = this.text;
    if (deep) this.children.forEach((child) => { const c = child.cloneNode(true); c.parent = copy; copy.children.push(c); });
    return copy;
  }
  get content() { return this._content; }
  set innerHTML(html) {
    this.children.slice().forEach((child) => child.remove());
    this.text = '';
    parseInto(this, String(html));
    if (this.isConnected) this.walk().forEach((node) => node !== this && this.ownerDocument.inserted?.(node));
  }
  get innerHTML() { return this.textContent; }
  walk() { return [this, ...this.children.flatMap((child) => child.walk())]; }
  querySelectorAll(selector) { return this.walk().slice(1).filter((node) => matches(node, selector)); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  closest(selector) {
    let node = this;
    while (node && node.tagName !== '#fragment') {
      if (matches(node, selector)) return node;
      node = node.parent;
    }
    return null;
  }
  focus() { this.ownerDocument.activeElement = this; }
  // A click bubbles from the element to the document.
  click() {
    const event = { type: 'click', target: this, button: 0, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
    for (let node = this; node; node = node.parent) FakeEventTarget.prototype.dispatchEvent.call(node, event);
    this.ownerDocument.dispatchEvent(event);
    return event;
  }
}

// Markup the tests write: elements with quoted attributes, and text.
function parseInto(root, html) {
  const document = root.ownerDocument;
  const stack = [root];
  const tokens = /<!--[\s\S]*?-->|<\/([a-z0-9]+)\s*>|<([a-z0-9]+)((?:\s+[\w-]+(?:="[^"]*")?)*)\s*\/?>|([^<]+)/gi;
  let match;
  while ((match = tokens.exec(html))) {
    const top = stack[stack.length - 1];
    if (match[0].startsWith('<!--')) continue;
    if (match[1]) {
      while (stack.length > 1 && stack.pop().tagName !== match[1].toUpperCase());
      continue;
    }
    if (match[2]) {
      const element = new FakeNode(document, match[2]);
      const attributes = /([\w-]+)(?:="([^"]*)")?/g;
      let attribute;
      while ((attribute = attributes.exec(match[3] || ''))) element.setAttribute(attribute[1], attribute[2] ?? '');
      element.parent = top;
      top.children.push(element);
      if (match[2].toLowerCase() === 'template') {
        element._content = new FakeNode(document, null);
      }
      if (!VOID.has(match[2].toLowerCase()) && !match[0].endsWith('/>')) stack.push(element);
      continue;
    }
    if (match[4] && match[4].trim()) {
      top.text += match[4];
    }
  }
}

function matchesSimple(node, part) {
  const parsed = /^([a-z0-9]+)?(?:#([\w-]+))?(?:\.([\w-]+))?(?:\[([\w-]+)(?:="?([^"\]]*)"?)?\])?$/i.exec(part);
  if (!parsed) return false;
  const [, tag, id, cls, attribute, value] = parsed;
  if (tag && node.tagName !== tag.toUpperCase()) return false;
  if (id && node.id !== id) return false;
  if (cls && !node.classList.contains(cls)) return false;
  if (attribute && !node.hasAttribute(attribute)) return false;
  if (attribute && value !== undefined && node.getAttribute(attribute) !== value) return false;
  return true;
}

function matches(node, selector) {
  if (!node.tagName || node.tagName === '#fragment') return false;
  return selector.split(',').some((group) => {
    const parts = group.trim().split(/\s+/);
    if (!matchesSimple(node, parts[parts.length - 1])) return false;
    let ancestor = node.parent;
    for (let i = parts.length - 2; i >= 0; i -= 1) {
      while (ancestor && !(ancestor.tagName !== '#fragment' && matchesSimple(ancestor, parts[i]))) ancestor = ancestor.parent;
      if (!ancestor) return false;
      ancestor = ancestor.parent;
    }
    return true;
  });
}

// ---- the history: entries of this document, and one before it

function makeHistory(window, location, record) {
  const entries = [{ url: 'http://elsewhere.test/', state: null, other: true }];
  let index = 0;
  const history = {
    scrollRestoration: 'auto',
    get length() { return entries.length; },
    get state() { return entries[index].state; },
    pushState(state, _title, url) {
      entries.splice(index + 1);
      entries.push({ url: new URL(url, location.href).href, state: structuredClone(state) });
      index += 1;
      location.set(entries[index].url);
    },
    replaceState(state, _title, url) {
      entries[index] = { url: url === undefined ? entries[index].url : new URL(url, location.href).href, state: structuredClone(state) };
      location.set(entries[index].url);
    },
    // A move to an entry of another document is the browser's: this page goes.
    async go(delta) {
      const next = index + delta;
      if (next < 0 || next >= entries.length) return;
      index = next;
      if (entries[index].other) {
        record.left.push(entries[index].url);
        return;
      }
      location.set(entries[index].url);
      window.dispatchEvent({ type: 'popstate', state: entries[index].state });
    },
    back() { return history.go(-1); },
    forward() { return history.go(1); },
    entries,
    index: () => index,
  };
  return history;
}

// ---- one page: the shell scripts under test, with two pilot views

const ENTRY_TABLE = {
  views: {
    home: {
      fragment: '/app/home/workspace',
      scripts: [{ src: '/h/home.js' }],
      styles: [],
      reads: [{ route: '/app/home/', urls: ['/app/home/api/pulse'] }],
      contract: true,
    },
    transcripts: {
      fragment: '/app/transcripts/workspace',
      scripts: [{ src: '/t/transcripts.js' }],
      styles: ['/t/date-nav.css'],
      reads: [{ route: '/app/transcripts/{day}', urls: ['/app/transcripts/api/day/{day}'] }],
      openDay: { route: '/app/transcripts/', url: '/app/transcripts/api/open-day' },
      contract: true,
    },
    more: {
      fragment: '/app/more/workspace',
      scripts: [{ src: '/m/more.js' }],
      styles: [],
      reads: [],
      contract: true,
    },
    entities: {
      fragment: '/app/entities/workspace',
      scripts: [],
      styles: [],
      reads: [],
    },
    settings: {
      fragment: '/app/settings/workspace',
      scripts: [],
      styles: [],
      reads: [],
    },
    body: {
      fragment: '/app/body/workspace',
      scripts: [{ src: '/b/body.js' }],
      styles: ['/static/date-nav.css'],
      reads: [
        { route: '/app/body/', urls: ['/app/body/api/status'] },
        { route: '/app/body/{day}', urls: ['/app/body/api/day/{day}'] },
      ],
      contract: true,
    },
    // No open-day read. An undated address is today's, and the view says so.
    activities: {
      fragment: '/app/activities/workspace',
      scripts: [{ src: '/a/activities.js' }],
      styles: [],
      reads: [{ route: '/app/activities/{day}', urls: ['/app/activities/api/day/{day}/view'] }],
      contract: true,
    },
    // No open-day read. An undated address is the last day with a location,
    // and the view says so after it has read the index. A week, a month, the
    // place list and a place are their own pages of this view.
    map: {
      fragment: '/app/map/workspace',
      scripts: [{ src: '/a/map.js' }],
      styles: [],
      reads: [
        { route: '/app/map/week/{day}', urls: ['/app/map/api/week/{day}'] },
        { route: '/app/map/{day}', urls: ['/app/map/api/day/{day}'] },
        { route: '/app/map/places', urls: ['/app/map/api/places'] },
        { route: '/app/map/', urls: ['/app/map/api/index'] },
      ],
      contract: true,
    },
    // Its code is inline in its fragment, as the real one's is. Its state is
    // the address's query, so its own links keep the path.
    search: {
      fragment: '/app/search/workspace',
      scripts: [],
      styles: [],
      reads: [],
      contract: true,
    },
  },
};

// Search's script, run inline from its fragment. Held answers live at the
// script's top level, which runs once in the life of the page.
const SEARCH_CODE = `
    window.__searchRuns = (window.__searchRuns || 0) + 1;
    const held = new Map();
    window.SolViews.register('search', {
      mount(ctx) {
        const views = window.__views.search;
        views.mounts += 1;
        const path = window.location.pathname;
        let shown = null;
        const entry = () => (window.history.state && window.history.state.solRoute ? window.history.state.solRoute.id : null);
        const show = (restore) => {
          const search = window.location.search;
          shown = search;
          const q = new URLSearchParams(search).get('q') || '';
          const root = document.getElementById('search-root');
          if (!q) { if (root) root.textContent = 'empty'; ctx.ready('content'); return; }
          const id = entry();
          const kept = restore ? held.get(id) : null;
          if (kept && kept.search === search) {
            views.held += 1;
            views.draws.push(kept.answer);
            if (root) root.textContent = kept.answer;
            ctx.ready('content');
            return;
          }
          ctx.read('/app/search/api/search?q=' + q, { fresh: true }).then((answer) => {
            if (window.location.pathname !== path || shown !== search) return;
            held.set(id, { search, answer: answer.q });
            views.draws.push(answer.q);
            const now = document.getElementById('search-root');
            if (now) now.textContent = answer.q;
            ctx.ready('content');
          }, () => ctx.ready('error'));
        };
        ctx.on(document, 'click', (event) => {
          if (event.defaultPrevented || event.button !== 0) return;
          const link = event.target && event.target.closest ? event.target.closest('a[href]') : null;
          if (!link) return;
          const url = new URL(link.href, window.location.href);
          if (url.pathname !== path || window.location.pathname !== path || url.search === window.location.search) return;
          event.preventDefault();
          views.own.push(url.search);
          ctx.history.push(path + url.search);
          show(false);
        });
        ctx.on(window, 'popstate', () => {
          if (window.location.pathname !== path || window.location.search === shown) return;
          views.pops += 1;
          show(true);
        });
        show(true);
        return { dispose() { views.disposes += 1; } };
      }
    });`;

const FRAGMENTS = {
  '/app/home/workspace': '<div class="home"><h2>your day</h2><div id="home-root"></div></div><script src="/h/home.js"></script>',
  '/app/transcripts/workspace': '<link rel="stylesheet" href="/t/date-nav.css"><div class="tr"><h2 data-day-heading>day</h2><div id="tr-root"></div></div><script src="/t/transcripts.js"></script>',
  '/app/more/workspace': '<div class="more"><div id="more-root"></div><a href="/app/transcripts/">transcripts</a><a href="/app/home/">home</a><a href="/app/body/">body</a><a href="/app/activities/">activities</a><a href="/app/map/">map</a><a href="/app/settings/">settings</a></div><script src="/m/more.js"></script>',
  '/app/body/workspace': '<link rel="stylesheet" href="/static/date-nav.css"><div class="body-shell"><h1 class="body-title">body</h1><div id="body-root"></div><a class="body-day-cell" href="/app/body/20260102">day</a></div><script src="/b/body.js"></script>',
  '/app/activities/workspace': '<div class="activities"><h1>activities</h1><div id="activities-root"></div><a href="/app/activities/20260103">day</a></div><script src="/a/activities.js"></script>',
  '/app/map/workspace': '<div class="map"><h1>map</h1><div id="map-root"></div><a href="/app/map/20261003">day</a><a href="/app/map/week/20261005">week</a><a href="/app/map/month/202610">month</a><a href="/app/map/places">places</a><a href="/app/map/place/0123456789ab">place</a></div><script src="/a/map.js"></script>',
  '/app/search/workspace': '<section id="journal-search"><h1>search your journal</h1><div id="search-root"></div>'
    + '<a href="/app/search/?q=alpha">alpha</a><a href="/app/search/?q=alpha&offset=20">older</a>'
    + '<a href="/app/transcripts/20260102?stream=ja1r#090000_300">segment</a><a href="/app/map/20261003">place</a>'
    + '<a href="/app/home/week/20260102">week</a><a href="/app/entities/#ada">entity</a></section>'
    + '<script>' + SEARCH_CODE + '</script>',
  '/app/entities/workspace': '<div><h2>entities</h2></div>',
  '/app/settings/workspace': '<div><h2>settings</h2></div>',
};

// Each pilot view adds one of everything the contract takes back, and reads
// through the store. `views` counts mounts, updates and draws.
const VIEW_CODE = {
  '/h/home.js': `
    window.SolViews.register('home', {
      mount(ctx) {
        window.__views.home.mounts += 1;
        ctx.on(document, 'keydown', () => {});
        ctx.on(window, 'resize', () => {});
        ctx.setInterval(() => {}, 60000);
        ctx.setTimeout(() => {}, 30000);
        ctx.listen('supervisor', () => {});
        ctx.onConnectionState(() => {});
        ctx.observe(new window.FakeObserver());
        ctx.global('homeGlobal', () => {});
        ctx.bodyClass('home-open', true);
        if (window.__views.home.throwAfterAdding) throw new Error('home failed part-way');
        if (window.__views.home.drawHeading) {
          const heading = document.createElement('h2');
          heading.textContent = 'your week';
          document.querySelector('.home').appendChild(heading);
        }
        ctx.read('/app/home/api/pulse').then((pulse) => {
          window.__views.home.draws.push(pulse);
          document.getElementById('home-root').textContent = String(pulse.n);
          ctx.ready('content');
        }, () => ctx.ready('error'));
        return () => { window.__views.home.disposes += 1; };
      }
    });`,
  '/t/transcripts.js': `
    window.SolViews.register('transcripts', {
      mount(ctx) {
        const views = window.__views.transcripts;
        views.mounts += 1;
        let day = ctx.route.day;
        ctx.on(document, 'keydown', () => {});
        ctx.on(window, 'popstate', () => { views.pops += 1; });
        ctx.setInterval(() => {}, 60000);
        ctx.listen('change', () => {});
        const read = () => {
          const asked = day;
          ctx.read('/app/transcripts/api/day/' + asked).then((answer) => {
            if (asked !== day) return;
            views.draws.push(answer.day);
            document.querySelector('[data-day-heading]').textContent = answer.day;
            ctx.ready(answer.empty ? 'empty' : 'content');
          }, () => ctx.ready('error'));
        };
        window.__openSegment = (key) => ctx.history.push(window.location.pathname + '?stream=a#' + key + '/transcript');
        read();
        return {
          update(params) {
            views.updates.push(params.day);
            day = params.day;
            read();
          },
          dispose() { views.disposes += 1; }
        };
      }
    });`,
  '/m/more.js': `
    window.SolViews.register('more', {
      mount(ctx) {
        window.__views.more.mounts += 1;
        ctx.on(document, 'keydown', () => {});
        ctx.ready('content');
        return () => { window.__views.more.disposes += 1; };
      }
    });`,
  '/b/body.js': `
    window.SolViews.register('body', {
      mount(ctx) {
        const views = window.__views.body;
        views.mounts += 1;
        let day = ctx.route.day;
        const read = () => {
          const asked = day;
          const url = asked ? '/app/body/api/day/' + asked : '/app/body/api/status';
          ctx.read(url).then((answer) => {
            if (asked !== day) return;
            views.draws.push(asked || 'overview');
            const root = document.getElementById('body-root');
            if (root) root.textContent = asked || 'overview';
            ctx.ready('content');
          }, () => { if (asked === day) ctx.ready('error'); });
        };
        read();
        return {
          update(params) {
            views.updates.push(params.day);
            day = params.day;
            read();
          },
          dispose() { views.disposes += 1; }
        };
      }
    });`,
  '/a/activities.js': `
    window.SolViews.register('activities', {
      mount(ctx) {
        const views = window.__views.activities;
        views.mounts += 1;
        // The fixed day stands in for today, so the case does not depend on the clock.
        if (!ctx.route.day) {
          ctx.history.replace('/app/activities/20260102' + window.location.search + window.location.hash);
          ctx.route = window.SolViews.routeOf(window.location);
        }
        let day = ctx.route.day;
        let selected = window.location.hash.indexOf('#work/') === 0 ? 'standup' : null;
        ctx.on(window, 'popstate', () => {
          if (window.location.pathname === '/app/activities/' + day) views.pops += 1;
        });
        window.__activitySelected = () => selected;
        window.__openActivity = () => {
          selected = 'standup';
          ctx.history.push(window.location.pathname + window.location.search + '#work/standup');
        };
        // Registered at mount, after the router's click listener. A click the
        // router has taken is the owner leaving, so the sheet stays.
        ctx.on(document, 'click', (event) => {
          if (!selected || event.defaultPrevented) return;
          selected = null;
          ctx.history.push(window.location.pathname + window.location.search);
        });
        const read = () => {
          const asked = day;
          ctx.read('/app/activities/api/day/' + asked + '/view').then((answer) => {
            if (asked !== day) return;
            views.draws.push(answer.day);
            const root = document.getElementById('activities-root');
            if (root) root.textContent = answer.day;
            ctx.ready('content');
          }, () => { if (asked === day) ctx.ready('error'); });
        };
        read();
        return {
          update(params) {
            views.updates.push(params.day);
            day = params.day;
            read();
          },
          dispose() { views.disposes += 1; }
        };
      }
    });`,
  '/a/map.js': `
    window.SolViews.register('map', {
      async mount(ctx) {
        const views = window.__views.map;
        views.mounts += 1;
        ctx.onDispose(() => { views.cleanups += 1; });
        const parts = window.location.pathname.split('/').filter(Boolean);
        const digits = (value, n) => new RegExp('^[0-9]{' + n + '}$').test(value || '');
        const monthNumber = parts[3] ? Number(parts[3].slice(4, 6)) : 0;
        const page = (parts[2] === 'week' && digits(parts[3], 8))
          || (parts[2] === 'month' && digits(parts[3], 6) && monthNumber >= 1 && monthNumber <= 12)
          || (parts[2] === 'places' && parts.length === 3)
          || (parts[2] === 'place' && parts.length === 4);
        if (page) {
          views.pages.push(window.location.pathname);
          const root = document.getElementById('map-root');
          if (root) root.textContent = window.location.pathname;
          ctx.ready('content');
          return {
            update(params) {
              views.updates.push(params.day);
              const asked = params.day;
              ctx.read('/app/map/api/day/' + asked).then((answer) => {
                views.draws.push(answer.day);
                const shown = document.getElementById('map-root');
                if (shown) shown.textContent = answer.day;
                ctx.ready('content');
              }, () => ctx.ready('error'));
            },
            dispose() { views.disposes += 1; }
          };
        }
        let day = ctx.route.day;
        if (!day) {
          // The index answers after the owner may already have left. A day
          // written then would replace the page they opened.
          const index = await new Promise((resolve) => {
            let settled = false;
            const done = (value) => { if (!settled) { settled = true; resolve(value); } };
            if (ctx.signal.aborted) { done(null); return; }
            ctx.signal.addEventListener('abort', () => done(null), { once: true });
            ctx.read('/app/map/api/index').then(done, () => done(null));
          });
          const here = window.location.pathname;
          const end = index && index.coverage && index.coverage.end;
          if (!end || ctx.signal.aborted || (here !== '/app/map' && here !== '/app/map/')) {
            return { dispose() { views.disposes += 1; } };
          }
          ctx.history.replace('/app/map/' + end + window.location.search + window.location.hash);
          ctx.route = window.SolViews.routeOf(window.location);
          day = ctx.route.day;
        }
        const read = () => {
          const asked = day;
          ctx.read('/app/map/api/day/' + asked).then((answer) => {
            if (asked !== day) return;
            views.draws.push(answer.day);
            const root = document.getElementById('map-root');
            if (root) root.textContent = answer.day;
            ctx.ready('content');
          }, () => { if (asked === day) ctx.ready('error'); });
        };
        read();
        return {
          update(params) {
            views.updates.push(params.day);
            day = params.day;
            read();
          },
          dispose() { views.disposes += 1; }
        };
      }
    });`,
};

function headScript() {
  const page = fs.readFileSync(path.join(crateDir, 'assets/static/shell.html'), 'utf8');
  return page.match(/<script>\s*\/\/ The journal's front door[\s\S]*?<\/script>/)[0]
    .replace(/^<script>/, '').replace(/<\/script>$/, '')
    .replace('{/*entry-table*/}', JSON.stringify(ENTRY_TABLE));
}

function makePage({ pathname = '/app/home/', transitions = false } = {}) {
  const clock = makeClock();
  const server = makeServer();
  const record = { assigned: [], reloads: [], left: [], announced: [], skipped: 0, transitions: 0 };
  const storage = new Map();
  const writes = [];
  let current = new URL(pathname, ORIGIN);
  const location = {
    get origin() { return ORIGIN; },
    get pathname() { return current.pathname; },
    get search() { return current.search; },
    get hash() { return current.hash; },
    get href() { return current.href; },
    set href(value) { record.assigned.push(new URL(value, current.href).pathname); },
    set(url) { current = new URL(url, current.href); },
    assign(value) { record.assigned.push(new URL(value, current.href).pathname); window.dispatchEvent({ type: 'pageswap', viewTransition: viewTransitionStub(record) }); },
    reload() { record.reloads.push(current.pathname); window.dispatchEvent({ type: 'pageswap', viewTransition: viewTransitionStub(record) }); },
  };
  const window = new FakeEventTarget();
  const document = new FakeEventTarget();
  document.location = location;
  document.title = 'journal';
  document.visibilityState = 'visible';
  document.hidden = false;
  document.readyState = 'complete';
  document.activeElement = null;
  document.root = new FakeNode(document, 'html');
  document.head = document.root.appendChild(new FakeNode(document, 'head'));
  document.body = document.root.appendChild(new FakeNode(document, 'body'));
  document.inserted = (node) => {
    // An inline script runs when it goes into the page.
    if (node.tagName === 'SCRIPT' && !node.hasAttribute('src') && node.text.trim()) {
      try {
        vm.runInContext(node.text, window, { filename: 'inline' });
      } catch (error) {
        window.__scriptErrors.push(error);
      }
    }
    if (node.tagName === 'SCRIPT' && node.hasAttribute('src')) {
      const src = node.getAttribute('src');
      setImmediate(() => {
        try {
          vm.runInContext(VIEW_CODE[src] || '', window, { filename: src });
        } catch (error) {
          window.__scriptErrors.push(error);
        }
        node.dispatchEvent({ type: 'load' });
      });
    }
    if (node.tagName === 'LINK' && node.getAttribute('rel') === 'stylesheet') {
      setImmediate(() => node.dispatchEvent({ type: 'load' }));
    }
  };
  document.createElement = (tag) => {
    const node = new FakeNode(document, tag);
    if (tag === 'template') node._content = new FakeNode(document, null);
    return node;
  };
  // A template's markup is its content.
  const createElement = document.createElement;
  document.createElement = (tag) => {
    const node = createElement(tag);
    if (tag === 'template') {
      Object.defineProperty(node, 'innerHTML', {
        set(html) { node._content = new FakeNode(document, null); parseInto(node._content, String(html)); },
        get() { return ''; },
      });
    }
    return node;
  };
  document.createTextNode = (text) => { const node = new FakeNode(document, 'span'); node.text = text; return node; };
  document.getElementById = (id) => document.root.walk().find((node) => node.id === id) || null;
  document.querySelector = (selector) => document.root.querySelector(selector);
  document.querySelectorAll = (selector) => document.root.querySelectorAll(selector);
  document.body.innerHTML =
    '<nav id="app-rail"><a href="/app/home/" data-app-name="home">home</a><a href="/app/transcripts/" data-app-name="transcripts">transcripts</a>' +
    '<a href="/app/body/" data-app-name="body">body</a><a href="/app/activities/" data-app-name="activities">activities</a><a href="/app/map/" data-app-name="map">map</a><a href="/app/more/" data-app-name="more">more</a><a href="/app/entities/" data-app-name="entities">entities</a><a href="/app/sources/" data-app-name="sources">sources</a><a href="/app/settings/" data-app-name="settings">settings</a><a href="/app/search/" data-app-name="search">search</a>' +
    '<div id="status-instrument"></div></nav><nav id="app-dock"><a href="/app/body/" data-app-name="body">body</a><a href="/app/activities/" data-app-name="activities">activities</a><a href="/app/map/" data-app-name="map">map</a><a href="/app/more/" data-app-name="more">more</a><a href="/app/home/" data-app-name="home">home</a></nav>' +
    '<div id="view-announcer" role="status" aria-live="polite"></div>' +
    '<main id="main-content" tabindex="-1"><div class="surface-state surface-state--loading" role="status" aria-busy="true"></div></main>';
  if (transitions) {
    document.startViewTransition = (update) => {
      record.transitions += 1;
      const done = Promise.resolve().then(update);
      return { updateCallbackDone: done, ready: done, finished: done, skipTransition() {} };
    };
  }

  const history = makeHistory(window, location, record);
  history.pushState(null, '', pathname);
  // The event client: listeners and connection handlers, counted.
  const eventListeners = new Map();
  const connectionHandlers = new Set();
  let connected = false;
  let built = false;
  const appEvents = {
    buildChanged: () => built,
    listen(tract, fn) {
      const list = eventListeners.get(tract) || new Set();
      list.add(fn);
      eventListeners.set(tract, list);
      return () => list.delete(fn);
    },
    onConnectionState(fn) {
      connectionHandlers.add(fn);
      fn({ connected });
      return () => connectionHandlers.delete(fn);
    },
  };
  const observers = new Set();
  class FakeObserver {
    constructor() { observers.add(this); }
    observe() {}
    disconnect() { observers.delete(this); }
  }
  const sessionStorage = {
    getItem: (key) => (storage.has(key) ? storage.get(key) : null),
    setItem: (key, value) => { writes.push(key); storage.set(key, String(value)); },
    removeItem: (key) => storage.delete(key),
  };
  Object.assign(window, {
    window, document, location, history, appEvents, FakeObserver, sessionStorage,
    localStorage: { getItem: () => null, setItem: (key) => writes.push('local:' + key), removeItem() {} },
    fetch: server.fetch,
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    scrollY: 0,
    scrollTo(_x, y) { window.scrollY = y; },
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    performance: { mark() {} },
    console: { log() {}, debug() {}, info() {}, warn() {}, error: (...args) => window.__consoleErrors.push(args.map(String).join(' ')) },
    AbortController, AbortSignal, DOMException, Response, ReadableStream, TextDecoder, TextEncoder, URL, URLSearchParams,
    Promise, JSON, Map, Set, WeakMap, Error, TypeError, Math, Number, String, Object, Array, Date, structuredClone,
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    __views: {
      home: { mounts: 0, disposes: 0, draws: [], throwAfterAdding: false },
      transcripts: { mounts: 0, disposes: 0, updates: [], draws: [], pops: 0 },
      more: { mounts: 0, disposes: 0 },
      body: { mounts: 0, disposes: 0, updates: [], draws: [] },
      activities: { mounts: 0, disposes: 0, updates: [], draws: [], pops: 0 },
      map: { mounts: 0, disposes: 0, cleanups: 0, updates: [], draws: [], pages: [] },
      search: { mounts: 0, disposes: 0, draws: [], own: [], pops: 0, held: 0 },
    },
    __scriptErrors: [],
    __consoleErrors: [],
    solShellData: {
      apps: ['home', 'transcripts', 'more', 'body', 'activities', 'map', 'search', 'entities', 'sources', 'settings'].map((name) => ({
        name, label: name, workspace_url: `/app/${name}/workspace`,
      })),
    },
    SurfaceState: { error: () => '<div class="surface-state surface-state--error" role="alert"><button class="surface-state-retry">try again</button></div>' },
  });
  vm.createContext(window);
  vm.runInContext(headScript(), window, { filename: 'shell.html' });
  for (const name of ['api.js', 'store.js', 'views.js', 'mount-workspace.js', 'router.js']) {
    vm.runInContext(staticSource(name), window, { filename: name });
  }
  const view = (name) => window.SolViews.current()?.name === name;
  const page = {
    clock, server, window, document, history, record, writes, observers, location,
    eventCount: () => [...eventListeners.values()].reduce((sum, set) => sum + set.size, 0),
    connectionCount: () => connectionHandlers.size,
    setConnected(next) {
      connected = next;
      connectionHandlers.forEach((fn) => fn({ connected }));
    },
    setBuildChanged(next) { built = next; },
    view,
    main: () => document.getElementById('main-content'),
    link: (href) => document.root.walk().find((node) => node.tagName === 'A' && node.getAttribute('href') === href),
    // A click the router doesn't take is a page load, as in a browser.
    click(href) {
      const event = page.link(href).click();
      if (!event.defaultPrevented) record.assigned.push(new URL(href, ORIGIN).pathname);
      return event;
    },
    // Answers every view read and fragment as a plain success.
    answerAll({ pulse = { n: 1 } } = {}) {
      server.answers.set('/app/home/api/pulse', server.json(pulse));
      server.answers.set('/app/body/api/status', server.json({ coverage: { start: '20260102', end: '20260102' } }));
      for (const [url, body] of Object.entries(FRAGMENTS)) server.answers.set(url, server.html(body));
      server.answers.set('/api/shell', server.json({ app_names: [] }));
      server.answers.set('/api/streams/labels', server.json({}));
    },
    async boot(name = 'home') {
      const app = window.solShellData.apps.find((candidate) => candidate.name === name);
      try {
        await window.mountWorkspaceFragment(app.workspace_url, { appName: name });
      } finally {
        window.solRouter.start(name);
      }
      await flush();
    },
  };
  return page;
}

function viewTransitionStub(record) {
  return { skipTransition() { record.skipped += 1; } };
}

function dayAnswer(day, extra = {}) {
  return (call) => call.respond(200, JSON.stringify({ day, ...extra }), { 'content-type': 'application/json' });
}

function answerDays(page) {
  for (const day of ['20260101', '20260102', '20260103']) {
    page.server.answers.set(`/app/transcripts/api/day/${day}`, dayAnswer(day));
  }
}

function answerBody(page) {
  for (const day of ['20260102', '20260103']) {
    page.server.answers.set(`/app/body/api/day/${day}`, page.server.json({ day }));
  }
}

function answerActivities(page) {
  for (const day of ['20260102', '20260103']) {
    page.server.answers.set(`/app/activities/api/day/${day}/view`, page.server.json({ day }));
  }
}

function answerMap(page, end = '20261005') {
  page.server.answers.set('/app/map/api/index', page.server.json({
    coverage: { start: '20261001', end },
  }));
  for (const day of ['20261003', '20261005', end]) {
    page.server.answers.set(`/app/map/api/day/${day}`, page.server.json({ day }));
  }
  page.server.answers.set('/app/map/api/week/20261005', page.server.json({ day: '20261005' }));
  page.server.answers.set('/app/map/api/places', page.server.json({ places: [] }));
}

// What the page holds outside a view's own markup.
function footprint(page) {
  return {
    documentListeners: page.document.listenerCount(),
    windowListeners: page.window.listenerCount(),
    timers: page.clock.count(),
    events: page.eventCount(),
    connections: page.connectionCount(),
    observers: page.observers.size,
    bodyClass: page.document.body.getAttribute('class') || '',
    homeGlobal: typeof page.window.homeGlobal,
  };
}

let cases = 0;
const queue = [];
function asyncCase(name, fn) {
  queue.push(async () => {
    try {
      await fn();
      cases += 1;
    } catch (error) {
      error.message = name + ': ' + error.message;
      throw error;
    }
  });
}

// ---- the contract

asyncCase('contract: dispose leaves no listener, timer, subscription, observer, global or body class behind', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  const before = footprint(page);
  await page.boot('home');
  const mounted = footprint(page);
  assert.ok(mounted.documentListeners > before.documentListeners && mounted.timers > before.timers, 'home added to the page');
  assert.strictEqual(mounted.bodyClass, 'home-open');
  page.window.SolViews.dispose();
  // The shell's own short waits (the still-working watch, the date code's
  // idle fetch) run out first.
  await page.clock.advance(15000);
  const after = footprint(page);
  // The router listens on the document and window for the page's life; it
  // started with home and doesn't grow.
  const router = { documentListeners: 3, windowListeners: 1, connections: 1 };
  assert.deepStrictEqual(after, {
    ...before,
    documentListeners: before.documentListeners + router.documentListeners,
    windowListeners: before.windowListeners + router.windowListeners,
    connections: before.connections + router.connections,
  });
  assert.strictEqual(page.window.__views.home.disposes, 1, 'the view\'s own dispose ran');
});

asyncCase('contract: a mount that throws part-way takes back what it added, and try again mounts it cleanly', async () => {
  const page = makePage();
  page.answerAll();
  page.window.__views.home.throwAfterAdding = true;
  const before = footprint(page);
  await page.boot('home').catch(() => {});
  await page.clock.advance(15000);
  const failed = footprint(page);
  const router = { documentListeners: 3, windowListeners: 1, connections: 1 };
  assert.deepStrictEqual(failed, {
    ...before,
    documentListeners: before.documentListeners + router.documentListeners,
    windowListeners: before.windowListeners + router.windowListeners,
    connections: before.connections + router.connections,
  }, 'nothing the failed mount added is left');
  assert.ok(page.main().querySelector('.surface-state-retry'), 'the view shows its error state');
  page.window.__views.home.throwAfterAdding = false;
  page.main().querySelector('.surface-state-retry').click();
  await page.clock.advance(100);
  assert.deepStrictEqual(page.record.reloads, [], 'no whole page load');
  assert.strictEqual(page.window.__views.home.mounts, 2);
  assert.ok(page.view('home'));
  assert.strictEqual(footprint(page).bodyClass, 'home-open');
});

asyncCase('contract: a transcripts day step is an update, not a remount', async () => {
  const page = makePage({ pathname: '/app/transcripts/20260101' });
  page.answerAll();
  answerDays(page);
  await page.boot('transcripts');
  assert.deepStrictEqual(page.window.__views.transcripts.draws, ['20260101']);
  assert.strictEqual(page.window.solRouter.navigate('/app/transcripts/20260102'), true);
  await page.clock.advance(400);
  const views = page.window.__views.transcripts;
  assert.strictEqual(views.mounts, 1, 'mounted once');
  assert.deepStrictEqual(views.updates, ['20260102']);
  assert.deepStrictEqual(views.draws, ['20260101', '20260102']);
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260102');
  assert.strictEqual(page.server.of('/app/transcripts/workspace').length, 1, 'the fragment is not asked for again');
});

// ---- the router

asyncCase('router: a link between pilot views swaps <main> and keeps the rail, status mark and stream', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260102' }));
  await page.boot('home');
  const rail = page.document.getElementById('app-rail');
  const status = page.document.getElementById('status-instrument');
  const events = page.window.appEvents;
  const event = page.click('/app/transcripts/');
  assert.ok(event.defaultPrevented, 'the router took the click');
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260102', 'the undated root takes its day');
  assert.ok(page.main().querySelector('.tr'), 'the new view is in <main>');
  assert.ok(!page.main().querySelector('.home'), 'the old one is gone');
  assert.strictEqual(page.document.getElementById('app-rail'), rail, 'the rail stays');
  assert.strictEqual(page.document.getElementById('status-instrument'), status, 'the status mark stays');
  assert.strictEqual(page.window.appEvents, events, 'the live stream stays');
  assert.strictEqual(page.connectionCount(), 1, 'the page keeps its own connection watch; home took its own with it');
  assert.deepStrictEqual(page.record.assigned, [], 'no page load');
  assert.deepStrictEqual(page.record.reloads, []);
});

asyncCase('router: to or from another view is a page load, sources included', async () => {
  const page = makePage();
  page.answerAll();
  await page.boot('home');
  for (const href of ['/app/entities/', '/app/sources/']) {
    const event = page.click(href);
    assert.ok(!event.defaultPrevented, `${href} is the browser's`);
  }
  assert.deepStrictEqual(page.record.assigned, ['/app/entities/', '/app/sources/']);
  assert.ok(page.view('home'), 'home stays mounted');

  const other = makePage({ pathname: '/app/entities/' });
  other.answerAll();
  await other.boot('entities');
  assert.strictEqual(other.window.solRouter.isActive(), false, 'a page opened on another view never turns the router on');
  const event = other.click('/app/home/');
  assert.ok(!event.defaultPrevented);
  assert.deepStrictEqual(other.record.assigned, ['/app/home/']);
});

asyncCase('router: home, more and transcripts stay in the page, and settings does not', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  const rail = page.document.getElementById('app-rail');
  const status = page.document.getElementById('status-instrument');
  page.window.scrollY = 240;
  page.click('/app/more/');
  await page.clock.advance(400);
  assert.ok(page.view('more'));
  assert.strictEqual(page.window.__views.more.mounts, 1);
  assert.strictEqual(page.document.getElementById('app-rail'), rail, 'the rail stays');
  assert.strictEqual(page.document.getElementById('status-instrument'), status, 'the status mark stays');
  assert.strictEqual(page.window.scrollY, 0, 'a new view starts at the top');
  assert.strictEqual(page.document.activeElement, page.main(), 'more draws its own heading, so focus is on the workspace');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'more');
  assert.deepStrictEqual(page.record.assigned, []);
  assert.deepStrictEqual(page.record.reloads, []);

  // more's own card, not the rail.
  page.main().querySelector('a[href="/app/transcripts/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260101');
  assert.strictEqual(page.window.__views.more.disposes, 1);
  page.window.scrollY = 80;

  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('more'));
  assert.strictEqual(page.window.scrollY, 0, 'more is where it was left');
  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.strictEqual(page.window.scrollY, 240, 'home is where it was left');
  await page.history.forward();
  await page.clock.advance(400);
  assert.ok(page.view('more'));
  await page.history.forward();
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.window.scrollY, 80);

  // The dock, still in this page.
  page.document.getElementById('app-dock').querySelector('a[href="/app/more/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('more'));
  page.document.getElementById('app-dock').querySelector('a[href="/app/home/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.deepStrictEqual(page.record.assigned, []);

  page.click('/app/settings/');
  assert.ok(!page.view('settings'));
  assert.deepStrictEqual(page.record.assigned, ['/app/settings/']);

  const settings = makePage({ pathname: '/app/settings/' });
  settings.answerAll();
  await settings.boot('settings');
  assert.strictEqual(settings.window.solRouter.isActive(), false, 'a page opened on settings never turns the router on');
  const leaving = settings.click('/app/more/');
  assert.ok(!leaving.defaultPrevented);
  assert.deepStrictEqual(settings.record.assigned, ['/app/more/']);
});

asyncCase('router: leaving the page does not abort a view read', async () => {
  const page = makePage();
  page.answerAll();
  await page.boot('home');
  const hold = page.window.apiJson('/app/home/api/hold');
  let settled = false;
  hold.then(() => { settled = true; }, () => { settled = true; });
  await flush();
  const call = page.server.pending('/app/home/api/hold')[0];
  assert.ok(call, 'the read is in flight');
  page.window.dispatchEvent({ type: 'pagehide', persisted: true });
  await flush();
  assert.strictEqual(call.aborted, false, 'pagehide leaves a view read in flight');
  assert.strictEqual(settled, false);
  page.window.dispatchEvent({ type: 'pageshow', persisted: true });
  await flush();
  assert.strictEqual(call.aborted, false, 'a restored page still has its read');
  call.respond(200, JSON.stringify({ ok: true }), { 'content-type': 'application/json' });
  await flush();
  assert.strictEqual(settled, true, 'the read can still answer after the page returns');

  const again = makePage();
  again.answerAll();
  await again.boot('home');
  const reading = again.window.apiJson('/app/home/api/hold');
  let readSettled = false;
  reading.then(() => { readSettled = true; }, () => { readSettled = true; });
  await flush();
  const open = again.server.pending('/app/home/api/hold')[0];
  again.click('/app/settings/');
  await flush();
  assert.strictEqual(open.aborted, false, 'a full-load click leaves the read in flight');
  assert.strictEqual(readSettled, false);
  assert.deepStrictEqual(again.record.assigned, ['/app/settings/']);

  const stayed = makePage();
  stayed.answerAll();
  await stayed.boot('home');
  const same = stayed.window.apiJson('/app/home/api/hold');
  same.then(() => {}, () => {});
  await flush();
  const kept = stayed.server.pending('/app/home/api/hold')[0];
  const link = stayed.document.createElement('a');
  link.setAttribute('href', '/app/home/#section');
  stayed.document.body.appendChild(link);
  link.click();
  await flush();
  assert.strictEqual(kept.aborted, false, 'a same-path link keeps the view\'s reads');
  assert.deepStrictEqual(stayed.record.assigned, []);
});

asyncCase('router: a day step on another dated view stays a page load', async () => {
  const page = makePage({ pathname: '/app/entities/' });
  page.answerAll();
  await page.boot('entities');
  assert.strictEqual(page.window.solRouter.navigate('/app/entities/20260102'), false);
  const pilot = makePage();
  pilot.answerAll();
  await pilot.boot('home');
  assert.strictEqual(pilot.window.solRouter.navigate('/app/entities/20260102'), false, 'leaving a pilot view for another view loads the page');
});

asyncCase('router: body stays in the page with home, transcripts and more, and a day change is an update', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  answerBody(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260102' }));
  await page.boot('home');
  const rail = page.document.getElementById('app-rail');
  page.window.scrollY = 180;

  page.click('/app/body/');
  await page.clock.advance(400);
  assert.ok(page.view('body'));
  assert.strictEqual(page.location.pathname, '/app/body/');
  assert.strictEqual(page.document.getElementById('app-rail'), rail, 'the rail stays');
  assert.strictEqual(page.window.scrollY, 0, 'a new view starts at the top');
  assert.strictEqual(page.document.activeElement, page.main().querySelector('h1'), 'focus is on the heading body ships');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'body');
  assert.strictEqual(page.window.__views.body.mounts, 1);
  assert.deepStrictEqual(page.window.__views.body.draws, ['overview']);
  assert.deepStrictEqual(page.record.assigned, []);
  assert.strictEqual(page.server.of('/app/body/workspace').length, 1, 'the fragment is fetched once');

  // A heatmap cell is a link in the overview. The day replaces it in place.
  page.click('/app/body/20260102');
  await page.clock.advance(400);
  const views = page.window.__views.body;
  assert.strictEqual(views.mounts, 1, 'overview to a day is not a remount');
  assert.deepStrictEqual(views.updates, ['20260102']);
  assert.deepStrictEqual(views.draws, ['overview', '20260102']);
  assert.strictEqual(page.location.pathname, '/app/body/20260102');
  assert.strictEqual(page.server.of('/app/body/workspace').length, 1, 'a day change does not fetch the fragment');

  // A day step keeps focus where it was and says nothing new.
  const step = page.link('/app/home/');
  step.focus();
  page.window.solRouter.navigate('/app/body/20260103');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 1);
  assert.deepStrictEqual(views.updates, ['20260102', '20260103']);
  assert.deepStrictEqual(views.draws, ['overview', '20260102', '20260103']);
  assert.strictEqual(page.document.activeElement, step, 'a day step does not move focus');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'body');

  // Back to the overview remounts, and stays in the page.
  page.click('/app/body/');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 2, 'a day back to the overview mounts again');
  assert.ok(page.view('body'));
  assert.strictEqual(page.location.pathname, '/app/body/');
  assert.deepStrictEqual(page.record.assigned, []);

  page.click('/app/transcripts/');
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260102');
  page.click('/app/body/');
  await page.clock.advance(400);
  assert.ok(page.view('body'));
  assert.strictEqual(page.location.pathname, '/app/body/');

  page.click('/app/more/');
  await page.clock.advance(400);
  assert.ok(page.view('more'));
  page.main().querySelector('a[href="/app/body/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('body'), 'more\'s card opens body in the page');
  assert.deepStrictEqual(page.record.assigned, []);

  page.document.getElementById('app-dock').querySelector('a[href="/app/home/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  page.document.getElementById('app-dock').querySelector('a[href="/app/body/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('body'), 'the dock opens body in the page');
  assert.deepStrictEqual(page.record.assigned, []);

  page.click('/app/settings/');
  assert.ok(!page.view('settings'));
  assert.deepStrictEqual(page.record.assigned, ['/app/settings/'], 'settings is still a full load');
  assert.strictEqual(page.window.solRouter.navigate('/app/entities/'), false, 'entities is still a full load');
});

asyncCase('router: back and forward restore body and where the page was', async () => {
  const page = makePage();
  page.answerAll();
  answerBody(page);
  await page.boot('home');
  page.window.scrollY = 220;
  page.click('/app/body/');
  await page.clock.advance(400);
  assert.ok(page.view('body'));
  assert.strictEqual(page.window.scrollY, 0);
  page.window.scrollY = 90;
  page.click('/app/body/20260102');
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/body/20260102');
  page.window.scrollY = 40;
  await page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/body/');
  assert.ok(page.view('body'));
  assert.strictEqual(page.window.scrollY, 90, 'the overview is where it was left');
  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.strictEqual(page.window.scrollY, 220, 'home is where it was left');
  await page.history.forward();
  await page.clock.advance(400);
  assert.ok(page.view('body'));
  assert.strictEqual(page.location.pathname, '/app/body/');
  assert.strictEqual(page.window.scrollY, 90);
  await page.history.forward();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/body/20260102');
  assert.strictEqual(page.window.scrollY, 40);
  assert.deepStrictEqual(page.record.assigned, []);
});

asyncCase('router: activities stays in the page with home, transcripts, more and body, and a day change is an update', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  answerBody(page);
  answerActivities(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260102' }));
  await page.boot('home');
  const rail = page.document.getElementById('app-rail');
  page.window.scrollY = 180;

  page.click('/app/activities/');
  await page.clock.advance(400);
  assert.ok(page.view('activities'));
  assert.strictEqual(page.location.pathname, '/app/activities/20260102', 'an undated address becomes the fixed day');
  assert.strictEqual(page.document.getElementById('app-rail'), rail, 'the rail stays');
  assert.strictEqual(page.window.scrollY, 0, 'a new view starts at the top');
  assert.strictEqual(page.document.activeElement, page.main().querySelector('h1'), 'focus is on the heading activities ships');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'activities');
  assert.strictEqual(page.window.__views.activities.mounts, 1);
  assert.deepStrictEqual(page.window.__views.activities.draws, ['20260102']);
  assert.deepStrictEqual(page.record.assigned, []);
  assert.strictEqual(page.server.of('/app/activities/workspace').length, 1, 'the fragment is fetched once');

  // The day link is the other day. The undated address already became 20260102.
  page.click('/app/activities/20260103');
  await page.clock.advance(400);
  const views = page.window.__views.activities;
  assert.strictEqual(views.mounts, 1, 'a day change is not a remount');
  assert.deepStrictEqual(views.updates, ['20260103']);
  assert.deepStrictEqual(views.draws, ['20260102', '20260103']);
  assert.strictEqual(page.location.pathname, '/app/activities/20260103');
  assert.strictEqual(page.server.of('/app/activities/workspace').length, 1, 'a day change does not fetch the fragment');

  const step = page.link('/app/home/');
  step.focus();
  page.window.solRouter.navigate('/app/activities/20260102');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 1);
  assert.deepStrictEqual(views.updates, ['20260103', '20260102']);
  assert.deepStrictEqual(views.draws, ['20260102', '20260103', '20260102']);
  assert.strictEqual(page.document.activeElement, step, 'a day step does not move focus');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'activities');

  page.click('/app/activities/');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 2, 'the undated address mounts again');
  assert.ok(page.view('activities'));
  assert.strictEqual(page.location.pathname, '/app/activities/20260102');
  assert.deepStrictEqual(page.record.assigned, []);

  page.click('/app/transcripts/');
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260102');
  page.click('/app/activities/');
  await page.clock.advance(400);
  assert.ok(page.view('activities'));

  page.click('/app/more/');
  await page.clock.advance(400);
  assert.ok(page.view('more'));
  page.main().querySelector('a[href="/app/activities/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('activities'), 'more\'s card opens activities in the page');

  page.click('/app/home/');
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  page.click('/app/activities/');
  await page.clock.advance(400);
  assert.ok(page.view('activities'));

  page.click('/app/body/');
  await page.clock.advance(400);
  assert.ok(page.view('body'));
  page.click('/app/activities/');
  await page.clock.advance(400);
  assert.ok(page.view('activities'), 'body opens activities in the page');

  page.document.getElementById('app-dock').querySelector('a[href="/app/home/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  page.document.getElementById('app-dock').querySelector('a[href="/app/activities/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('activities'), 'the dock opens activities in the page');
  assert.deepStrictEqual(page.record.assigned, []);

  page.click('/app/settings/');
  assert.ok(!page.view('settings'));
  assert.deepStrictEqual(page.record.assigned, ['/app/settings/'], 'settings is still a full load');
  assert.strictEqual(page.window.solRouter.navigate('/app/entities/'), false, 'entities is still a full load');
});

asyncCase('router: back and forward restore activities and where the page was', async () => {
  const page = makePage();
  page.answerAll();
  answerActivities(page);
  await page.boot('home');
  page.window.scrollY = 220;
  page.click('/app/activities/');
  await page.clock.advance(400);
  assert.ok(page.view('activities'));
  assert.strictEqual(page.location.pathname, '/app/activities/20260102');
  assert.strictEqual(page.window.scrollY, 0);
  page.window.scrollY = 90;
  page.click('/app/activities/20260103');
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/activities/20260103');
  page.window.scrollY = 40;
  await page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/activities/20260102');
  assert.ok(page.view('activities'));
  assert.strictEqual(page.window.scrollY, 90, 'the day is where it was left');
  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.strictEqual(page.window.scrollY, 220, 'home is where it was left');
  await page.history.forward();
  await page.clock.advance(400);
  assert.ok(page.view('activities'));
  assert.strictEqual(page.location.pathname, '/app/activities/20260102');
  assert.strictEqual(page.window.scrollY, 90);
  await page.history.forward();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/activities/20260103');
  assert.strictEqual(page.window.scrollY, 40);
  assert.deepStrictEqual(page.record.assigned, []);
});

asyncCase('router: an undated activities arrival becomes that day, and an own entry stays the view\'s', async () => {
  const page = makePage();
  page.answerAll();
  answerActivities(page);
  await page.boot('home');
  page.click('/app/activities/');
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/activities/20260102');
  assert.strictEqual(page.window.solRouter.held().mountedPath, '/app/activities/20260102');
  const views = page.window.__views.activities;
  assert.strictEqual(views.mounts, 1);
  page.window.__openActivity();
  assert.strictEqual(page.location.hash, '#work/standup');
  assert.strictEqual(page.location.pathname, '/app/activities/20260102');
  await page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 1, 'back from the view\'s own entry is not a new mount');
  assert.strictEqual(views.pops, 1, 'the view got its own entry');
  assert.strictEqual(page.location.pathname, '/app/activities/20260102');
  assert.strictEqual(page.location.hash, '');
  assert.ok(page.view('activities'));
  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  await page.history.forward();
  await page.clock.advance(400);
  assert.ok(page.view('activities'));
  assert.strictEqual(page.location.pathname, '/app/activities/20260102', 'forward lands on the dated address');
});

asyncCase('router: a view cannot write another view\'s address', async () => {
  const page = makePage();
  page.answerAll();
  answerActivities(page);
  await page.boot('home');
  page.click('/app/activities/');
  await page.clock.advance(400);
  const held = page.window.solRouter.held();
  assert.strictEqual(held.mountedPath, '/app/activities/20260102');
  const href = page.location.href;
  const length = page.history.length;
  page.window.solRouter.viewEntry('replace', '/app/home/');
  page.window.solRouter.viewEntry('push', '/app/transcripts/20260102');
  assert.strictEqual(page.window.solRouter.held().mountedPath, held.mountedPath);
  assert.strictEqual(page.window.solRouter.held().entryId, held.entryId);
  assert.strictEqual(page.location.href, href, 'a foreign address is not written');
  assert.strictEqual(page.history.length, length, 'a foreign push adds no entry');
  page.window.solRouter.viewEntry('replace', '/app/activities/20260103');
  assert.strictEqual(page.location.pathname, '/app/activities/20260103', 'the same view still replaces its own address');
  assert.strictEqual(page.window.solRouter.held().mountedPath, '/app/activities/20260103');
  assert.strictEqual(page.history.length, length, 'a replace does not add an entry');
});

asyncCase('router: a dock link with an activity open adds one entry, and back returns to it', async () => {
  const page = makePage();
  page.answerAll();
  answerActivities(page);
  await page.boot('home');
  page.click('/app/activities/');
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/activities/20260102');
  page.window.__openActivity();
  assert.strictEqual(page.location.hash, '#work/standup');
  assert.strictEqual(page.window.__activitySelected(), 'standup');
  const length = page.history.length;
  const event = page.document.getElementById('app-dock').querySelector('a[href="/app/more/"]').click();
  assert.ok(event.defaultPrevented, 'the router took the dock link');
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/more/');
  assert.strictEqual(page.history.length, length + 1, 'the dock tap adds the one entry the router wrote');
  await page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/activities/20260102');
  assert.strictEqual(page.location.hash, '#work/standup');
  assert.ok(page.view('activities'));
  assert.strictEqual(page.window.__activitySelected(), 'standup', 'back lands on the open activity');
});

asyncCase('router: map stays in the page with home, transcripts, more, body and activities, and a day change is an update', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  answerBody(page);
  answerActivities(page);
  answerMap(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260102' }));
  await page.boot('home');
  const rail = page.document.getElementById('app-rail');
  page.window.scrollY = 180;

  page.click('/app/map/');
  await page.clock.advance(400);
  assert.ok(page.view('map'));
  assert.strictEqual(page.location.pathname, '/app/map/20261005', 'an undated address becomes the index day');
  assert.strictEqual(page.document.getElementById('app-rail'), rail, 'the rail stays');
  assert.strictEqual(page.window.scrollY, 0, 'a new view starts at the top');
  assert.strictEqual(page.document.activeElement, page.main().querySelector('h1'), 'focus is on the heading the map ships');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'map');
  assert.strictEqual(page.window.__views.map.mounts, 1);
  assert.deepStrictEqual(page.window.__views.map.draws, ['20261005']);
  assert.deepStrictEqual(page.record.assigned, []);
  assert.strictEqual(page.server.of('/app/map/workspace').length, 1, 'the fragment is fetched once');

  page.click('/app/map/20261003');
  await page.clock.advance(400);
  const views = page.window.__views.map;
  assert.strictEqual(views.mounts, 1, 'a day change is not a remount');
  assert.deepStrictEqual(views.updates, ['20261003']);
  assert.deepStrictEqual(views.draws, ['20261005', '20261003']);
  assert.strictEqual(page.location.pathname, '/app/map/20261003');
  assert.strictEqual(page.server.of('/app/map/workspace').length, 1, 'a day change does not fetch the fragment');

  const step = page.link('/app/home/');
  step.focus();
  page.window.solRouter.navigate('/app/map/20261005');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 1);
  assert.deepStrictEqual(views.updates, ['20261003', '20261005']);
  assert.strictEqual(page.document.activeElement, step, 'a day step does not move focus');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'map');

  page.click('/app/map/');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 2, 'the undated address mounts again');
  assert.ok(page.view('map'));
  assert.strictEqual(page.location.pathname, '/app/map/20261005');
  assert.deepStrictEqual(page.record.assigned, []);

  for (const href of ['/app/transcripts/', '/app/more/', '/app/home/', '/app/body/', '/app/activities/']) {
    page.click(href);
    await page.clock.advance(400);
    assert.ok(!page.view('map'), href + ' opens in the page');
    assert.deepStrictEqual(page.record.assigned, []);
    page.click('/app/map/');
    await page.clock.advance(400);
    assert.ok(page.view('map'), 'the map opens from ' + href + ' in the page');
    assert.strictEqual(page.location.pathname, '/app/map/20261005');
    assert.deepStrictEqual(page.record.assigned, []);
  }

  page.click('/app/more/');
  await page.clock.advance(400);
  assert.ok(page.view('more'));
  page.main().querySelector('a[href="/app/map/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('map'), 'more\'s card opens the map in the page');

  page.document.getElementById('app-dock').querySelector('a[href="/app/home/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  page.document.getElementById('app-dock').querySelector('a[href="/app/map/"]').click();
  await page.clock.advance(400);
  assert.ok(page.view('map'), 'the dock opens the map in the page');
  assert.strictEqual(page.server.of('/app/map/workspace').length, 1, 'later visits keep the fragment');
  assert.deepStrictEqual(page.record.assigned, []);

  page.click('/app/settings/');
  assert.ok(!page.view('settings'));
  assert.deepStrictEqual(page.record.assigned, ['/app/settings/'], 'settings is still a full load');
  assert.strictEqual(page.window.solRouter.navigate('/app/entities/'), false, 'entities is still a full load');
});

asyncCase('router: back and forward restore the map and where the page was', async () => {
  const page = makePage();
  page.answerAll();
  answerMap(page);
  await page.boot('home');
  page.window.scrollY = 220;
  page.click('/app/map/');
  await page.clock.advance(400);
  assert.ok(page.view('map'));
  assert.strictEqual(page.location.pathname, '/app/map/20261005');
  assert.strictEqual(page.window.scrollY, 0);
  page.window.scrollY = 90;
  page.click('/app/map/20261003');
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/map/20261003');
  page.window.scrollY = 40;
  await page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/map/20261005');
  assert.ok(page.view('map'));
  assert.strictEqual(page.window.scrollY, 90, 'the day is where it was left');
  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.strictEqual(page.window.scrollY, 220, 'home is where it was left');
  await page.history.forward();
  await page.clock.advance(400);
  assert.ok(page.view('map'));
  assert.strictEqual(page.location.pathname, '/app/map/20261005');
  assert.strictEqual(page.window.scrollY, 90);
  await page.history.forward();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/map/20261003');
  assert.strictEqual(page.window.scrollY, 40);
  assert.deepStrictEqual(page.record.assigned, []);
});

asyncCase('router: an undated map arrival is the index day, and the map\'s other pages stay in the page', async () => {
  const page = makePage();
  page.answerAll();
  answerMap(page, '20261008');
  await page.boot('home');
  const before = page.history.length;
  page.click('/app/map/');
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/map/20261008', 'the day is the one the index names');
  assert.strictEqual(page.history.length, before + 1, 'finding the day replaces the address');
  assert.strictEqual(page.window.solRouter.held().mountedPath, '/app/map/20261008');
  const views = page.window.__views.map;
  assert.strictEqual(views.mounts, 1);
  assert.deepStrictEqual(views.draws, ['20261008']);
  assert.strictEqual(page.server.of('/app/map/workspace').length, 1);

  page.click('/app/map/week/20261005');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 2, 'a week is another page of the map');
  assert.deepStrictEqual(views.updates, []);
  assert.strictEqual(page.location.pathname, '/app/map/week/20261005');
  assert.strictEqual(page.main().querySelector('#map-root').textContent, '/app/map/week/20261005');
  assert.strictEqual(page.server.of('/app/map/workspace').length, 1, 'the fragment stays');
  assert.deepStrictEqual(page.record.assigned, []);

  page.click('/app/map/20261003');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 2, 'a day from a week is an update');
  assert.deepStrictEqual(views.updates, ['20261003']);
  assert.strictEqual(page.location.pathname, '/app/map/20261003');
  assert.strictEqual(page.main().querySelector('#map-root').textContent, '20261003');

  page.click('/app/map/month/202610');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 3);
  assert.strictEqual(page.location.pathname, '/app/map/month/202610');

  page.click('/app/map/places');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 4);
  assert.strictEqual(page.location.pathname, '/app/map/places');

  page.click('/app/map/place/0123456789ab');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 5);
  assert.strictEqual(page.location.pathname, '/app/map/place/0123456789ab');

  page.click('/app/map/');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, 6, 'the undated address mounts again');
  assert.strictEqual(page.location.pathname, '/app/map/20261008');
  assert.deepStrictEqual(page.record.assigned, []);
});

function answerSearch(page) {
  page.server.answers.set('/app/search/api/search?q=alpha', page.server.json({ q: 'alpha' }));
  page.server.answers.set('/app/home/api/week/20260102', page.server.json({}));
}

asyncCase('router: search stays in the page with every contract view, and its own links are its entries', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  answerBody(page);
  answerActivities(page);
  answerMap(page);
  answerSearch(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260102' }));
  await page.boot('home');
  const views = page.window.__views.search;

  for (const name of ['home', 'transcripts', 'more', 'body', 'activities', 'map']) {
    page.click('/app/search/');
    await page.clock.advance(400);
    assert.ok(page.view('search'), `to search from ${name === 'home' ? 'home' : 'the last view'}`);
    assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'search');
    assert.strictEqual(page.document.activeElement, page.main().querySelector('h1'), 'focus is on the heading search ships');
    page.document.getElementById('app-rail').querySelector(`a[data-app-name="${name}"]`).click();
    await page.clock.advance(400);
    assert.ok(page.view(name), `search to ${name}`);
  }
  assert.deepStrictEqual(page.record.assigned, [], 'no page loads');
  assert.strictEqual(page.window.__searchRuns, 1, 'the inline script ran once');

  page.click('/app/search/');
  await page.clock.advance(400);
  const length = page.history.length;
  const mounts = views.mounts;
  const event = page.click('/app/search/?q=alpha');
  await page.clock.advance(400);
  assert.strictEqual(event.defaultPrevented, true, 'search takes its own link');
  assert.strictEqual(page.history.length, length + 1, 'one entry');
  assert.strictEqual(views.mounts, mounts, 'not a remount');
  assert.deepStrictEqual(views.own, ['?q=alpha']);
  assert.strictEqual(page.location.search, '?q=alpha');
  assert.deepStrictEqual(views.draws, ['alpha']);

  page.click('/app/search/?q=alpha&offset=20');
  assert.strictEqual(page.history.length, length + 2, 'a page of results is one entry');
  await page.clock.advance(400);
  page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(views.pops, 1, 'back between two searches is the view\'s');
  assert.strictEqual(views.mounts, mounts);

  // The rail's search on a results page: the view's empty search, not a move.
  page.click('/app/search/');
  await page.clock.advance(400);
  assert.strictEqual(page.location.href, ORIGIN + '/app/search/');
  assert.strictEqual(views.mounts, mounts, 'the empty search is the view\'s entry');
  assert.strictEqual(page.document.getElementById('search-root').textContent, 'empty');
  // The same address again is a fresh mount, as for every view.
  page.click('/app/search/');
  await page.clock.advance(400);
  assert.strictEqual(views.mounts, mounts + 1);

  page.click('/app/search/?q=alpha');
  await page.clock.advance(400);
  page.click('/app/map/20261003');
  await page.clock.advance(400);
  assert.ok(page.view('map'), 'a location result opens the map day in the page');
  page.history.back();
  await page.clock.advance(400);
  page.click('/app/home/week/20260102');
  await page.clock.advance(400);
  assert.ok(page.view('home'), 'a reflection result opens its week in the page');
  page.history.back();
  await page.clock.advance(400);
  assert.deepStrictEqual(page.record.assigned, []);
  page.click('/app/entities/#ada');
  assert.deepStrictEqual(page.record.assigned, ['/app/entities/'], 'entities is off the contract: a page load');
  assert.deepStrictEqual(page.window.__scriptErrors, []);
});

asyncCase('router: a result\'s transcripts segment, then back, draws the answer search held, with its scroll', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  answerSearch(page);
  await page.boot('home');
  const views = page.window.__views.search;
  page.click('/app/search/');
  await page.clock.advance(400);
  page.click('/app/search/?q=alpha');
  await page.clock.advance(400);
  assert.strictEqual(page.server.of('/app/search/api/search?q=alpha').length, 1);
  page.window.scrollY = 300;

  page.click('/app/transcripts/20260102?stream=ja1r#090000_300');
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.location.pathname + page.location.search + page.location.hash, '/app/transcripts/20260102?stream=ja1r#090000_300',
    'the segment address reaches transcripts whole');
  assert.deepStrictEqual(page.record.assigned, []);

  page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('search'));
  assert.strictEqual(views.mounts, 2, 'search is mounted again');
  assert.strictEqual(page.window.__searchRuns, 1, 'from the script that ran once, so its held answers are still there');
  assert.strictEqual(views.held, 1, 'the held answer is drawn');
  assert.strictEqual(page.server.of('/app/search/api/search?q=alpha').length, 1, 'no search read on back');
  assert.strictEqual(page.document.getElementById('search-root').textContent, 'alpha');
  assert.strictEqual(page.window.scrollY, 300, 'and the scroll comes back');

  // Forward again, then a forward arrival at the same address: a new entry, which reads.
  page.history.forward();
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  page.window.solRouter.navigate('/app/search/?q=alpha');
  await page.clock.advance(400);
  assert.strictEqual(page.server.of('/app/search/api/search?q=alpha').length, 2, 'a forward arrival reads');
  assert.deepStrictEqual(page.window.__scriptErrors, []);
});

asyncCase('router: a mount still waiting publishes nothing, is released once, and does not write over the page opened', async () => {
  const page = makePage();
  page.answerAll();
  answerMap(page);
  let release = null;
  page.server.answers.set('/app/map/api/index', (call) => {
    release = () => page.server.json({ coverage: { start: '20261001', end: '20261005' } })(call);
  });
  await page.boot('home');
  const published = [];
  page.document.addEventListener('workspace:mounted', (event) => {
    published.push(event.detail && event.detail.appName);
  });
  page.click('/app/map/');
  // The swap waits out the index, then the view is mounted and still waiting on it.
  await page.clock.advance(400);
  assert.ok(release, 'the index read started');
  assert.strictEqual(page.location.pathname, '/app/map/');
  assert.strictEqual(page.window.__views.map.mounts, 1);
  page.click('/app/home/');
  release();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.strictEqual(page.location.pathname, '/app/home/', 'the index day is not written over home');
  assert.deepStrictEqual(page.record.assigned, []);
  assert.deepStrictEqual(published, ['home'], 'a map the owner already left is not announced');
  const left = page.window.__views.map;
  assert.strictEqual(left.cleanups, 1, 'the mount still waiting was released once');
  assert.strictEqual(left.disposes, 0, 'its dispose does not run again after the owner has left');
});

asyncCase('router: back and forward between pilot entries restore view and scroll', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  await page.boot('home');
  page.window.scrollY = 300;
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.window.scrollY, 0, 'a new view starts at the top');
  page.window.scrollY = 120;
  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.strictEqual(page.window.scrollY, 300, 'home is where it was left');
  await page.history.forward();
  await page.clock.advance(400);
  assert.ok(page.view('transcripts'));
  assert.strictEqual(page.window.scrollY, 120);
  assert.deepStrictEqual(page.record.assigned, []);
});

asyncCase('router: back across a segment entry and a day update lands each step, then home', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  const views = page.window.__views.transcripts;
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260101');
  page.window.solRouter.navigate('/app/transcripts/20260102');
  await page.clock.advance(400);
  page.window.__openSegment('120000_300');
  assert.strictEqual(page.location.search + page.location.hash, '?stream=a#120000_300/transcript');
  // back: the segment entry is the view's own; transcripts restores it.
  await page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260102');
  assert.strictEqual(views.pops, 1, 'the view got its own entry');
  assert.strictEqual(views.mounts, 1, 'no remount for a segment entry');
  // back: day A, another path, the router's: an update.
  await page.history.back();
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260101');
  assert.deepStrictEqual(views.updates, ['20260102', '20260101']);
  assert.strictEqual(views.mounts, 1);
  // back: home.
  await page.history.back();
  await page.clock.advance(400);
  assert.ok(page.view('home'), 'back from transcripts reaches home');
  assert.strictEqual(page.window.__views.home.mounts, 2);
  // back once more leaves the page for the entry before it.
  await page.history.back();
  assert.deepStrictEqual(page.record.left, ['http://elsewhere.test/'], 'the browser takes the entry before the page');
  assert.deepStrictEqual(page.record.assigned, []);
});

asyncCase('router: a page that learned a new build loads the destination as a new page', async () => {
  const page = makePage();
  page.answerAll();
  await page.boot('home');
  page.setBuildChanged(true);
  const event = page.click('/app/transcripts/');
  assert.ok(event.defaultPrevented);
  assert.deepStrictEqual(page.record.assigned, ['/app/transcripts/']);
  assert.ok(page.view('home'), 'no new code mounted into this page');
  assert.ok(page.writes.includes('solstone.serverBuildReloadAt'));
  // A view this page's code doesn't know is a page load too.
  const unknown = makePage();
  unknown.answerAll();
  await unknown.boot('home');
  unknown.window.solShellData.apps = unknown.window.solShellData.apps.filter((app) => app.name !== 'transcripts');
  unknown.click('/app/transcripts/');
  assert.deepStrictEqual(unknown.record.assigned, ['/app/transcripts/']);
});

asyncCase('router: a fragment naming code from another build loads the whole page, never runs it here', async () => {
  const page = makePage();
  page.answerAll();
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  page.server.answers.set('/app/transcripts/workspace', page.server.html('<div class="tr"><h2>day</h2></div><script src="/app/transcripts/static/0123456789abcdef/transcripts.js"></script>'));
  await page.boot('home');
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  assert.deepStrictEqual(page.record.reloads, ['/app/transcripts/20260101'], 'one whole page load of the destination');
  assert.strictEqual(page.window.__views.transcripts.mounts, 0, 'the other build\'s code never ran');
  assert.ok(page.writes.includes('solstone.serverBuildReloadAt'));
});

asyncCase('router: a quick second move ends the first one\'s reads, and the second view adopts its own', async () => {
  const page = makePage();
  page.answerAll();
  page.server.answers.delete('/app/home/api/pulse');
  const firstPulse = () => page.server.of('/app/home/api/pulse');
  // The first pulse answers at once; later ones wait for the test.
  page.server.answers.set('/app/home/api/pulse', (call) => { if (firstPulse().length === 1) page.server.json({ n: 1 })(call); });
  await page.boot('home');
  page.click('/app/transcripts/');
  await flush();
  const dayRead = () => page.server.of('/app/transcripts/api/day/20260101');
  page.click('/app/home/');
  await flush();
  const transcriptsEarly = page.server.calls.filter((call) => call.url.startsWith('/app/transcripts/api/'));
  assert.ok(transcriptsEarly.length > 0, 'the first move started its reads');
  assert.ok(transcriptsEarly.every((call) => call.aborted), 'the first move\'s reads were aborted');
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  assert.strictEqual(page.window.__views.transcripts.mounts, 0, 'transcripts never mounted');
  const pulses = page.server.pending('/app/home/api/pulse');
  assert.strictEqual(pulses.length, 1, 'one pulse read for the second move');
  pulses[0].respond(200, JSON.stringify({ n: 2 }), { 'content-type': 'application/json' });
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.deepStrictEqual(page.window.__views.home.draws, [{ n: 1 }, { n: 2 }]);
  assert.strictEqual(page.server.of('/app/home/api/pulse').length, 2, 'home adopted the read its move started');
  assert.strictEqual(dayRead().length <= 1, true);
});

// ---- the store

asyncCase('store: one read in flight per key; a refresh asks of its own', async () => {
  const page = makePage({ pathname: '/app/entities/' });
  const abort = page.window.SolStore.createAbort();
  const scope = page.window.SolStore.scope(abort.signal);
  page.window.apiJson = (url, opts) => page.window.fetch(url, opts).then((r) => r.json());
  const a = scope.read('/app/x/api/state');
  const b = scope.read('/app/x/api/state');
  await flush();
  assert.strictEqual(a, b);
  assert.strictEqual(page.server.of('/app/x/api/state').length, 1);
  scope.read('/app/x/api/state', { fresh: true });
  await flush();
  assert.strictEqual(page.server.of('/app/x/api/state').length, 2);
});

asyncCase('store: an ended mount\'s reads are aborted and never answer', async () => {
  const page = makePage({ pathname: '/app/entities/' });
  const abort = page.window.SolStore.createAbort();
  const scope = page.window.SolStore.scope(abort.signal);
  page.window.apiJson = (url, opts) => page.window.fetch(url, opts).then((r) => r.json(), (e) => { throw e; });
  let answered = false;
  scope.read('/app/x/api/state').then(() => { answered = true; }, () => { answered = true; });
  await flush();
  abort.abort();
  page.server.of('/app/x/api/state')[0].respond(200, '{}', { 'content-type': 'application/json' });
  await flush();
  assert.strictEqual(page.server.of('/app/x/api/state')[0].aborted, true);
  assert.strictEqual(answered, false, 'never drawn, never an error');
  scope.read('/app/x/api/state');
  await flush();
  assert.strictEqual(page.server.of('/app/x/api/state').length, 1, 'nothing more is asked after the end');
});

asyncCase('store: two reads cut off by an ended mount are different promises, and the store exports no shared one', async () => {
  const page = makePage({ pathname: '/app/entities/' });
  const abort = page.window.SolStore.createAbort();
  const scope = page.window.SolStore.scope(abort.signal);
  abort.abort();
  const first = scope.read('/app/x/api/one');
  const second = scope.read('/app/x/api/two');
  let settled = 0;
  first.then(() => { settled += 1; }, () => { settled += 1; });
  second.then(() => { settled += 1; }, () => { settled += 1; });
  await flush();
  assert.notStrictEqual(first, second);
  assert.strictEqual(settled, 0, 'neither answers');
  assert.strictEqual(page.server.of('/app/x/api/one').length, 0, 'nothing is asked');
  assert.strictEqual(page.server.of('/app/x/api/two').length, 0, 'nothing is asked');
  const shared = Object.values(page.window.SolStore).filter((value) => value && typeof value.then === 'function');
  assert.deepStrictEqual(shared, []);
  assert.strictEqual(page.window.SolStore.NEVER, undefined);
  assert.strictEqual(typeof page.window.SolStore.never, 'function');
  const made = page.window.SolStore.never();
  assert.notStrictEqual(made, page.window.SolStore.never());
  let madeSettled = false;
  made.then(() => { madeSettled = true; }, () => { madeSettled = true; });
  await flush();
  assert.strictEqual(madeSettled, false);
});

asyncCase('store: moving between views writes nothing to web storage', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  for (let i = 0; i < 3; i += 1) {
    page.click('/app/transcripts/');
    await page.clock.advance(400);
    page.window.solRouter.navigate('/app/transcripts/20260102');
    await page.clock.advance(400);
    page.click('/app/home/');
    await page.clock.advance(400);
  }
  assert.deepStrictEqual(page.writes, []);
});

asyncCase('store: a view whose first read failed reads again on the stream\'s connect, once per connect', async () => {
  const page = makePage({ pathname: '/app/transcripts/20260101' });
  page.answerAll();
  let down = true;
  page.server.answers.set('/app/transcripts/api/day/20260101', (call) => (down ? call.respond(500, '{}', { 'content-type': 'application/json' }) : dayAnswer('20260101')(call)));
  await page.boot('transcripts');
  await page.clock.advance(5000);
  assert.strictEqual(page.window.SolViews.current().state, 'error');
  const reads = () => page.server.of('/app/transcripts/api/day/20260101').length;
  const first = reads();
  page.setConnected(true);
  await page.clock.advance(5000);
  assert.strictEqual(reads(), first + 1, 'one read on the connect');
  assert.strictEqual(page.window.SolViews.current().state, 'error', 'still down');
  page.setConnected(true);
  await page.clock.advance(5000);
  assert.strictEqual(reads(), first + 1, 'the same connect never reads twice');
  down = false;
  page.setConnected(false);
  page.setConnected(true);
  await page.clock.advance(5000);
  assert.strictEqual(reads(), first + 2, 'the next connect reads again');
  assert.strictEqual(page.window.SolViews.current().state, 'content');
  assert.deepStrictEqual(page.window.__views.transcripts.draws, ['20260101']);
  // After a day step whose read fails, the next connect still reads again.
  page.server.answers.set('/app/transcripts/api/day/20260102', (call) => call.respond(500, '{}', { 'content-type': 'application/json' }));
  page.window.solRouter.navigate('/app/transcripts/20260102');
  await page.clock.advance(5000);
  assert.strictEqual(page.window.SolViews.current().state, 'error');
  const stepped = page.server.of('/app/transcripts/api/day/20260102').length;
  page.server.answers.set('/app/transcripts/api/day/20260102', dayAnswer('20260102'));
  page.setConnected(false);
  page.setConnected(true);
  await page.clock.advance(5000);
  assert.ok(page.server.of('/app/transcripts/api/day/20260102').length > stepped, 'a failed day step reads again on the connect');
});

// ---- what a screen reader hears

asyncCase('accessibility: focus moves to the new view\'s heading and the view is announced by name', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  const heading = page.main().querySelector('h2');
  assert.strictEqual(page.document.activeElement, heading, 'focus is on the heading');
  assert.strictEqual(heading.getAttribute('tabindex'), '-1');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'transcripts');
  // A day step keeps focus where it was.
  const step = page.link('/app/home/');
  step.focus();
  page.window.solRouter.navigate('/app/transcripts/20260102');
  await page.clock.advance(400);
  assert.strictEqual(page.document.activeElement, step);
});

asyncCase('accessibility: a view with no heading drawn yet takes focus on the workspace itself', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  page.server.answers.set('/app/home/workspace', page.server.html('<div class="home"><div id="home-root"></div></div><script src="/h/home.js"></script>'));
  await page.boot('transcripts');
  page.main().removeAttribute('tabindex');
  page.click('/app/home/');
  await page.clock.advance(400);
  assert.strictEqual(page.document.activeElement, page.main(), 'focus is on the workspace');
  assert.strictEqual(page.main().getAttribute('tabindex'), '-1', 'the workspace can take focus');
  assert.strictEqual(page.document.getElementById('view-announcer').textContent, 'home');
});

asyncCase('accessibility: a heading the view draws itself is passed over for the workspace, so a redraw keeps focus', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  page.server.answers.set('/app/home/workspace', page.server.html('<div class="home"><div id="home-root"></div></div><script src="/h/home.js"></script>'));
  page.window.__views.home.drawHeading = true;
  await page.boot('transcripts');
  page.click('/app/home/');
  await page.clock.advance(400);
  assert.ok(page.main().querySelector('h2'), 'the view drew a heading');
  assert.strictEqual(page.document.activeElement, page.main(), 'focus is on the workspace');
  // Home draws its section again once a read comes in.
  page.main().querySelector('.home').innerHTML = '<div id="home-root"></div><h2>your week</h2>';
  assert.strictEqual(page.document.activeElement, page.main(), 'focus stays on the workspace, not the page');
});

asyncCase('accessibility: a view still loading at ten seconds is announced as still working, once', async () => {
  const page = makePage();
  page.answerAll();
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  const announcer = page.document.getElementById('view-announcer');
  page.click('/app/transcripts/');
  // The swap waits 300 ms for the read, and the name follows 300 ms later.
  await page.clock.advance(1000);
  assert.strictEqual(announcer.textContent, 'transcripts');
  await page.clock.advance(8500);
  assert.strictEqual(announcer.textContent, 'transcripts');
  await page.clock.advance(1000);
  assert.strictEqual(announcer.textContent, 'still working…');
  announcer.textContent = '';
  await page.clock.advance(20000);
  assert.strictEqual(announcer.textContent, '', 'said once');
  // A day step still loading at ten seconds says so too.
  page.server.pending('/app/transcripts/api/day/20260101').forEach((call) => dayAnswer('20260101')(call));
  await page.clock.advance(1000);
  announcer.textContent = '';
  page.window.solRouter.navigate('/app/transcripts/20260102');
  await page.clock.advance(9500);
  assert.strictEqual(announcer.textContent, '', 'nothing said for a day step before ten seconds');
  await page.clock.advance(1000);
  assert.strictEqual(announcer.textContent, 'still working\u2026', 'a slow day step is announced as still working');
  page.server.pending('/app/transcripts/api/day/20260102').forEach((call) => dayAnswer('20260102')(call));
  // A view ready before ten seconds is never announced as still working.
  page.server.pending('/app/transcripts/api/day/20260101').forEach((call) => dayAnswer('20260101')(call));
  page.click('/app/home/');
  await page.clock.advance(15000);
  assert.strictEqual(announcer.textContent, 'home');
});

// ---- transitions and prefetch

asyncCase('transition: the swap waits for the next view\'s reads, at most 300 ms', async () => {
  const page = makePage({ transitions: true });
  page.answerAll();
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  page.click('/app/transcripts/');
  await page.clock.advance(200);
  assert.ok(page.main().querySelector('.home'), 'the old view stays while the read is out');
  await page.clock.advance(150);
  assert.ok(page.main().querySelector('.tr'), 'swapped at 300 ms with the read still out');
  assert.strictEqual(page.record.transitions, 1);
  // A read that answers sooner swaps sooner.
  answerDays(page);
  page.click('/app/home/');
  await page.clock.advance(50);
  assert.ok(page.main().querySelector('.home'));
});

asyncCase('prefetch: pressing or hovering on a rail entry fetches its code, never its data', async () => {
  const page = makePage();
  page.answerAll();
  page.server.answers.set('/t/transcripts.js', (call) => call.respond(200, ''));
  page.server.answers.set('/t/date-nav.css', (call) => call.respond(200, ''));
  await page.boot('home');
  const before = page.server.calls.length;
  const rail = page.link('/app/transcripts/');
  page.document.dispatchEvent({ type: 'pointerover', target: rail });
  await page.clock.advance(50);
  assert.strictEqual(page.server.calls.length, before, 'a pass over the rail asks for nothing');
  await page.clock.advance(100);
  const asked = page.server.calls.slice(before).map((call) => call.url);
  assert.deepStrictEqual(asked.sort(), ['/app/transcripts/workspace', '/t/date-nav.css', '/t/transcripts.js']);
  page.document.dispatchEvent({ type: 'pointerdown', target: rail });
  await page.clock.advance(150);
  assert.strictEqual(page.server.calls.length, before + 3, 'once per view');
  page.document.dispatchEvent({ type: 'pointerdown', target: page.link('/app/entities/') });
  await page.clock.advance(150);
  assert.strictEqual(page.server.calls.length, before + 3, 'another view\'s code is its own page\'s');
  assert.ok(!page.server.calls.some((call) => call.url.includes('/api/') && call.url !== '/api/shell' && call.url !== '/api/streams/labels' && call.url !== '/app/home/api/pulse'));
  assert.ok(page.window.solRouter.held().fragments <= 2, 'one fragment per pilot view at most');
});

// ---- a first arrival, a superseded move and what the store keeps

// Every mount, with the address the page had when it ran.
function recordMounts(page) {
  const seen = [];
  const views = page.window.SolViews;
  const mount = views.mount;
  views.mount = function (name, options) {
    seen.push([name, page.location.pathname]);
    return mount.call(this, name, options);
  };
  return seen;
}

// Each mount ran at an address of its own view.
function assertOwnAddresses(seen) {
  for (const [name, at] of seen) {
    assert.ok(at === `/app/${name}/` || at.startsWith(`/app/${name}/`), `${name} mounted at ${at}`);
  }
}

// A view transition that runs its callback in a later frame, as a browser
// does. `frame()` runs the ones waiting; skipping one runs it at once.
function heldTransitions(page) {
  const waiting = [];
  page.document.startViewTransition = (update) => {
    page.record.transitions += 1;
    let go;
    const gate = new Promise((resolve) => { go = resolve; });
    const done = gate.then(update);
    waiting.push(go);
    return { updateCallbackDone: done, ready: done, finished: done, skipTransition() { page.record.skipped += 1; go(); } };
  };
  return {
    waiting: () => waiting.length,
    frame() { waiting.splice(0).forEach((go) => go()); },
  };
}

// A fragment that waits for the test, answered by `release()`.
function heldFragment(page, url) {
  const calls = [];
  page.server.answers.set(url, (call) => { calls.push(call); });
  return {
    calls,
    release(index = calls.length - 1) { page.server.html(FRAGMENTS[url])(calls[index]); },
    fail(index = calls.length - 1, status = 500) { calls[index].respond(status, 'no'); },
  };
}

asyncCase('superseded: with view transitions, a move placed after a newer one changes nothing', async () => {
  const page = makePage({ transitions: true });
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  const frames = heldTransitions(page);
  const seen = recordMounts(page);
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  assert.strictEqual(frames.waiting(), 1, 'the swap is checked and waits for its frame');
  assert.ok(page.view('home'));
  // The newer move skips the waiting transition, which runs the old swap.
  page.click('/app/body/');
  await page.clock.advance(400);
  frames.frame();
  await page.clock.advance(400);
  assert.ok(page.view('body'));
  assert.strictEqual(page.location.pathname, '/app/body/');
  assert.strictEqual(page.window.__views.transcripts.mounts, 0, 'the superseded view never mounts');
  assert.strictEqual(page.window.__views.home.disposes, 1, 'home goes once, for the newer view');
  assert.ok(page.record.skipped >= 1);
  assert.deepStrictEqual(seen, [['body', '/app/body/']]);
  assert.ok(page.main().querySelector('.body-shell') && !page.main().querySelector('.tr'));
});

asyncCase('superseded: without a transition, a place whose move ended disposes, inserts and mounts nothing', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  await page.boot('home');
  const seen = recordMounts(page);
  const controller = new page.window.AbortController();
  await page.window.mountWorkspaceFragment('/app/transcripts/workspace', {
    appName: 'transcripts',
    signal: controller.signal,
    swap: async (place) => {
      controller.abort();
      return place();
    },
  });
  await page.clock.advance(400);
  assert.ok(page.view('home'));
  assert.ok(page.main().querySelector('.home'), 'the markup on the page stays');
  assert.strictEqual(page.window.__views.home.disposes, 0);
  assert.deepStrictEqual(seen, []);
});

asyncCase('superseded: a day step made while transcripts reads its open day keeps its own address', async () => {
  // A newer move that mounts nothing (a day step taken in place) leaves the
  // older move's open-day read running; its answer must not write over the
  // step's address.
  const page = makePage();
  page.answerAll();
  answerDays(page);
  const openDays = [];
  page.server.answers.set('/app/transcripts/api/open-day', (call) => {
    openDays.push(call);
    if (openDays.length === 1) dayAnswer('20260101')(call);
  });
  await page.boot('home');
  page.click('/app/transcripts/');
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260101');
  const seen = recordMounts(page);
  page.click('/app/transcripts/');
  await page.clock.advance(50);
  assert.strictEqual(openDays.length, 2, 'the undated address reads its day again');
  page.window.solRouter.navigate('/app/transcripts/20260103');
  await page.clock.advance(50);
  if (!openDays[1].settled) dayAnswer('20260102')(openDays[1]);
  await page.clock.advance(400);
  assert.strictEqual(page.location.pathname, '/app/transcripts/20260103', 'the late day is not written over the step');
  assert.deepStrictEqual(Array.from(page.window.__views.transcripts.updates).slice(-1), ['20260103']);
  assert.ok(!page.history.entries.some((entry) => entry.url.includes('/app/transcripts/20260102')), 'no entry took the late day');
  assert.deepStrictEqual(seen, []);
});

asyncCase('superseded: a back press while transcripts reads its day is a move, and the leaving view writes no entry', async () => {
  const page = makePage({ transitions: true });
  page.answerAll();
  answerDays(page);
  answerSearch(page);
  let openDay = null;
  page.server.answers.set('/app/transcripts/api/open-day', (call) => { openDay = call; });
  await page.boot('home');
  const frames = heldTransitions(page);
  page.click('/app/search/');
  await page.clock.advance(400);
  frames.frame();
  await page.clock.advance(400);
  assert.ok(page.view('search'));
  const seen = recordMounts(page);
  page.click('/app/transcripts/');
  await page.clock.advance(50);
  // The leaving view's own entry is refused: the address is the move's.
  page.window.solRouter.viewEntry('push', '/app/search/?q=alpha');
  assert.strictEqual(page.location.pathname, '/app/transcripts/');
  await page.history.back();
  await page.clock.advance(50);
  assert.strictEqual(page.location.pathname, '/app/search/');
  if (!openDay.settled) dayAnswer('20260101')(openDay);
  await page.clock.advance(400);
  frames.frame();
  await page.clock.advance(400);
  assert.ok(page.view('search'), 'back lands on search');
  assert.strictEqual(page.location.pathname, '/app/search/');
  assert.strictEqual(page.window.__views.transcripts.mounts, 0);
  assert.ok(!page.history.entries.some((entry) => entry.url.includes('/app/transcripts/2026')));
  assert.deepStrictEqual(seen, [['search', '/app/search/']]);
});

asyncCase('superseded: across quick moves, back presses and open-day reads, every mount runs at its own address', async () => {
  const page = makePage({ transitions: true });
  page.answerAll();
  answerDays(page);
  answerBody(page);
  answerSearch(page);
  const openDays = [];
  page.server.answers.set('/app/transcripts/api/open-day', (call) => { openDays.push(call); });
  await page.boot('home');
  const frames = heldTransitions(page);
  const seen = recordMounts(page);
  const answerOpenDays = () => openDays.filter((call) => !call.settled).forEach((call) => dayAnswer('20260101')(call));
  const steps = [
    () => page.click('/app/transcripts/'),
    () => page.click('/app/body/'),
    () => frames.frame(),
    () => page.click('/app/search/'),
    () => page.click('/app/transcripts/'),
    () => page.history.back(),
    answerOpenDays,
    () => frames.frame(),
    () => page.click('/app/more/'),
    () => page.click('/app/transcripts/'),
    answerOpenDays,
    () => page.history.back(),
    () => frames.frame(),
    () => page.history.forward(),
    answerOpenDays,
    () => frames.frame(),
  ];
  for (const step of steps) {
    await step();
    await page.clock.advance(120);
  }
  for (let i = 0; i < 4; i += 1) {
    answerOpenDays();
    frames.frame();
    await page.clock.advance(400);
  }
  assert.ok(seen.length >= 3, 'views mounted along the way');
  assertOwnAddresses(seen);
  const now = page.window.SolViews.current();
  assert.ok(now && page.location.pathname.startsWith(`/app/${now.name}/`), 'the view on the page is the address\'s');
  assert.deepStrictEqual(page.record.assigned, []);
});

asyncCase('failed move: an open-day read that fails loads the page, and a failed fragment leaves the new view\'s error state', async () => {
  // The day can't be read: the leaving view no longer owns the address, so
  // the page loads it.
  {
    const page = makePage();
    page.answerAll();
    answerSearch(page);
    page.server.answers.set('/app/transcripts/api/open-day', (call) => call.respond(500, 'no'));
    await page.boot('home');
    page.click('/app/search/');
    await page.clock.advance(400);
    page.click('/app/transcripts/');
    await page.clock.advance(15000);
    assert.deepStrictEqual(page.record.assigned, ['/app/transcripts/'], 'the address is loaded as a page');
    assert.strictEqual(page.window.__views.transcripts.mounts, 0);
  }
  // The fragment can't be read: the error state is transcripts', at its
  // address, with nothing of search still working behind it, and a reconnect
  // reads transcripts again.
  {
    const page = makePage();
    page.answerAll();
    answerDays(page);
    answerSearch(page);
    page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
    const fragment = heldFragment(page, '/app/transcripts/workspace');
    await page.boot('home');
    page.click('/app/search/');
    await page.clock.advance(400);
    const before = page.window.__views.search.disposes;
    page.click('/app/transcripts/');
    await page.clock.advance(50);
    // The early read and its ordinary retry both fail.
    for (let i = 0; i < 3 && !page.main().querySelector('.surface-state-retry'); i += 1) {
      fragment.calls.filter((call) => !call.settled).forEach((call) => call.respond(500, 'no'));
      await page.clock.advance(4000);
    }
    assert.strictEqual(page.location.pathname, '/app/transcripts/20260101');
    assert.ok(page.main().querySelector('.surface-state-retry'), 'the error state, with try again');
    assert.strictEqual(page.window.SolViews.current(), null, 'search is no longer mounted');
    assert.strictEqual(page.window.__views.search.disposes, before + 1);
    page.setConnected(false);
    page.setConnected(true);
    await page.clock.advance(50);
    fragment.release();
    await page.clock.advance(400);
    assert.ok(page.view('transcripts'), 'the reconnect read transcripts again');
    assert.deepStrictEqual(page.record.assigned, []);
  }
});

asyncCase('store: the last failure per key is plain fields, never an error object', async () => {
  const page = makePage();
  page.answerAll();
  page.server.answers.set('/app/home/api/broken', (call) => call.respond(500, '{"error":"no"}', { 'content-type': 'application/json' }));
  const controller = page.window.SolStore.createAbort();
  const reads = page.window.SolStore.scope(controller.signal);
  await reads.read('/app/home/api/broken').catch(() => {});
  const cut = page.window.SolStore.createAbort();
  const cutReads = page.window.SolStore.scope(cut.signal);
  cutReads.read('/app/home/api/never');
  await flush();
  cut.abort();
  await flush();
  for (const url of ['/app/home/api/broken', '/app/home/api/never']) {
    const entry = page.window.SolStore.last(url);
    assert.ok(entry && entry.error, url + ' failed');
    assert.deepStrictEqual(Object.keys(entry.error).sort(), ['message', 'name', 'status'], url);
    for (const value of Object.values(entry.error)) {
      assert.ok(value === null || ['string', 'number'].includes(typeof value), url + ': plain fields only');
    }
  }
  assert.strictEqual(page.window.SolStore.last('/app/home/api/broken').error.status, 500);
  assert.strictEqual(page.window.SolStore.last('/app/home/api/never').error.name, 'AbortError');
});

asyncCase('leaks: a hundred switches and fifty day steps leave the page as the first switch did', async () => {
  const page = makePage();
  page.answerAll();
  answerDays(page);
  page.server.answers.set('/app/transcripts/api/open-day', page.server.json({ day: '20260101' }));
  await page.boot('home');
  const cycle = async () => {
    page.click('/app/transcripts/');
    await page.clock.advance(400);
    page.window.solRouter.navigate('/app/transcripts/20260102');
    await page.clock.advance(400);
    page.click('/app/home/');
    await page.clock.advance(400);
  };
  await cycle();
  await page.clock.advance(15000);
  const first = { ...footprint(page), router: page.window.solRouter.held().fragments, head: page.document.head.children.length };
  for (let i = 0; i < 49; i += 1) await cycle();
  await page.clock.advance(15000);
  const last = { ...footprint(page), router: page.window.solRouter.held().fragments, head: page.document.head.children.length };
  assert.deepStrictEqual(last, first);
  assert.ok(page.window.solRouter.held().scrolls <= 50, 'the scroll memory is bounded');
  assert.deepStrictEqual(page.window.__scriptErrors, []);
});

(async () => {
  for (const run of queue) await run();
  console.log('DOM CASES: ' + cases + ' passed');
})().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
