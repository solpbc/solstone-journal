// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The dashboard's reads, mounts and live stream under a link that stalls,
// fails or goes silent: read bounds, retries, dropped answers, the mount's
// retry, and the stream's watchdog, all on a simulated clock.

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
  for (let i = 0; i < 25; i += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

function makeClock() {
  let now = 0;
  let sequence = 0;
  const timers = new Map();
  const add = (fn, ms, repeat) => {
    sequence += 1;
    const delay = Math.max(0, Number(ms) || 0);
    timers.set(sequence, { at: now + delay, fn, every: repeat ? Math.max(1, delay) : 0 });
    return sequence;
  };
  return {
    now: () => now,
    count: () => timers.size,
    setTimeout: (fn, ms) => add(fn, ms, false),
    setInterval: (fn, ms) => add(fn, ms, true),
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
        timer.fn();
      }
      now = end;
      await flush();
    },
  };
}

function abortError() {
  return new DOMException('The operation was aborted.', 'AbortError');
}

// A body the test feeds by hand: `push` sends bytes, `end` finishes it.
function controlledBody() {
  let controller;
  const stream = new ReadableStream({ start(c) { controller = c; } });
  const encoder = new TextEncoder();
  return {
    stream,
    push(text) { controller.enqueue(encoder.encode(text)); },
    end() { controller.close(); },
    fail(error) { try { controller.error(error); } catch (_) {} },
  };
}

// Every call is recorded and waits for the test to answer it.
function makeServer() {
  const calls = [];
  function fetch(input, init = {}) {
    const url = new URL(String(input), ORIGIN + '/app/home/');
    return new Promise((resolve, reject) => {
      const call = {
        url: url.pathname + url.search,
        method: String(init.method || 'GET').toUpperCase(),
        init,
        aborted: false,
        settled: false,
        body: null,
        respond(status, text = '', headers = {}) {
          if (call.settled) return;
          call.settled = true;
          resolve(new Response(text, { status, headers }));
        },
        respondStreaming(status = 200, headers = {}) {
          if (call.settled) return null;
          call.settled = true;
          call.body = controlledBody();
          resolve(new Response(call.body.stream, { status, headers }));
          return call.body;
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
          if (call.body) call.body.fail(abortError());
          if (!call.settled) {
            call.settled = true;
            reject(abortError());
          }
        };
        if (signal.aborted) onAbort();
        else signal.addEventListener('abort', onAbort, { once: true });
      }
      calls.push(call);
    });
  }
  return { fetch, calls, last: () => calls[calls.length - 1] };
}

class FakeEventTarget {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, fn) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push(fn);
  }
  removeEventListener(type, fn) {
    const list = this.listeners.get(type) || [];
    this.listeners.set(type, list.filter((candidate) => candidate !== fn));
  }
  dispatchEvent(event) {
    (this.listeners.get(event.type) || []).slice().forEach((fn) => fn(event));
    return true;
  }
  listenerCount(type) { return (this.listeners.get(type) || []).length; }
}

// ---- a small document: enough for the mount's fragment, scripts and sheets

class FakeElement extends FakeEventTarget {
  constructor(document, tagName) {
    super();
    this.ownerDocument = document;
    this.tagName = tagName.toUpperCase();
    this.attributeMap = new Map();
    this.children = [];
    this.parent = null;
    this.text = '';
    this._html = '';
  }
  get attributes() {
    return Array.from(this.attributeMap, ([name, value]) => ({ name, value }));
  }
  setAttribute(name, value) { this.attributeMap.set(name, String(value)); }
  getAttribute(name) { return this.attributeMap.has(name) ? this.attributeMap.get(name) : null; }
  hasAttribute(name) { return this.attributeMap.has(name); }
  get src() { return this.hasAttribute('src') ? new URL(this.getAttribute('src'), ORIGIN).href : ''; }
  get textContent() { return this.text; }
  set textContent(value) { this.text = String(value); }
  get className() { return this.getAttribute('class') || ''; }
  appendChild(child) {
    child.parent = this;
    this.children.push(child);
    this.ownerDocument.inserted(child);
    return child;
  }
  remove() {
    if (!this.parent) return;
    this.parent.children = this.parent.children.filter((child) => child !== this);
    this.parent = null;
  }
  replaceWith(other) {
    const parent = this.parent;
    const index = parent.children.indexOf(this);
    parent.children[index] = other;
    other.parent = parent;
    this.parent = null;
    this.ownerDocument.inserted(other);
  }
  get innerHTML() { return this._html; }
  set innerHTML(html) {
    this._html = String(html);
    this.children = [];
    const tags = /<(script|link|button)\b([^>]*)>([\s\S]*?<\/script>)?/gi;
    let match;
    while ((match = tags.exec(this._html))) {
      const element = new FakeElement(this.ownerDocument, match[1]);
      const attributes = /([\w-]+)="([^"]*)"/g;
      let attribute;
      while ((attribute = attributes.exec(match[2]))) element.setAttribute(attribute[1], attribute[2]);
      if (match[1].toLowerCase() === 'script' && match[3]) {
        element.text = match[3].replace(/<\/script>$/i, '');
      }
      element.parent = this;
      this.children.push(element);
    }
  }
  querySelectorAll(selector) {
    return this.children.filter((child) => matches(child, selector));
  }
  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }
  click() { this.dispatchEvent({ type: 'click' }); }
}

function matches(element, selector) {
  return selector.split(',').some((part) => {
    const parsed = /^([a-z]+)?(?:\.([\w-]+))?(?:\[([\w-]+)(?:="?([^"\]]*)"?)?\])?$/.exec(part.trim());
    if (!parsed) return false;
    const [, tag, cls, attribute, value] = parsed;
    if (tag && element.tagName !== tag.toUpperCase()) return false;
    if (cls && !element.className.split(/\s+/).includes(cls)) return false;
    if (attribute && !element.hasAttribute(attribute)) return false;
    if (attribute && value !== undefined && element.getAttribute(attribute) !== value) return false;
    return true;
  });
}

