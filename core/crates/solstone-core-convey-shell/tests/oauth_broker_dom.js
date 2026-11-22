// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

'use strict';

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const crypto = require('crypto');

const crateDir = process.argv[2];
const staticDir = path.join(crateDir, 'assets/static');

const hostJs = fs.readFileSync(path.join(staticDir, 'journal-web-host.js'), 'utf8');
const outsideJs = fs.readFileSync(path.join(staticDir, 'external-navigation.js'), 'utf8');
const brokerJs = fs.readFileSync(path.join(staticDir, 'oauth-broker.js'), 'utf8');

function createHarness(opts = {}) {
  const openedPopups = [];
  const assignedLocations = [];
  const listeners = new Map();

  const location = {
    hostname: opts.hostname ?? '127.0.0.1',
    origin: opts.origin ?? 'http://127.0.0.1:5015',
    assign(url) {
      assignedLocations.push(url);
    },
  };

  const sandbox = {
    URL,
    AbortController,
    crypto: { randomUUID: () => crypto.randomUUID() },
    console,
    navigator: { userAgent: opts.userAgent ?? 'Mozilla/5.0' },
    location,
    addEventListener(type, fn) {
      if (!listeners.has(type)) listeners.set(type, []);
      listeners.get(type).push(fn);
    },
    open(url, target, features) {
      if (opts.blockPopups) return null;
      const popup = {
        closed: false,
        opener: null,
        location: {
          href: url,
          replace(next) {
            this.href = next;
          },
        },
        close() {
          this.closed = true;
        },
        target,
        features,
      };
      openedPopups.push(popup);
      return popup;
    },
  };
  sandbox.window = sandbox;
  sandbox.top = opts.inIframe ? {} : sandbox;

  const context = vm.createContext(sandbox);
  vm.runInContext(hostJs, context, { filename: 'journal-web-host.js' });
  if (opts.nativeV1Capability) {
    sandbox[sandbox.solstoneJournalWebHost.javascriptCapability] = 1;
  }
  if (opts.journalHost) {
    sandbox[sandbox.solstoneAuthContract.capability] = opts.journalHost;
  }
  vm.runInContext(outsideJs, context, { filename: 'external-navigation.js' });
  vm.runInContext(brokerJs, context, { filename: 'oauth-broker.js' });

  return {
    window: sandbox,
    broker: sandbox.solstoneOAuthBroker,
    openedPopups,
    assignedLocations,
    dispatch(type) {
      for (const fn of listeners.get(type) || []) fn();
    },
  };
}

const PREFS = [
  { mode: 'loopback', host: '127.0.0.1', path: '/auth/callback', port: 0 },
  { mode: 'custom_scheme', uri: 'solstone://oauth/callback' },
];

let passed = 0;

async function runCase(name, fn) {
  try {
    await fn();
    passed += 1;
  } catch (error) {
    console.error(`FAIL ${name}: ${error && error.stack || error}`);
    process.exitCode = 1;
  }
}