function makeDocument(context) {
  const document = new FakeEventTarget();
  document.readyState = 'complete';
  document.hidden = false;
  document.visibilityState = 'visible';
  document.currentScript = null;
  document.scriptBehavior = {};
  document.inserted = (element) => {
    if (element.tagName === 'SCRIPT') {
      if (element.hasAttribute('src')) {
        const behaviour = document.scriptBehavior[element.getAttribute('src')] || 'load';
        if (behaviour === 'hang') return;
        setImmediate(() => {
          if (behaviour === 'load') vm.runInContext(document.scriptEffects[element.getAttribute('src')] || '', context);
          element.dispatchEvent({ type: behaviour === 'load' ? 'load' : 'error' });
        });
      } else if (element.text) {
        vm.runInContext(element.text, context);
      }
    }
    if (element.tagName === 'LINK' && element.getAttribute('rel') === 'stylesheet') {
      setImmediate(() => element.dispatchEvent({ type: 'load' }));
    }
  };
  document.scriptEffects = {};
  document.head = new FakeElement(document, 'head');
  document.main = new FakeElement(document, 'main');
  document.createElement = (tag) => {
    const element = new FakeElement(document, tag);
    if (tag === 'template') {
      Object.defineProperty(element, 'content', { get: () => element });
    }
    return element;
  };
  document.getElementById = (id) => (id === 'main-content' ? document.main : null);
  document.querySelector = () => null;
  document.querySelectorAll = () => [];
  return document;
}

// ---- one page: the shell scripts under test, on the simulated clock

function makePage({ scripts = ['api.js'] } = {}) {
  const clock = makeClock();
  const server = makeServer();
  const reloads = [];
  const assigned = [];
  const storage = new Map();
  const consolePushes = [];
  const location = {
    origin: ORIGIN,
    pathname: '/app/home/',
    reload: () => reloads.push(clock.now()),
  };
  Object.defineProperty(location, 'href', {
    get: () => ORIGIN + location.pathname,
    set: (value) => assigned.push(String(value)),
  });
  const FakeDate = class extends Date {};
  FakeDate.now = () => 1_800_000_000_000 + clock.now();
  const eventSources = [];
  class FakeEventSource extends FakeEventTarget {
    constructor(url) {
      super();
      this.url = url;
      this.readyState = 0;
      this.closed = false;
      eventSources.push(this);
    }
    close() { this.closed = true; this.readyState = 2; }
    open() { this.readyState = 1; this.onopen?.({}); }
    send(type, data) {
      const event = { type, data: JSON.stringify(data) };
      if (type === 'message') this.onmessage?.(event);
      else this.dispatchEvent(event);
    }
    error(closed) {
      this.readyState = closed ? 2 : 0;
      this.onerror?.({ type: 'error' });
    }
  }
  const window = {
    location,
    sessionStorage: {
      getItem: (key) => (storage.has(key) ? storage.get(key) : null),
      setItem: (key, value) => storage.set(key, String(value)),
    },
    convey: { diagnosticConsole: { push: (entry) => consolePushes.push(entry) } },
    performance: { mark() {} },
    matchMedia: () => ({ matches: false }),
  };
  const consoleErrors = [];
  const context = {
    window,
    console: { log() {}, debug() {}, warn() {}, error: (...args) => consoleErrors.push(args.map(String).join(' ')) },
    fetch: server.fetch,
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval,
    clearInterval: clock.clearInterval,
    Date: FakeDate,
    AbortController, DOMException, Response, ReadableStream, TextDecoder, TextEncoder, URL, URLSearchParams,
    Promise, JSON, Map, Set, WeakMap, Error, TypeError, Math, Number, String, Object, Array,
    EventSource: FakeEventSource,
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    performance: window.performance,
  };
  Object.assign(window, {
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    fetch: server.fetch,
  });
  vm.createContext(context);
  const document = makeDocument(context);
  context.document = document;
  window.document = document;
  const windowEvents = new FakeEventTarget();
  window.addEventListener = windowEvents.addEventListener.bind(windowEvents);
  window.removeEventListener = windowEvents.removeEventListener.bind(windowEvents);
  window.dispatchEvent = windowEvents.dispatchEvent.bind(windowEvents);
  for (const name of scripts) {
    vm.runInContext(staticSource(name), context, { filename: name });
  }
  return { clock, server, window, document, reloads, assigned, consolePushes, eventSources, context, consoleErrors };
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

function settledState(promise) {
  const state = { done: false, value: undefined, error: undefined };
  promise.then(
    (value) => { state.done = true; state.value = value; },
    (error) => { state.done = true; state.error = error; },
  );
  return state;
}

// ---- read bounds

asyncCase('bounds: a read whose body stops sending ends in an error at the stall bound', async () => {
  const page = makePage();
  const read = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  const body = page.server.last().respondStreaming(200, { 'Content-Type': 'application/json' });
  body.push('{"a":');
  await page.clock.advance(19000);
  assert.strictEqual(read.done, false, 'still within the stall bound');
  await page.clock.advance(1500);
  assert.strictEqual(read.done, true, 'ended at the stall bound');
  assert.strictEqual(read.error?.name, 'TimeoutError');
  assert.strictEqual(page.server.calls.length, 1, 'a read past its bound is not retried');
});

asyncCase('bounds: a slow read that keeps sending bytes is never cut off', async () => {
  const page = makePage();
  const read = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  const body = page.server.last().respondStreaming(200);
  body.push('{"parts":[');
  for (let i = 0; i < 12; i += 1) {
    await page.clock.advance(15000);
    body.push('1,');
  }
  body.push('2]}');
  body.end();
  await flush();
  assert.strictEqual(read.error, undefined);
  assert.deepStrictEqual(read.value.parts.length, 13, 'three minutes of trickle arrives whole');
});

asyncCase('bounds: no first byte by the bound ends the read; a coalesced route gets twice as long', async () => {
  const page = makePage();
  const pulse = settledState(page.window.apiJson('/app/home/api/pulse'));
  const health = settledState(page.window.apiJson('/app/health/api/state'));
  await page.clock.advance(30500);
  assert.strictEqual(pulse.error?.name, 'TimeoutError');
  assert.strictEqual(health.done, false, 'the coalesced read is still within its bound');
  await page.clock.advance(30000);
  assert.strictEqual(health.error?.name, 'TimeoutError');
  assert.ok(page.server.calls.every((call) => call.aborted), 'both requests were cut short');
});

asyncCase('bounds: a write has no deadline, so it is never reported failed while it may succeed', async () => {
  const page = makePage();
  const write = settledState(page.window.apiJson('/app/home/api/approve', { method: 'POST', body: '{}' }));
  await page.clock.advance(5 * 60 * 1000);
  assert.strictEqual(write.done, false, 'the write is still waiting on its answer');
  page.server.last().respond(200, '{"ok":true}');
  await flush();
  assert.deepStrictEqual(write.value, { ok: true });
});

asyncCase('bounds: the page\'s own fetch reads are bounded too, and its writes are left alone', async () => {
  const page = makePage();
  const read = settledState(page.window.fetch('/app/stats/api/usage?day=20261101'));
  const write = settledState(page.window.fetch('/app/settings/api/x', { method: 'PUT' }));
  const outside = settledState(page.window.fetch('https://example.invalid/x'));
  const stream = settledState(page.window.fetch('/sse/events'));
  await page.clock.advance(31000);
  assert.strictEqual(read.error?.name, 'TimeoutError');
  assert.strictEqual(write.done, false);
  assert.strictEqual(outside.done, false);
  assert.strictEqual(stream.done, false);
});

// ---- retries

for (const status of [502, 503, 504]) {
  asyncCase(`retries: a read retries a gateway ${status}, at most twice`, async () => {
    const page = makePage();
    const read = settledState(page.window.apiJson('/app/home/api/pulse'));
    await flush();
    page.server.last().respond(status, '');
    await page.clock.advance(1500);
    assert.strictEqual(page.server.calls.length, 2, 'first retry after about a second');
    page.server.last().respond(status, '');
    await page.clock.advance(4000);
    assert.strictEqual(page.server.calls.length, 3, 'second retry after about three seconds');
    page.server.last().respond(status, '');
    await page.clock.advance(10000);
    assert.strictEqual(page.server.calls.length, 3, 'no third retry');
    assert.strictEqual(read.error?.status, status);
  });
}

asyncCase('retries: a read retries a dropped connection and succeeds when it comes back', async () => {
  const page = makePage();
  const read = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  page.server.last().failTransport();
  await page.clock.advance(1500);
  page.server.last().respond(200, '{"ok":1}');
  await flush();
  assert.deepStrictEqual(read.value, { ok: 1 });
  assert.strictEqual(page.server.calls.length, 2);
});

asyncCase('retries: a write and a 4xx are never retried, nor a refusal from the phone\'s proxy', async () => {
  const page = makePage();
  const write = settledState(page.window.apiJson('/app/home/api/approve', { method: 'POST' }));
  await flush();
  page.server.last().respond(503, '');
  await page.clock.advance(10000);
  assert.strictEqual(page.server.calls.length, 1, 'a write is sent once');
  assert.strictEqual(write.error?.status, 503);
  for (const status of [400, 404, 409]) {
    const before = page.server.calls.length;
    const read = settledState(page.window.apiJson('/app/home/api/pulse'));
    await flush();
    page.server.last().respond(status, '');
    await page.clock.advance(10000);
    assert.strictEqual(page.server.calls.length, before + 1, `${status} is not retried`);
    assert.strictEqual(read.error?.status, status);
  }
  const before = page.server.calls.length;
  const refused = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  page.server.last().respond(403, '', { 'X-SPL-Loopback-Refused': 'x' });
  await page.clock.advance(10000);
  assert.strictEqual(page.server.calls.length, before + 1, 'the proxy\'s refusal is not retried');
  assert.strictEqual(refused.done, true);
});

// ---- generations: a view's reads end with its mount; the shell's don't

asyncCase('generations: a view read from an older mount is aborted and never answers', async () => {
  const page = makePage();
  const old = settledState(page.window.apiJson('/app/home/api/pulse'));
  const shell = settledState(page.window.apiJson('/api/shell', { scope: 'shell' }));
  await flush();
  page.window.beginViewGeneration();
  await flush();
  assert.strictEqual(page.server.calls[0].aborted, true, 'the old view\'s read was stopped');
  assert.strictEqual(page.server.calls[1].aborted, false, 'the shell\'s read is untouched');
  page.server.calls[1].respond(200, '{"apps":[]}');
  await page.clock.advance(120000);
  assert.strictEqual(old.done, false, 'and its answer, or its error, never reaches the page');
  assert.deepStrictEqual(shell.value, { apps: [] });
  const fresh = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  page.server.last().respond(200, '{"fresh":true}');
  await flush();
  assert.deepStrictEqual(fresh.value, { fresh: true }, 'the new mount\'s reads answer');
});

asyncCase('generations: a caller\'s own abort still rejects, as before', async () => {
  const page = makePage();
  const controller = new AbortController();
  const read = settledState(page.window.apiJson('/app/home/api/pulse', { signal: controller.signal }));
  await flush();
  controller.abort();
  await flush();
  assert.strictEqual(read.error?.name, 'AbortError');
});

// ---- abort reasons and what an abort leaves running

asyncCase('bounds: an aborted read whose body nobody reads keeps no stall clock', async () => {
  const page = makePage();
  const before = page.clock.count();
  const caller = new AbortController();
  const got = settledState(page.window.fetch('/app/home/api/pulse', { signal: caller.signal }));
  await flush();
  // A body that has sent what it has, with nobody reading it.
  page.server.last().respondStreaming(200, JSON_TYPE).push('{"n":');
  await flush();
  assert.ok(got.value, 'the headers are in');
  assert.strictEqual(page.clock.count(), before + 1, 'the stall clock runs while the body is open');
  caller.abort();
  await flush();
  assert.strictEqual(page.clock.count(), before, 'the abort ends the stall clock with the read');
  await page.clock.advance(25000);
  assert.strictEqual(page.clock.count(), before);
});

asyncCase('aborts: a read cut off by a new mount is aborted with the one reason built at load', async () => {
  const page = makePage();
  const first = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  page.window.beginViewGeneration();
  await flush();
  const second = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  page.window.beginViewGeneration();
  await flush();
  const [a, b] = page.server.calls.map((call) => call.init.signal.reason);
  assert.strictEqual(a?.name, 'AbortError');
  assert.strictEqual(a, b, 'the same reason both times, so no abort builds one');
  assert.strictEqual(first.done, false);
  assert.strictEqual(second.done, false);
});

asyncCase('aborts: a caller\'s reason is passed on, through the read and its retry wait', async () => {
  const page = makePage();
  const caller = new AbortController();
  const reason = new DOMException('left', 'AbortError');
  settledState(page.window.apiJson('/app/home/api/pulse', { signal: caller.signal }));
  await flush();
  caller.abort(reason);
  await flush();
  assert.strictEqual(page.server.last().init.signal.reason, reason, 'the request ends with the caller\'s reason');
  // A read waiting to retry a dropped connection rejects with it too.
  const waiting = new AbortController();
  const retry = settledState(page.window.apiJson('/app/home/api/briefing', { signal: waiting.signal }));
  await flush();
  page.server.last().failTransport();
  await flush();
  const again = new DOMException('left again', 'AbortError');
  waiting.abort(again);
  await flush();
  assert.strictEqual(retry.error, again, 'the retry wait settles with the reason it was aborted with');
});

asyncCase('aborts: early reads are ended with the reason built at load, adopted, refetched or left', async () => {
  const page = makePage();
  const generation = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  page.window.beginViewGeneration();
  await flush();
  const built = page.server.calls[0].init.signal.reason;
  assert.strictEqual(generation.done, false);
  // Adopted, then the mount ends.
  earlyTable(page);
  const adopted = earlyRead(page, '/app/home/api/briefing');
  settledState(page.window.apiJson('/app/home/api/briefing'));
  await flush();
  page.window.beginViewGeneration();
  await flush();
  assert.strictEqual(adopted.controller.signal.reason, built, 'an adopted read ends with the reason');
  // Answered with the wrong type, so asked again.
  earlyTable(page);
  const wrong = earlyAnswer(page, '/app/home/api/stats', new Response('<p>', { status: 200, headers: { 'Content-Type': 'text/html' } }));
  settledState(page.window.apiJson('/app/home/api/stats'));
  await flush();
  assert.strictEqual(wrong.signal.reason, built, 'a read asked again ends with the reason');
  // Left for nothing to take.
  earlyTable(page);
  const left = earlyRead(page, '/app/home/api/events');
  page.window.settleEarly();
  assert.strictEqual(left.controller.signal.reason, built, 'an unclaimed read ends with the reason');
});

asyncCase('aborts: a deadline aborts with its own error, never with the view\'s reason', async () => {
  const page = makePage();
  const read = settledState(page.window.apiJson('/app/home/api/pulse'));
  const plain = settledState(page.window.fetch('/app/home/api/briefing'));
  await page.clock.advance(30500);
  for (const call of page.server.calls) {
    assert.ok(call.init.signal.reason instanceof page.window.ReadDeadlineError, call.url + ' ends with its deadline');
  }
  assert.strictEqual(read.error?.name, 'TimeoutError');
  assert.strictEqual(plain.error?.name, 'TimeoutError');
});

// ---- the mount

// ---- reads started in the page's head

function earlyTable(page, view = 'x') {
  const table = { view, claimed: false, reads: Object.create(null), stats: {} };
  page.window.__solEarly = table;
  return table;
}

function earlyRead(page, path, expect = 'json') {
  const table = page.window.__solEarly;
  const controller = new AbortController();
  const response = page.server.fetch(path, { signal: controller.signal });
  response.catch(() => {});
  table.reads[ORIGIN + path] = { response, controller, startedAt: page.context.Date.now(), expect };
  return { controller, call: page.server.last() };
}

function earlyAnswer(page, path, response, expect = 'json') {
  const controller = new AbortController();
  page.window.__solEarly.reads[ORIGIN + path] = {
    response: Promise.resolve(response), controller, startedAt: page.context.Date.now(), expect,
  };
  return controller;
}

const JSON_TYPE = { 'Content-Type': 'application/json' };

asyncCase('early: a plain success is adopted and asked once', async () => {
  const page = makePage();
  earlyTable(page);
  const { call } = earlyRead(page, '/app/home/api/pulse');
  const read = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  assert.strictEqual(page.server.calls.length, 1, 'no second request');
  call.respond(200, '{"a":1}', JSON_TYPE);
  await flush();
  assert.deepStrictEqual(JSON.parse(JSON.stringify(read.value)), { a: 1 });
  assert.strictEqual(page.window.__solEarly.stats.adopted, 1);
});

asyncCase('early: a plain fetch adopts by its exact address too', async () => {
  const page = makePage();
  earlyTable(page);
  const { call } = earlyRead(page, '/app/stats/api/stats');
  const read = settledState(page.window.fetch('/app/stats/api/stats', { credentials: 'same-origin' }));
  await flush();
  call.respond(200, '{"b":2}', JSON_TYPE);
  await flush();
  assert.strictEqual(page.server.calls.length, 1);
  assert.strictEqual(read.value.status, 200);
});

const UNUSABLE = [
  ['a redirect', () => ({ ok: true, status: 200, redirected: true, headers: new Headers(JSON_TYPE), body: null })],
  ['a 401', () => new Response('{}', { status: 401, headers: JSON_TYPE })],
  ['a 403', () => new Response('{}', { status: 403, headers: JSON_TYPE })],
  ['a page where JSON was expected', () => new Response('<html>', { status: 200, headers: { 'Content-Type': 'text/html' } })],
];
for (const [label, response] of UNUSABLE) {
  asyncCase(`early: ${label} is thrown away and the view asks through apiJson`, async () => {
    const page = makePage();
    earlyTable(page);
    earlyAnswer(page, '/app/home/api/pulse', response());
    const read = settledState(page.window.apiJson('/app/home/api/pulse'));
    await flush();
    assert.strictEqual(page.server.calls.length, 1, 'the real request went out');
    page.server.last().respond(200, '{"c":3}', JSON_TYPE);
    await flush();
    assert.strictEqual(read.value.c, 3);
    assert.strictEqual(page.window.__solEarly.stats.refetched, 1);
  });
}

asyncCase('early: a 401 on the real request still signs in, exactly as today', async () => {
  const page = makePage();
  earlyTable(page);
  earlyAnswer(page, '/app/home/api/pulse', new Response('{}', { status: 401, headers: JSON_TYPE }));
  const read = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  page.server.last().respond(401, '{}', JSON_TYPE);
  await flush();
  assert.deepStrictEqual(page.assigned, ['/']);
  assert.ok(read.error);
});

asyncCase('early: a gateway failure spends one of the two ordinary retries, never an extra one', async () => {
  const page = makePage();
  earlyTable(page);
  earlyAnswer(page, '/app/home/api/pulse', new Response('', { status: 503 }));
  const read = settledState(page.window.apiJson('/app/home/api/pulse'));
  await page.clock.advance(2000);
  assert.strictEqual(page.server.calls.length, 1, 'first retry');
  page.server.last().respond(503);
  await page.clock.advance(5000);
  assert.strictEqual(page.server.calls.length, 2, 'second and last retry');
  page.server.last().respond(503);
  await page.clock.advance(10000);
  assert.strictEqual(page.server.calls.length, 2, 'no third request');
  assert.ok(read.error);
});

asyncCase('early: in flight past the bound is kept, and expires with no second request', async () => {
  const page = makePage();
  earlyTable(page);
  earlyRead(page, '/app/stats/api/stats');
  await page.clock.advance(25000);
  const read = settledState(page.window.apiJson('/app/stats/api/stats'));
  await page.clock.advance(4000);
  assert.strictEqual(read.done, false, 'adopted, with what is left of its bound');
  await page.clock.advance(2000);
  assert.strictEqual(read.error?.name, 'TimeoutError');
  assert.strictEqual(read.error?.deadline, 'first-byte');
  assert.strictEqual(page.server.calls.length, 1, 'a read past its bound is not asked again');
});

asyncCase('early: a single-flight route keeps its own longer bound', async () => {
  const page = makePage();
  earlyTable(page);
  earlyRead(page, '/app/health/api/state');
  await page.clock.advance(40000);
  const read = settledState(page.window.apiJson('/app/health/api/state'));
  await page.clock.advance(15000);
  assert.strictEqual(read.done, false, 'inside the 60 s bound');
  await page.clock.advance(6000);
  assert.strictEqual(read.error?.name, 'TimeoutError');
  assert.strictEqual(page.server.calls.length, 1);
});

asyncCase('early: a near-miss address is never adopted', async () => {
  const page = makePage();
  earlyTable(page);
  earlyRead(page, '/app/entities/api/journal/summary?offset=0&limit=50');
  const read = settledState(page.window.apiJson('/app/entities/api/journal/summary?offset=0&limit=25'));
  await flush();
  assert.strictEqual(page.server.calls.length, 2, 'it asked for its own');
  page.server.last().respond(200, '{"own":true}', JSON_TYPE);
  await flush();
  assert.strictEqual(read.value.own, true);
});

asyncCase('early: taken once, so a later read of the same address asks the journal', async () => {
  const page = makePage();
  earlyTable(page);
  const { call } = earlyRead(page, '/app/home/api/pulse');
  const first = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  call.respond(200, '{"n":1}', JSON_TYPE);
  await flush();
  assert.strictEqual(first.value.n, 1);
  const again = settledState(page.window.apiJson('/app/home/api/pulse'));
  await flush();
  assert.strictEqual(page.server.calls.length, 2);
  page.server.last().respond(200, '{"n":2}', JSON_TYPE);
  await flush();
  assert.strictEqual(again.value.n, 2);
});

asyncCase('early: settle aborts what nothing took, and nothing is adopted after it', async () => {
  const page = makePage();
  earlyTable(page);
  const { controller } = earlyRead(page, '/app/home/api/briefing');
  page.window.settleEarly();
  assert.strictEqual(controller.signal.aborted, true);
  assert.strictEqual(page.window.__solEarly.stats.unclaimed, 1);
  const read = settledState(page.window.apiJson('/app/home/api/briefing'));
  await flush();
  assert.strictEqual(page.server.calls.length, 2, 'asked for itself');
  page.server.last().respond(200, '{"d":1}', JSON_TYPE);
  await flush();
  assert.strictEqual(read.value.d, 1);
});

asyncCase('early: a view the owner left aborts the read and it is never drawn', async () => {
  const page = makePage();
  earlyTable(page, 'home');
  const { controller, call } = earlyRead(page, '/app/home/api/pulse');
  assert.strictEqual(page.window.claimEarlyView('settings'), false);
  assert.strictEqual(controller.signal.aborted, true);
  assert.strictEqual(call.aborted, true);
  assert.strictEqual(page.window.__solEarly.reads, null);
});

asyncCase('early: the mount adopts the fragment, and a second mount adopts nothing', async () => {
  const page = mountPage();
  earlyTable(page, 'x');
  const { call } = earlyRead(page, '/app/x/workspace', 'html');
  const first = settledState(page.window.mountWorkspaceFragment('/app/x/workspace', { appName: 'x' }));
  await flush();
  assert.strictEqual(page.server.calls.length, 1);
  call.respond(200, '<div id="v"></div>', { 'Content-Type': 'text/html; charset=utf-8' });
  await page.clock.advance(100);
  assert.ok(first.done && !first.error, 'mounted');
  assert.strictEqual(page.window.__solEarly.stats.adopted, 1);
  earlyRead(page, '/app/x/api/state');
  const second = settledState(page.window.mountWorkspaceFragment('/app/x/workspace', { appName: 'x' }));
  await flush();
  assert.strictEqual(page.server.calls.length, 3, 'the second mount asked for its own fragment');
  page.server.last().respond(200, '<div></div>', { 'Content-Type': 'text/html' });
  await page.clock.advance(100);
  assert.ok(second.done);
  assert.strictEqual(page.window.__solEarly.reads, null, 'the first mount\'s leftovers are gone');
});

asyncCase('early: an adopted read belongs to the mount that used it', async () => {
  const page = mountPage();
  earlyTable(page, 'x');
  const { controller } = earlyRead(page, '/app/x/api/state');
  page.window.beginViewGeneration();
  page.window.claimEarlyView('x');
  const read = settledState(page.window.apiJson('/app/x/api/state'));
  await flush();
  page.window.beginViewGeneration();
  await flush();
  assert.strictEqual(controller.signal.aborted, true);
  assert.strictEqual(read.done, false, 'never delivered to the view that left');
});

// The page's own head script, against a table like the one the build writes.
function runHead({ pathname, search = '', hash = '', table }) {
  const page = fs.readFileSync(path.join(crateDir, 'assets/static/shell.html'), 'utf8');
  const script = page.match(/<script>\s*\/\/ The journal's front door[\s\S]*?<\/script>/)[0]
    .replace(/^<script>/, '').replace(/<\/script>$/, '')
    .replace('{/*entry-table*/}', JSON.stringify(table));
  const asked = [];
  const links = [];
  const location = { pathname, search, hash };
  Object.defineProperty(location, 'href', { get: () => ORIGIN + location.pathname + location.search + location.hash });
  const window = { location, addEventListener() {} };
  window.history = {
    state: null,
    replaceState(_state, _title, url) {
      const next = new URL(url, ORIGIN);
      location.pathname = next.pathname;
      location.hash = next.hash;
    },
  };
  const context = {
    window,
    document: {
      createElement: () => ({}),
      head: { appendChild: (link) => links.push(link) },
    },
    fetch: (url) => { asked.push(new URL(url).pathname); return Promise.resolve(); },
    AbortController,
    URL,
    URLSearchParams,
    Date,
    Object,
  };
  vm.runInNewContext(script, context);
  return { asked, links, window };
}

const HEAD_TABLE = {
  views: {
    home: {
      fragment: '/app/home/workspace',
      scripts: [{ src: '/h/home.js' }, { src: '/h/week.js', routePrefix: '/app/home/week/' }],
      styles: ['/h/home.css'],
      reads: [{ route: '/app/home/', urls: ['/app/home/api/pulse', '/app/home/api/briefing'] }],
    },
    transcripts: {
      fragment: '/app/transcripts/workspace',
      scripts: [],
      styles: [],
      reads: [{ route: '/app/transcripts/{day}', urls: ['/app/transcripts/api/day/{day}'] }],
      openDay: { route: '/app/transcripts/', url: '/app/transcripts/api/open-day' },
    },
  },
};
const SHELL_READS = ['/api/shell', '/api/streams/labels'];

asyncCase('head: a cold / starts home, read from the path after it becomes /app/home/', async () => {
  const { asked, links } = runHead({ pathname: '/', table: HEAD_TABLE });
  assert.deepStrictEqual(asked, [...SHELL_READS, '/app/home/workspace', '/app/home/api/pulse', '/app/home/api/briefing']);
  assert.deepStrictEqual(links.map((link) => [link.href, link.as]), [['/h/home.js', 'script'], ['/h/home.css', 'style']]);
});

asyncCase('head: a dated route reads its own day; an undated one reads what picks the day', async () => {
  assert.ok(runHead({ pathname: '/app/transcripts/20260731', table: HEAD_TABLE }).asked.includes('/app/transcripts/api/day/20260731'));
  const undated = runHead({ pathname: '/app/transcripts/', table: HEAD_TABLE }).asked;
  assert.ok(undated.includes('/app/transcripts/api/open-day'));
  assert.ok(!undated.some((url) => url.startsWith('/app/transcripts/api/day/')), 'the page never works out a date');
});

asyncCase('head: a query or a hash leaves the reads to the view; the code still starts', async () => {
  for (const extra of [{ search: '?facet=work' }, { hash: '#runs' }]) {
    const { asked } = runHead({ pathname: '/app/home/', table: HEAD_TABLE, ...extra });
    assert.deepStrictEqual(asked, [...SHELL_READS, '/app/home/workspace']);
  }
});

asyncCase('head: a route without a declaration, or an unknown view, starts no reads', async () => {
  assert.deepStrictEqual(runHead({ pathname: '/app/home/week/2026-W31/', table: HEAD_TABLE }).asked, [...SHELL_READS, '/app/home/workspace']);
  assert.deepStrictEqual(runHead({ pathname: '/app/constructor/', table: HEAD_TABLE }).asked, SHELL_READS);
});

function mountPage() {
  const page = makePage({ scripts: ['api.js', 'mount-workspace.js'] });
  page.window.SurfaceState = {
    error: () => '<button class="surface-state-retry">x</button>',
  };
  page.window.createViewReady = () => () => {};
  page.window.appEvents = { buildChanged: () => false };
  return page;
}

asyncCase('mount: a chain that fails after a script ran retries as a full load, never stacking listeners', async () => {
  const page = mountPage();
  page.document.scriptEffects['/a.js'] = "document.addEventListener('click', function () {});";
  page.document.scriptBehavior['/b.js'] = 'error';
  const mount = settledState(page.window.mountWorkspaceFragment('/app/x/workspace', { appName: 'x' }));
  await flush();
  page.server.calls[0].respond(200, '<script src="/a.js"></script><script src="/b.js"></script>');
  await page.clock.advance(100);
  assert.ok(mount.error, 'the mount failed');
  assert.strictEqual(page.document.listenerCount('click'), 1, 'the first script ran');
  page.document.main.querySelector('.surface-state-retry').click();
  await page.clock.advance(100);
  assert.strictEqual(page.reloads.length, 1, 'try again loads the whole page');
  assert.strictEqual(page.server.calls.filter((call) => call.url === '/app/x/workspace').length, 1,
    'the fragment was not mounted again on top');
  assert.strictEqual(page.document.listenerCount('click'), 1);
});

asyncCase('mount: a failure before any script ran retries in place', async () => {
  const page = mountPage();
  const first = settledState(page.window.mountWorkspaceFragment('/app/x/workspace', { appName: 'x' }));
  await flush();
  page.server.calls[0].respond(404, '');
  await flush();
  assert.ok(first.error);
  page.document.main.querySelector('.surface-state-retry').click();
  await flush();
  assert.strictEqual(page.reloads.length, 0);
  assert.strictEqual(page.server.calls.length, 2, 'the fragment is asked for again');
});

asyncCase('mount: a script that never loads ends the mount at its bound', async () => {
  const page = mountPage();
  page.document.scriptBehavior['/slow.js'] = 'hang';
  const mount = settledState(page.window.mountWorkspaceFragment('/app/x/workspace', { appName: 'x' }));
  await flush();
  page.server.calls[0].respond(200, '<script src="/slow.js"></script>');
  await page.clock.advance(59000);
  assert.strictEqual(mount.done, false);
  await page.clock.advance(2000);
  assert.ok(mount.error, 'the mount ends in its error state');
});

asyncCase('mount: a fragment that stops sending ends in the error state', async () => {
  const page = mountPage();
  const mount = settledState(page.window.mountWorkspaceFragment('/app/x/workspace', { appName: 'x' }));
  await flush();
  page.server.calls[0].respondStreaming(200).push('<div>');
  await page.clock.advance(21000);
  assert.ok(mount.error);
  assert.ok(page.document.main.querySelector('.surface-state-retry'));
});

asyncCase('mount: an in-page remount leaves the shell\'s reads running', async () => {
  const page = mountPage();
  const shellRead = settledState(page.window.apiJson('/api/shell', { scope: 'shell' }));
  const viewRead = settledState(page.window.apiJson('/app/x/api/state'));
  await flush();
  page.window.mountWorkspaceFragment('/app/y/workspace', { appName: 'y' }).catch(() => {});
  await flush();
  assert.strictEqual(page.server.calls[0].aborted, false, 'the shell read keeps going');
  assert.strictEqual(page.server.calls[1].aborted, true, 'the old view read stops');
  page.server.calls[0].respond(200, '{"ok":true}');
  await flush();
  assert.deepStrictEqual(shellRead.value, { ok: true });
  assert.strictEqual(viewRead.done, false);
});

asyncCase('mount: after the journal was upgraded, the next mount loads the whole page once', async () => {
  const page = mountPage();
  page.window.appEvents = { buildChanged: () => true };
  await page.window.mountWorkspaceFragment('/app/x/workspace', { appName: 'x' });
  assert.strictEqual(page.reloads.length, 1);
  assert.strictEqual(page.server.calls.length, 0, 'no fragment from the new build is mixed in');
});

// ---- the stream

function streamPage() {
  const page = makePage({ scripts: ['api.js', 'websocket.js'] });
  return page;
}

function openStream(source, build = 'aaaa') {
  source.open();
  source.send('hello', { server_build: build });
  source.send('continuity', { state: 'connected', generation: 1, epoch: 1 });
}

function beat(source) {
  source.send('heartbeat', { at: 1 });
}

function aboutCalls(page) {
  return page.server.calls.filter((call) => call.url === '/api/system/about');
}

// A probe asks the same question on several connections at once.
const FANOUT = 6;

function probeRounds(page) {
  return aboutCalls(page).length / FANOUT;
}

asyncCase('stream: steady heartbeats never start a probe', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  for (let i = 0; i < 60; i += 1) {
    await page.clock.advance(5000);
    beat(page.eventSources[0]);
  }
  assert.strictEqual(aboutCalls(page).length, 0);
  assert.strictEqual(page.window.appEvents.getMetrics().state, 'connected');
  assert.strictEqual(page.eventSources.length, 1);
});

asyncCase('stream: silence starts a probe, and an answer opens a new stream with no click', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  await page.clock.advance(14000);
  assert.strictEqual(aboutCalls(page).length, 0, 'not before the silence bound');
  await page.clock.advance(4000);
  assert.strictEqual(probeRounds(page), 1, 'the watchdog, not the browser, asks');
  aboutCalls(page)[0].respond(200, '{}');
  await flush();
  assert.strictEqual(page.eventSources.length, 2, 'a new stream');
  assert.strictEqual(page.eventSources[0].closed, true, 'the silent one is closed');
  openStream(page.eventSources[1]);
  assert.strictEqual(page.window.appEvents.getMetrics().state, 'connected');
});