async function main() {
  // Verify headless constraint: zero DOM creation or queries in oauth-broker.js.
  await runCase('oauth-broker.js stays strictly headless with no DOM references', async () => {
    assert(!/\bdocument\b/.test(brokerJs), 'oauth-broker.js must not reference document');
    assert(!/innerHTML|createElement|querySelector/.test(brokerJs), 'oauth-broker.js must have no DOM APIs');
  });

  // Verify window.solstoneAuthContract is frozen and matches frozen contract tokens.
  await runCase('window.solstoneAuthContract is exposed and frozen', async () => {
    const { window } = createHarness();
    const contract = window.solstoneAuthContract;
    assert.strictEqual(Object.isFrozen(contract), true);
    assert.deepStrictEqual({ ...contract }, {
      authorize: 'auth.authorize',
      cancel: 'auth.cancel',
      capability: 'solstoneJournalHost',
      prepare: 'auth.prepare',
      prepared: 'auth.prepared',
      result: 'auth.result',
      unsupported: 'unsupported',
      version: 2,
    });
  });

  // Top-frame isolation: iframe sees supportsNativeBroker() === false even if host object exists.
  await runCase('iframe disables native broker support', async () => {
    const { broker } = createHarness({
      inIframe: true,
      journalHost: {
        version: 2,
        types: ['import.pick', 'auth.prepare', 'auth.authorize', 'auth.cancel'],
        send: async () => { throw new Error('must not be called'); },
      },
    });
    assert.strictEqual(broker.supportsNativeBroker(), false);
  });

  // Step 1: Native app broker happy path (loopback / custom_scheme).
  await runCase('step 1 native broker completes prepare -> start -> authorize -> finish', async () => {
    const sent = [];
    const journalHost = {
      version: 2,
      types: ['import.pick', 'auth.prepare', 'auth.authorize', 'auth.cancel'],
      async send(msg) {
        sent.push(msg);
        if (msg.type === 'auth.prepare') {
          return {
            type: 'auth.prepared',
            id: msg.id,
            outcome: 'ready',
            session_id: 'sess-1',
            mode: 'loopback',
            redirect_uri: 'http://127.0.0.1:54321/auth/callback',
          };
        }
        if (msg.type === 'auth.authorize') {
          return {
            type: 'auth.result',
            id: msg.id,
            outcome: 'callback',
            callback_url: 'http://127.0.0.1:54321/auth/callback?code=c1&state=st1',
          };
        }
        throw new Error('unexpected msg: ' + msg.type);
      },
    };
    const { broker } = createHarness({ journalHost, nativeV1Capability: true });
    assert.strictEqual(broker.supportsNativeBroker(), true);
    assert.strictEqual(broker.isSameMachineBrowser(), false);

    let finishArgs = null;
    const res = await broker.run({
      preferences: PREFS,
      async start(ctx) {
        assert.strictEqual(ctx.strategy, 'native_broker');
        assert.strictEqual(ctx.brokerMode, 'loopback');
        assert.strictEqual(ctx.redirectUri, 'http://127.0.0.1:54321/auth/callback');
        return {
          attemptId: 'att-1',
          authorizeUrl: 'https://provider.example/authorize?state=st1',
          expectedState: 'st1',
          timeoutSeconds: 300,
        };
      },
      async finish(ctx) {
        finishArgs = ctx;
        return { signedIn: true };
      },
    });

    assert.strictEqual(res.ok, true);
    assert.strictEqual(res.strategy, 'native_broker');
    assert.strictEqual(res.brokerMode, 'loopback');
    assert.deepStrictEqual(res.result, { signedIn: true });
    assert.strictEqual(finishArgs.callbackUrl, 'http://127.0.0.1:54321/auth/callback?code=c1&state=st1');
    assert.strictEqual(sent.length, 2);
    assert.strictEqual(sent[0].type, 'auth.prepare');
    assert.strictEqual(sent[1].type, 'auth.authorize');
    assert.strictEqual(sent[1].session_id, 'sess-1');
    assert.strictEqual(sent[1].expected_state, 'st1');
    assert.strictEqual(sent[1].timeout_seconds, 300);
  });

  // Step 1 teardown: start() failure or AbortSignal sends auth.cancel cleanly.
  await runCase('step 1 sends auth.cancel on start failure and on abort signal', async () => {
    const sent = [];
    let resolveAuth;
    const journalHost = {
      version: 2,
      types: ['auth.prepare', 'auth.authorize', 'auth.cancel'],
      async send(msg) {
        sent.push(msg);
        if (msg.type === 'auth.prepare') {
          return {
            type: 'auth.prepared',
            id: msg.id,
            outcome: 'ready',
            session_id: 'sess-abort',
            mode: 'custom_scheme',
            redirect_uri: 'solstone://oauth/callback',
          };
        }
        if (msg.type === 'auth.authorize') {
          return new Promise((resolve) => {
            resolveAuth = () => resolve({ type: 'auth.result', id: msg.id, outcome: 'cancelled' });
          });
        }
        if (msg.type === 'auth.cancel') {
          if (resolveAuth) resolveAuth();
          return { type: 'auth.result', id: msg.id, outcome: 'cancelled' };
        }
      },
    };
    const { broker } = createHarness({ journalHost });
    const ac = new AbortController();
    const runPromise = broker.run({
      preferences: PREFS,
      signal: ac.signal,
      async start() {
        setTimeout(() => ac.abort(), 5);
        return {
          attemptId: 'att-abort',
          authorizeUrl: 'https://provider.example/authorize',
          expectedState: 'st-abort',
        };
      },
      async finish() {
        throw new Error('finish must not run after cancel');
      },
    });
    const res = await runPromise;
    assert.strictEqual(res.ok, false);
    assert.strictEqual(res.strategy, 'native_broker');
    assert.strictEqual(res.outcome, 'cancelled');
    assert(sent.some((m) => m.type === 'auth.cancel' && m.session_id === 'sess-abort'));
  });

  // Step 1 no_compatible_mode on mobile falls down to Step 5 paste fallback cleanly.
  await runCase('step 1 no_compatible_mode falls through to step 5 paste fallback', async () => {
    const journalHost = {
      version: 2,
      types: ['auth.prepare', 'auth.authorize', 'auth.cancel'],
      async send(msg) {
        return { type: 'auth.prepared', id: msg.id, outcome: 'no_compatible_mode' };
      },
    };
    const { broker, assignedLocations } = createHarness({
      journalHost,
      nativeV1Capability: true,
      hostname: '127.0.0.1',
    });
    const res = await broker.run({
      preferences: [{ mode: 'loopback', host: '127.0.0.1', path: '/auth/callback', port: 0 }],
      async start(ctx) {
        assert.strictEqual(ctx.strategy, 'paste_url');
        return {
          attemptId: 'att-paste-1',
          authorizeUrl: 'https://provider.example/authorize?state=st-p',
          expectedState: 'st-p',
        };
      },
      async finish(ctx) {
        assert.strictEqual(ctx.strategy, 'paste_url');
        assert.strictEqual(ctx.callbackUrl, 'http://127.0.0.1:9999/auth/callback?code=ok&state=st-p');
        return { pasted: true };
      },
      async onPasteFallback({ submitCallbackUrl, opened }) {
        assert.strictEqual(opened, true);
        await submitCallbackUrl('http://127.0.0.1:9999/auth/callback?code=ok&state=st-p');
      },
    });
    assert.strictEqual(res.ok, true);
    assert.strictEqual(res.strategy, 'paste_url');
    assert.deepStrictEqual(res.result, { pasted: true });
    assert.deepStrictEqual(assignedLocations, ['https://provider.example/authorize?state=st-p']);
    assert(res.skipped.some((s) => s.strategy === 'native_broker' && s.reason === 'no_compatible_mode'));
  });

  // Step 2: BYO HTTPS callback takes precedence over journal loopback when enabled.
  await runCase('step 2 BYO HTTPS callback runs when configured', async () => {
    const { broker, openedPopups } = createHarness({
      hostname: 'journal.owner.example',
      origin: 'https://journal.owner.example',
    });
    const res = await broker.run({
      byoHttps: true,
      async start(ctx) {
        assert.strictEqual(ctx.strategy, 'byo_https');
        return {
          attemptId: 'att-byo',
          authorizeUrl: 'https://provider.example/authorize?redirect_uri=https://journal.owner.example/cb',
        };
      },
      async waitForCompletion(ctx) {
        assert.strictEqual(ctx.strategy, 'byo_https');
        assert.strictEqual(ctx.opened, true);
        return { signedIn: true, via: 'byo_https' };
      },
      async finish() {
        throw new Error('finish not used by waitForCompletion');
      },
    });
    assert.strictEqual(res.ok, true);
    assert.strictEqual(res.strategy, 'byo_https');
    assert.deepStrictEqual(res.result, { signedIn: true, via: 'byo_https' });
    assert.strictEqual(openedPopups.length, 1);
    assert.strictEqual(
      openedPopups[0].location.href,
      'https://provider.example/authorize?redirect_uri=https://journal.owner.example/cb',
    );
  });

  // Step 3: Journal-direct loopback on same-machine plain browser.
  await runCase('step 3 journal-direct loopback runs on same-machine plain browser', async () => {
    const { broker, openedPopups } = createHarness({
      hostname: '127.0.0.1',
      origin: 'http://127.0.0.1:5015',
    });
    assert.strictEqual(broker.supportsNativeBroker(), false);
    assert.strictEqual(broker.isSameMachineBrowser(), true);
    const res = await broker.run({
      preferences: PREFS,
      async start(ctx) {
        assert.strictEqual(ctx.strategy, 'journal_loopback');
        return {
          attemptId: 'att-lb',
          authorizeUrl: 'https://provider.example/authorize?state=lb1',
        };
      },
      async waitForCompletion(ctx) {
        assert.strictEqual(ctx.strategy, 'journal_loopback');
        assert.strictEqual(ctx.opened, true);
        return { signedIn: true, via: 'loopback' };
      },
      async finish() {
        throw new Error('not called');
      },
    });
    assert.strictEqual(res.ok, true);
    assert.strictEqual(res.strategy, 'journal_loopback');
    assert.deepStrictEqual(res.result, { signedIn: true, via: 'loopback' });
    assert.strictEqual(openedPopups.length, 1);
    assert.strictEqual(openedPopups[0].location.href, 'https://provider.example/authorize?state=lb1');
  });

  // Step 4: Device Code on remote plain browser (skips native, BYO, and loopback).
  await runCase('step 4 device code runs on remote plain browser without opening a blank popup', async () => {
    const { broker, openedPopups } = createHarness({
      hostname: 'remote.lan',
      origin: 'https://remote.lan',
    });
    assert.strictEqual(broker.isSameMachineBrowser(), false);
    let shownCode = null;
    const res = await broker.run({
      preferences: PREFS,
      async start(ctx) {
        assert.strictEqual(ctx.strategy, 'device_code');
        return {
          attemptId: 'att-dev',
          userCode: 'ABCD-1234',
          verificationUri: 'https://provider.example/device',
          expiresIn: 600,
          interval: 5,
        };
      },
      async onDeviceCode(payload) {
        shownCode = payload.userCode;
        assert.strictEqual(payload.verificationUri, 'https://provider.example/device');
      },
      async waitForCompletion(ctx) {
        assert.strictEqual(ctx.strategy, 'device_code');
        return { signedIn: true, via: 'device_code' };
      },
      async finish() {
        throw new Error('not called');
      },
    });
    assert.strictEqual(res.ok, true);
    assert.strictEqual(res.strategy, 'device_code');
    assert.strictEqual(shownCode, 'ABCD-1234');
    assert.strictEqual(openedPopups.length, 0);
  });

  // Step 5: Remote plain browser when Device Code declines ({ available: false }) falls to Paste-URL.
  await runCase('step 5 paste-URL fallback runs when steps 1-4 are unavailable', async () => {
    const { broker } = createHarness({
      hostname: 'remote.lan',
      origin: 'https://remote.lan',
    });
    const res = await broker.run({
      preferences: PREFS,
      async start(ctx) {
        if (ctx.strategy === 'device_code') return { available: false };
        assert.strictEqual(ctx.strategy, 'paste_url');
        return {
          attemptId: 'att-p5',
          authorizeUrl: 'https://provider.example/authorize?state=s5',
          expectedState: 's5',
        };
      },
      async onDeviceCode() {
        throw new Error('must not be invoked when start returns available:false');
      },
      async onPasteFallback() {
        return { callbackUrl: 'http://127.0.0.1:45678/auth/callback?code=c5&state=s5' };
      },
      async finish(ctx) {
        assert.strictEqual(ctx.callbackUrl, 'http://127.0.0.1:45678/auth/callback?code=c5&state=s5');
        return { ok: 'done5' };
      },
    });
    assert.strictEqual(res.ok, true);
    assert.strictEqual(res.strategy, 'paste_url');
    assert.deepStrictEqual(res.result, { ok: 'done5' });
    assert.deepStrictEqual(
      Array.from(res.skipped, (s) => s.strategy),
      ['native_broker', 'byo_https', 'journal_loopback', 'device_code'],
    );
  });

  // Older native host returning { type: "unsupported" } falls cleanly down the ladder.
  await runCase('older native host returning unsupported falls through cleanly', async () => {
    const journalHost = {
      version: 2,
      types: ['auth.prepare', 'auth.authorize', 'auth.cancel'],
      async send(msg) {
        return { type: 'unsupported', id: msg.id };
      },
    };
    const { broker } = createHarness({ journalHost, nativeV1Capability: true });
    const res = await broker.run({
      preferences: PREFS,
      async start(ctx) {
        assert.strictEqual(ctx.strategy, 'paste_url');
        return { attemptId: 'att-old', authorizeUrl: 'https://provider.example/auth' };
      },
      async onPasteFallback({ submitCallbackUrl }) {
        await submitCallbackUrl('http://127.0.0.1:12345/auth/callback?code=old&state=x');
      },
      async finish() {
        return { migrated: true };
      },
    });
    assert.strictEqual(res.ok, true);
    assert.strictEqual(res.strategy, 'paste_url');
    assert(res.skipped.some((s) => s.strategy === 'native_broker' && s.reason === 'unsupported'));
  });

  // Verify shell.html loads oauth-broker.js in the boot partition so views can invoke it immediately.
  await runCase('shell.html includes oauth-broker.js in data-shell-part="boot"', async () => {
    const shellHtml = fs.readFileSync(path.join(staticDir, 'shell.html'), 'utf8');
    assert(
      /<script\b[^>]*\bsrc="\/static\/oauth-broker\.js"[^>]*\bdata-shell-part="boot"[^>]*><\/script>/.test(shellHtml),
      'shell.html must load /static/oauth-broker.js in data-shell-part="boot"',
    );
  });

  console.log(`DOM CASES: ${passed} passed`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