asyncCase('stream: a silently dead link is back within ten seconds of a ten second drop', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  await page.clock.advance(4900);
  beat(page.eventSources[0]);
  // the drop: no bytes, no error, for ten seconds; the probe can't get through
  const dropStart = page.clock.now();
  page.server.failing = true;
  const answerDuringDrop = () => {
    for (const call of aboutCalls(page)) {
      if (!call.settled && page.clock.now() < dropStart + 10000) call.failTransport();
    }
  };
  while (page.clock.now() < dropStart + 10000) {
    await page.clock.advance(250);
    answerDuringDrop();
  }
  let back = null;
  while (page.clock.now() < dropStart + 40000 && back === null) {
    await page.clock.advance(250);
    for (const call of aboutCalls(page)) if (!call.settled) call.respond(200, '{}');
    const latest = page.eventSources[page.eventSources.length - 1];
    if (page.eventSources.length > 1 && latest.readyState === 0) {
      openStream(latest);
      back = page.clock.now();
    }
  }
  assert.ok(back !== null, 'the stream came back');
  assert.ok(back - (dropStart + 10000) <= 10000, `back ${back - dropStart - 10000} ms after the link returned`);
});

asyncCase('stream: "connected" is never shown after forty seconds of silence', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  let connectedWhileSilent = 0;
  for (let t = 0; t < 120000; t += 500) {
    await page.clock.advance(500);
    for (const call of aboutCalls(page)) if (!call.settled) call.failTransport();
    if (page.window.appEvents.getMetrics().state === 'connected') connectedWhileSilent = t + 500;
  }
  assert.ok(connectedWhileSilent < 40000, `connected shown at ${connectedWhileSilent} ms of silence`);
});

asyncCase('stream: one copy of the probe on a connection that never answers doesn\'t hold it up', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  await page.clock.advance(18000);
  const round = aboutCalls(page);
  assert.strictEqual(round.length, FANOUT);
  round[2].respond(200, '{}');
  await flush();
  assert.strictEqual(page.eventSources.length, 2, 'the first answer reopens the stream at once');
  assert.ok(round.filter((call) => call !== round[2]).every((call) => call.aborted), 'the rest are stopped');
});

asyncCase('stream: a lost sign-in goes to the sign-in path, with no reconnect loop', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  page.eventSources[0].error(true);
  await page.clock.advance(3000);
  assert.strictEqual(probeRounds(page), 1);
  aboutCalls(page)[0].respond(401, '');
  await page.clock.advance(60000);
  assert.deepStrictEqual(page.assigned, ['/']);
  assert.strictEqual(page.eventSources.length, 1, 'no new stream is opened');
  assert.strictEqual(probeRounds(page), 1, 'and no more probes');
  assert.ok(aboutCalls(page).every((call) => call.settled), 'the other copies were stopped');
});

asyncCase('stream: a stream that keeps closing after the journal answers backs off', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  for (let t = 0; t < 120000; t += 500) {
    const latest = page.eventSources[page.eventSources.length - 1];
    if (latest.readyState !== 2) latest.error(true);
    for (const call of aboutCalls(page)) if (!call.settled) call.respond(200, '{}');
    await page.clock.advance(500);
  }
  const opened = page.eventSources.length;
  assert.ok(opened <= 15, `${opened} streams in two minutes`);
});

async function lostWithBackoff(page) {
  openStream(page.eventSources[0]);
  page.eventSources[0].error(true);
  // the link is down: the first rounds find nothing, and the backoff grows
  for (let round = 0; round < 4; round += 1) {
    await page.clock.advance(500);
    for (const call of aboutCalls(page)) if (!call.settled) call.failTransport();
    await page.clock.advance(4000);
    for (const call of aboutCalls(page)) if (!call.settled) call.failTransport();
  }
  await flush();
  return aboutCalls(page).length;
}

asyncCase('stream: the network coming back asks at once, not after the backoff', async () => {
  const page = streamPage();
  const asked = await lostWithBackoff(page);
  page.window.dispatchEvent({ type: 'online' });
  await flush();
  assert.strictEqual(aboutCalls(page).length, asked + FANOUT, 'one round at once');
  aboutCalls(page)[aboutCalls(page).length - 1].respond(200, '{}');
  await flush();
  assert.strictEqual(page.eventSources.length, 2, 'a new stream with no wait');
});

asyncCase('stream: coming back into view after a loss asks at once', async () => {
  const page = streamPage();
  await lostWithBackoff(page);
  page.document.hidden = true;
  page.document.dispatchEvent({ type: 'visibilitychange' });
  await page.clock.advance(60000);
  for (const call of aboutCalls(page)) if (!call.settled) call.failTransport();
  await flush();
  const asked = aboutCalls(page).length;
  page.document.hidden = false;
  page.document.dispatchEvent({ type: 'visibilitychange' });
  await flush();
  assert.strictEqual(aboutCalls(page).length, asked + FANOUT);
});

asyncCase('stream: the network coming back mid-question asks again as soon as it ends', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  page.eventSources[0].error(true);
  await page.clock.advance(3000);
  const first = aboutCalls(page).length;
  assert.strictEqual(first, FANOUT, 'a round is out');
  page.window.dispatchEvent({ type: 'online' });
  await flush();
  assert.strictEqual(aboutCalls(page).length, first, 'no second round while one is out');
  for (const call of aboutCalls(page)) if (!call.settled) call.failTransport();
  await flush();
  assert.strictEqual(aboutCalls(page).length, first + FANOUT, 'the next round goes at once');
});

asyncCase('stream: a healthy page coming into view, or online again, asks nothing', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  page.window.dispatchEvent({ type: 'online' });
  page.document.dispatchEvent({ type: 'visibilitychange' });
  await page.clock.advance(5000);
  beat(page.eventSources[0]);
  await flush();
  assert.strictEqual(aboutCalls(page).length, 0);
});

asyncCase('stream: a handled loss is not logged as an error', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  page.eventSources[0].error(false);
  page.eventSources[0].error(true);
  await flush();
  assert.deepStrictEqual(page.consoleErrors, []);
});

asyncCase('stream: a new build on reconnect marks the page stale', async () => {
  const page = streamPage();
  openStream(page.eventSources[0], 'build-a');
  assert.strictEqual(page.window.appEvents.buildChanged(), false);
  await page.clock.advance(18000);
  aboutCalls(page)[0].respond(200, '{}');
  await flush();
  openStream(page.eventSources[1], 'build-b');
  assert.strictEqual(page.window.appEvents.buildChanged(), true);
  assert.strictEqual(page.reloads.length, 0, 'a page being read is not reloaded');
});

asyncCase('stream: a page back from the background waits for its buffered events before probing', async () => {
  const page = streamPage();
  openStream(page.eventSources[0]);
  page.document.hidden = true;
  await page.clock.advance(60000);
  assert.strictEqual(aboutCalls(page).length, 0, 'a hidden page does not probe');
  page.document.hidden = false;
  page.document.dispatchEvent({ type: 'visibilitychange' });
  await page.clock.advance(500);
  beat(page.eventSources[0]);
  await page.clock.advance(5000);
  assert.strictEqual(aboutCalls(page).length, 0, 'the stream was fine');
  assert.strictEqual(page.window.appEvents.getMetrics().state, 'connected');
});

// ---- hidden pages

function appPage() {
  const page = makePage({ scripts: ['api.js'] });
  page.window.ConveyIcons = { svg: () => '' };
  page.window.CONVEY_COPY = {};
  page.document.body = page.document.createElement('body');
  vm.runInContext(staticSource('app.js'), page.context, { filename: 'app.js' });
  return page;
}

function hide(page, hidden) {
  page.document.hidden = hidden;
  page.document.visibilityState = hidden ? 'hidden' : 'visible';
  page.document.dispatchEvent({ type: 'visibilitychange' });
}

asyncCase('hidden: a visible-only poll stops while hidden and reads once on return', async () => {
  const page = appPage();
  let runs = 0;
  page.window.AppServices.visibleInterval(() => { runs += 1; }, 15000);
  await page.clock.advance(30000);
  assert.strictEqual(runs, 2);
  hide(page, true);
  await page.clock.advance(5 * 60 * 1000);
  assert.strictEqual(runs, 2, 'nothing runs while hidden');
  hide(page, false);
  await flush();
  assert.strictEqual(runs, 3, 'one read on return');
  await page.clock.advance(15000);
  assert.strictEqual(runs, 4, 'then the cadence carries on');
});

asyncCase('hidden: a wait for the page to be seen runs once, when it is', async () => {
  const page = appPage();
  let runs = 0;
  hide(page, true);
  page.window.AppServices.whenVisible(() => { runs += 1; });
  await page.clock.advance(60000);
  assert.strictEqual(runs, 0);
  hide(page, false);
  hide(page, true);
  hide(page, false);
  assert.strictEqual(runs, 1);
});

asyncCase('still working: every loading state carries the line, held back for ten seconds', async () => {
  const page = appPage();
  const html = page.window.SurfaceState.loading({ text: 'loading' });
  assert.ok(html.includes('data-role="still-working"'), html);
  const css = staticSource('app.css');
  const rule = /\.surface-state-still\s*\{([^}]*)\}/.exec(css);
  assert.ok(rule && /visibility:\s*hidden/.test(rule[1]), 'hidden from sight and from assistive technology');
  assert.ok(/animation:[^;]*\b10s\b/.test(rule[1]), 'shown at ten seconds');
  const handWritten = [];
  const roots = [path.join(crateDir, 'assets'), ...['home-web', 'transcripts-web', 'import-web']
    .map((crate) => path.join(crateDir, '..', `solstone-core-${crate}`, 'assets'))];
  const walk = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) { if (entry.name !== 'tests' && entry.name !== 'vendor') walk(full); continue; }
      if (!/\.(html|js)$/.test(entry.name)) continue;
      const text = fs.readFileSync(full, 'utf8');
      const states = text.split('surface-state--loading').length - 1;
      const lines = text.split('data-role="still-working"').length - 1;
      if (states && full.endsWith('app.js')) continue;
      if (states !== lines) handWritten.push(`${path.relative(crateDir, full)}: ${states} states, ${lines} lines`);
    }
  };
  roots.forEach(walk);
  assert.deepStrictEqual(handWritten, []);
});

(async () => {
  for (const run of queue) await run();
  console.log('DOM CASES: ' + cases + ' passed');
})().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
