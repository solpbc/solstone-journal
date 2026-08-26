// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

let cases = 0;

const REQUIRED_COPY_KEYS = [
  'DEVICE_EMPTY_TITLE', 'DEVICE_EMPTY_BODY', 'DEVICE_PAIR_CTA', 'MODAL_TITLE', 'STEP_1', 'STEP_2',
  'STEP_3', 'PAIR_NETWORK_LINE', 'DETAILS_DISCLOSURE', 'CA_FP_LABEL', 'CA_FP_NOTE',
  'PAIR_LINK_FIELD_LABEL', 'PAIR_LINK_COPY_LABEL', 'CHECK_PAIRING_CTA', 'PAIR_START_FAIL_BODY',
  'EXPIRED_BUTTON', 'WINDOW_CLOSED_BUTTON', 'SUCCESS_HEADING', 'SUCCESS_SUBHEAD',
  'SUCCESS_VERIFY_NOTE', 'SUCCESS_DONE', 'PAIR_LINK_COPY_SUCCESS_TOAST', 'PAIR_LINK_COPY_FAIL_TOAST',
  'DEVICE_LABEL_DEFAULT_FORMAT',
];

const MARKUP_COPY_KEYS = REQUIRED_COPY_KEYS.filter((key) => ![
  'DEVICE_EMPTY_TITLE', 'DEVICE_EMPTY_BODY', 'PAIR_LINK_COPY_SUCCESS_TOAST', 'PAIR_LINK_COPY_FAIL_TOAST',
  'DEVICE_LABEL_DEFAULT_FORMAT',
].includes(key));

function response(body, status = 200) {
  return {
    ok: status === 200,
    status,
    json: async () => body,
  };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function settle() {
  return new Promise((resolve) => setImmediate(resolve));
}

function renderEmptyStateHTML(workspace, copy) {
  const start = workspace.indexOf('function emptyStateHTML() {');
  const end = workspace.indexOf('\nfunction captureLabel', start);
  assert.notStrictEqual(start, -1);
  assert.notStrictEqual(end, -1);
  const context = vm.createContext({
    window: {
      LinkCopy: copy,
      NetworkRender: { resolve: (values, key) => values[key] || '' },
      SurfaceState: {
        empty: ({ heading, desc, action }) => `${heading}|${desc}|${action}`,
      },
    },
    escapeHtml: (value) => String(value),
  });
  vm.runInContext(workspace.slice(start, end), context, { filename: 'workspace-empty-state.js' });
  return context.emptyStateHTML();
}

class ClassList {
  constructor(element) {
    this.element = element;
    this.values = new Set();
  }

  add(...names) {
    names.forEach((name) => this.values.add(name));
  }

  remove(...names) {
    names.forEach((name) => this.values.delete(name));
  }

  contains(name) {
    return this.values.has(name);
  }
}

class Element {
  constructor(document, tagName = 'div', id = '') {
    this.ownerDocument = document;
    this.tagName = tagName.toUpperCase();
    this.id = id;
    this.dataset = {};
    this.attributes = {};
    this.children = [];
    this.parentElement = null;
    this.listeners = {};
    this.style = {};
    this.classList = new ClassList(this);
    this.textContent = '';
    this._innerHTML = '';
    this._hidden = false;
    this.inert = false;
    this.disabled = false;
  }

  get hidden() {
    return this._hidden;
  }

  set hidden(value) {
    this._hidden = Boolean(value);
    if (this._hidden) this.attributes.hidden = '';
    else delete this.attributes.hidden;
    this.ownerDocument?.notifyMutation();
  }

  get isConnected() {
    return this.ownerDocument?.body?.contains(this) || false;
  }

  get innerHTML() {
    return this._innerHTML;
  }

  set innerHTML(value) {
    this._innerHTML = String(value);
    this.children = [];
    if (this._innerHTML.includes('<svg')) this.appendChild(new Element(this.ownerDocument, 'svg'));
    this.ownerDocument?.notifyMutation();
  }

  append(...children) {
    children.forEach((child) => this.appendChild(child));
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    this.ownerDocument?.notifyMutation();
    return child;
  }

  removeChild(child) {
    this.children = this.children.filter((candidate) => candidate !== child);
    child.parentElement = null;
    this.ownerDocument?.notifyMutation();
  }

  addEventListener(name, listener) {
    (this.listeners[name] ||= []).push(listener);
  }

  emit(name, event = {}) {
    for (const listener of this.listeners[name] || []) {
      listener({
        target: this,
        preventDefault() {},
        stopPropagation() {},
        ...event,
      });
    }
  }

  focus() {
    this.ownerDocument.activeElement = this;
    this.ownerDocument.emit('focusin', { target: this });
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
    if (name === 'hidden') this._hidden = true;
    if (!name.startsWith('data-convey-') && name !== 'inert' && name !== 'tabindex') {
      this.ownerDocument?.notifyMutation();
    }
  }

  removeAttribute(name) {
    delete this.attributes[name];
    if (name === 'hidden') this._hidden = false;
    if (!name.startsWith('data-convey-') && name !== 'inert' && name !== 'tabindex') {
      this.ownerDocument?.notifyMutation();
    }
  }

  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  hasAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name);
  }

  contains(node) {
    return node === this || this.children.some((child) => child.contains(node));
  }

  matches(selector) {
    if (selector === '.report-error-modal') return this.classList.contains('report-error-modal');
    if (selector === '[role="dialog"][aria-modal="true"]') {
      return this.getAttribute('role') === 'dialog' && this.getAttribute('aria-modal') === 'true';
    }
    if (selector === '[data-pairing-action]') return Object.hasOwn(this.dataset, 'pairingAction');
    if (selector === '[data-pairing-action="open"]') return this.dataset.pairingAction === 'open';
    if (selector === '[tabindex]') return this.hasAttribute('tabindex');
    if (selector === '[contenteditable="true"]') return this.getAttribute('contenteditable') === 'true';
    if (selector === '[contenteditable=""]') return this.getAttribute('contenteditable') === '';
    if (selector === 'a[href]') return this.tagName === 'A' && this.hasAttribute('href');
    if (selector === 'area[href]') return this.tagName === 'AREA' && this.hasAttribute('href');
    return selector.toUpperCase() === this.tagName;
  }

  closest(selector) {
    for (let node = this; node; node = node.parentElement) {
      if (node.matches(selector)) return node;
    }
    return null;
  }

  querySelectorAll(selector) {
    const selectors = selector.split(',').map((part) => part.trim());
    const matches = [];
    const visit = (node) => {
      node.children.forEach((child) => {
        if (selectors.some((part) => child.matches(part))) matches.push(child);
        visit(child);
      });
    };
    visit(this);
    return matches;
  }

  querySelector(selector) {
    if (selector.startsWith('#')) return this.ownerDocument.getElementById(selector.slice(1));
    return this.querySelectorAll(selector)[0] || null;
  }
}

class MutationObserverShim {
  constructor(callback) {
    this.callback = callback;
  }

  observe(documentBody) {
    documentBody.ownerDocument.observers.push(this);
  }

  disconnect() {}
}

function createEnvironment(manifestDir, options = {}) {
  const nodes = new Map();
  const documentListeners = {};
  const document = {
    activeElement: null,
    observers: [],
    body: null,
    getElementById(id) { return nodes.get(id) || null; },
    createElement(tagName) { return new Element(document, tagName); },
    addEventListener(name, listener) { (documentListeners[name] ||= []).push(listener); },
    emit(name, event = {}) {
      for (const listener of documentListeners[name] || []) {
        listener({
          preventDefault() { this.defaultPrevented = true; },
          stopPropagation() { this.propagationStopped = true; },
          target: document,
          ...event,
        });
      }
    },
    notifyMutation() {
      this.observers.forEach((observer) => observer.callback([]));
    },
  };
  const body = new Element(document, 'body');
  document.body = body;
  const make = (id, tagName = 'div') => {
    const element = new Element(document, tagName, id);
    nodes.set(id, element);
    return element;
  };

  const root = make('link-workspace-root', 'main');
  root.setAttribute('tabindex', '-1');
  body.appendChild(root);
  const opener = make('link-empty-pairing-action', 'button');
  opener.dataset.pairingAction = 'open';
  root.appendChild(opener);
  const dialog = make('link-pairing-dialog');
  dialog.setAttribute('role', 'dialog');
  dialog.setAttribute('aria-modal', 'true');
  dialog.hidden = true;
  root.appendChild(dialog);
  const close = make('link-pairing-close', 'button');
  close.dataset.pairingAction = 'close';
  dialog.appendChild(close);

  const starting = make('link-pairing-starting');
  const material = make('link-pairing-material');
  const expired = make('link-pairing-expired');
  const unavailable = make('link-pairing-unavailable');
  const windowClosed = make('link-pairing-window-closed');
  const complete = make('link-pairing-complete');
  dialog.append(starting, material, expired, unavailable, windowClosed, complete);
  const labelRow = make('link-pairing-label-row');
  const label = make('link-device-label', 'span');
  labelRow.appendChild(label);
  const networkLine = make('link-pairing-network-line', 'p');
  const fingerprint = make('link-pairing-fingerprint', 'code');
  const linkValue = make('link-pairing-link-value', 'code');
  const qr = make('link-pairing-qr');
  const copyButton = make('link-pairing-copy', 'button');
  copyButton.dataset.pairingAction = 'copy';
  const check = make('link-pairing-check', 'button');
  check.dataset.pairingAction = 'check';
  const materialClose = make('link-pairing-material-close', 'button');
  materialClose.dataset.pairingAction = 'close';
  material.append(labelRow, networkLine, fingerprint, linkValue, qr, copyButton, check, materialClose);
  const expiredButton = make('link-pairing-expired-action', 'button');
  expiredButton.dataset.pairingAction = 'regenerate';
  expired.appendChild(expiredButton);
  const retryButton = make('link-pairing-retry', 'button');
  retryButton.dataset.pairingAction = 'regenerate';
  unavailable.appendChild(retryButton);
  const closedButton = make('link-pairing-window-action', 'button');
  closedButton.dataset.pairingAction = 'regenerate';
  windowClosed.appendChild(closedButton);
  const successHeading = make('link-pairing-success-heading', 'h3');
  const successSubhead = make('link-pairing-success-subhead', 'p');
  const completeClose = make('link-pairing-complete-close', 'button');
  completeClose.dataset.pairingAction = 'close';
  complete.append(successHeading, successSubhead, completeClose);

  const timerQueue = new Map();
  let timerId = 0;
  const requests = [];
  const fetchQueue = [];
  const linkListeners = new Set();
  const clipboardWrites = [];
  const toasts = [];
  const window = {
    document,
    location: { pathname: options.pathname || '/app/network/workspace' },
    LinkCopy: {
      ...Object.fromEntries(REQUIRED_COPY_KEYS.map((key) => [key, `copy-${key}`])),
      PAIR_NETWORK_LINE: 'network {time}',
      SUCCESS_HEADING: 'heading {label}',
      SUCCESS_SUBHEAD: 'subhead {short_fp}',
      DEVICE_LABEL_DEFAULT_FORMAT: 'label-{month}-{day}',
    },
    appEvents: {
      listen(tract, callback) {
        assert.strictEqual(tract, 'link');
        linkListeners.add(callback);
        return () => linkListeners.delete(callback);
      },
    },
    navigator: { clipboard: { writeText: async () => {} } },
    qrcode: options.qrcode === false ? undefined : () => ({
      addData() {},
      make() {},
      createSvgTag() { return '<svg></svg>'; },
      getModuleCount() { return 23; },
    }),
    fetch(url, request) {
      requests.push({ url, request });
      const next = fetchQueue.shift();
      if (!next) return Promise.reject(new Error('unexpected fetch'));
      return next.promise || Promise.resolve(next);
    },
    clearTimeout(id) { timerQueue.delete(id); },
    setTimeout(callback, delay) {
      const id = ++timerId;
      timerQueue.set(id, { callback, delay });
      return id;
    },
    requestAnimationFrame(callback) { callback(); },
    getComputedStyle(element) {
      return {
        display: element.hidden ? 'none' : (element.style.display || 'block'),
        position: element === dialog ? 'fixed' : 'static',
      };
    },
    whenShellReady(callback) { callback(); },
  };
  window.window = window;
  const context = vm.createContext({
    window,
    document,
    MutationObserver: MutationObserverShim,
    module: { exports: {} },
    console,
    encodeURIComponent,
    setImmediate,
  });
  const modalSource = fs.readFileSync(path.join(manifestDir, 'assets/static/modal_layer.js'), 'utf8');
  const networkSource = fs.readFileSync(path.join(manifestDir, 'assets/network/network.js'), 'utf8');
  vm.runInContext(modalSource, context, { filename: 'modal_layer.js' });
  vm.runInContext(networkSource, context, { filename: 'network.js' });
  const controller = window.NetworkRender.initPairingCeremony({
    clipboardWriteText: options.clipboardWriteText || (async (value) => {
      clipboardWrites.push(value);
      return options.clipboardResult !== false;
    }),
    showToast: options.showToast || ((message) => toasts.push(message)),
  });

  function click(target) {
    root.emit('click', { target });
  }

  function emitLink() {
    linkListeners.forEach((listener) => listener({ event: 'pair_complete' }));
  }

  return {
    controller,
    dialog,
    document,
    nodes,
    root,
    opener,
    requests,
    fetchQueue,
    clipboardWrites,
    toasts,
    emitLink,
    click,
    timers: timerQueue,
    window,
  };
}

function material(overrides = {}) {
  return {
    nonce: 'nonce-a',
    pair_link: 'https://go.example/p#ALPHANUMERIC',
    expires_in: 125,
    device_label: 'sample device',
    ca_fingerprint: 'abcdef0123456789',
    ...overrides,
  };
}

async function testCase(_name, callback) {
  await callback();
  cases += 1;
}

async function main() {
  const manifestDir = process.argv[2];
  if (!manifestDir) throw new Error('manifest directory required');
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/network/workspace.html'), 'utf8');
  const activeAsset = path.join(manifestDir, 'assets/network/network.js');
  const retiredAsset = path.join(manifestDir, '../solstone-core-sol-link/assets/init.html');

  await testCase('successful start renders QR, link, read-only label, and fingerprint', async () => {
    const env = createEnvironment(manifestDir);
    env.fetchQueue.push(response(material()));
    env.opener.focus();
    env.click(env.opener);
    await settle();
    assert.strictEqual(env.dialog.hidden, false);
    assert.strictEqual(env.nodes.get('link-pairing-material').hidden, false);
    assert.strictEqual(env.nodes.get('link-pairing-network-line').textContent, 'network 2:05');
    assert.strictEqual(env.nodes.get('link-device-label').textContent, 'sample device');
    assert.strictEqual(env.nodes.get('link-pairing-fingerprint').textContent, 'abcdef0123456789');
    assert.strictEqual(env.nodes.get('link-pairing-link-value').textContent, 'https://go.example/p#ALPHANUMERIC');
    assert.strictEqual(env.nodes.get('link-pairing-qr').querySelector('svg').getAttribute('data-module-count'), '23');
    assert.strictEqual(env.nodes.get('link-device-label').tagName, 'SPAN');
    assert.ok(workspace.includes('<span id="link-device-label"'));
    assert.strictEqual(env.requests[0].url, '/app/network/pair-start');
  });

  await testCase('pair start sends a non-empty default device label', async () => {
    const env = createEnvironment(manifestDir);
    const started = deferred();
    env.fetchQueue.push(started);
    env.click(env.opener);
    const body = JSON.parse(env.requests[0].request.body);
    assert.strictEqual(typeof body.device_label, 'string');
    assert.ok(body.device_label);
    assert.match(body.device_label, /^label-[a-z]{3}-\d{1,2}$/);
    started.resolve(response(material({ device_label: body.device_label })));
    await settle();
  });

  await testCase('completion uses the non-empty label echoed by pair start', async () => {
    const env = createEnvironment(manifestDir);
    const started = deferred();
    env.fetchQueue.push(started);
    env.click(env.opener);
    const label = JSON.parse(env.requests[0].request.body).device_label;
    started.resolve(response(material({ device_label: label })));
    await settle();
    env.fetchQueue.push(response({ present: true, used: true }));
    env.emitLink();
    await settle();
    assert.ok(env.nodes.get('link-pairing-success-heading').textContent);
    assert.ok(env.nodes.get('link-pairing-success-heading').textContent.includes(label));
  });

  await testCase('expires_in 125 formats as 2:05 and arms a 125-second timer', async () => {
    const env = createEnvironment(manifestDir);
    env.fetchQueue.push(response(material()));
    env.click(env.opener);
    await settle();
    assert.strictEqual(env.nodes.get('link-pairing-network-line').textContent, 'network 2:05');
    assert.deepStrictEqual([...env.timers.values()].map(({ delay }) => delay), [125 * 1000]);
  });

  await testCase('relay refusal has no material or expiry', async () => {
    const env = createEnvironment(manifestDir);
    env.fetchQueue.push(response({ reason_code: 'relay_pairing_unavailable' }, 503));
    env.click(env.opener);
    await settle();
    assert.strictEqual(env.nodes.get('link-pairing-unavailable').hidden, false);
    assert.strictEqual(env.nodes.get('link-pairing-check').hidden, true);
    assert.strictEqual(env.timers.size, 0);
  });

  await testCase('notification verifies the nonce status before completing', async () => {
    const env = createEnvironment(manifestDir);
    env.fetchQueue.push(response(material()));
    env.click(env.opener);
    await settle();
    env.fetchQueue.push(response({ present: true, used: true }));
    env.emitLink();
    await settle();
    assert.strictEqual(env.requests[1].url, '/app/network/api/pair/nonce-status?nonce=nonce-a');
    assert.strictEqual(env.nodes.get('link-pairing-complete').hidden, false);
    assert.strictEqual(env.nodes.get('link-pairing-success-subhead').textContent, 'subhead abcdef0123456789');
  });

  await testCase('an unrelated link event leaves an open current nonce incomplete', async () => {
    const env = createEnvironment(manifestDir);
    env.fetchQueue.push(response(material()));
    env.click(env.opener);
    await settle();
    env.fetchQueue.push(response({ present: true, used: false }));
    env.emitLink();
    await settle();
    assert.strictEqual(env.requests[1].url, '/app/network/api/pair/nonce-status?nonce=nonce-a');
    assert.strictEqual(env.nodes.get('link-pairing-material').hidden, false);
    assert.strictEqual(env.nodes.get('link-pairing-complete').hidden, true);
  });

  await testCase('manual check reports busy and distinguishes used open missing and failed status', async () => {
    const pending = createEnvironment(manifestDir);
    pending.fetchQueue.push(response(material()));
    pending.click(pending.opener);
    await settle();
    const nonceStatus = deferred();
    pending.fetchQueue.push(nonceStatus);
    pending.click(pending.nodes.get('link-pairing-check'));
    assert.strictEqual(pending.nodes.get('link-pairing-check').disabled, true);
    assert.strictEqual(pending.nodes.get('link-pairing-check').getAttribute('aria-busy'), 'true');
    nonceStatus.resolve(response({ present: true, used: false }));
    await settle();
    assert.strictEqual(pending.nodes.get('link-pairing-check').disabled, false);
    assert.strictEqual(pending.nodes.get('link-pairing-check').getAttribute('aria-busy'), null);

    const check = async (status, expectedId) => {
      const env = createEnvironment(manifestDir);
      env.fetchQueue.push(response(material()));
      env.click(env.opener);
      await settle();
      const failedRequest = status instanceof Error ? deferred() : null;
      env.fetchQueue.push(failedRequest || status);
      env.click(env.nodes.get('link-pairing-check'));
      if (failedRequest) failedRequest.reject(status);
      await settle();
      assert.strictEqual(env.nodes.get(expectedId).hidden, false);
    };
    await check(response({ present: true, used: true }), 'link-pairing-complete');
    await check(response({ present: true, used: false }), 'link-pairing-material');
    await check(response({ present: false, used: false }), 'link-pairing-window-closed');
    await check(new Error('offline'), 'link-pairing-unavailable');
  });

  await testCase('malformed material is recoverable without a status request', async () => {
    for (const bad of [
      null,
      [],
      material({ nonce: undefined }),
      material({ nonce: '   ' }),
      material({ nonce: 1 }),
      material({ pair_link: undefined }),
      material({ pair_link: '' }),
      material({ pair_link: 1 }),
      material({ ca_fingerprint: undefined }),
      material({ ca_fingerprint: '' }),
      material({ ca_fingerprint: 1 }),
      material({ expires_in: undefined }),
      material({ expires_in: 0 }),
      material({ expires_in: '125' }),
      material({ device_label: 1 }),
    ]) {
      const env = createEnvironment(manifestDir);
      env.fetchQueue.push(response(bad));
      env.click(env.opener);
      await settle();
      assert.strictEqual(env.nodes.get('link-pairing-unavailable').hidden, false);
      assert.strictEqual(env.requests.length, 1);
      assert.strictEqual(env.nodes.get('link-pairing-check').hidden, true);
    }
    const withoutQr = createEnvironment(manifestDir, { qrcode: false });
    withoutQr.fetchQueue.push(response(material()));
    withoutQr.click(withoutQr.opener);
    await settle();
    assert.strictEqual(withoutQr.nodes.get('link-pairing-unavailable').hidden, false);
  });

  await testCase('stale responses cannot update after close or regeneration', async () => {
    const closed = createEnvironment(manifestDir);
    const first = deferred();
    closed.fetchQueue.push(first);
    closed.click(closed.opener);
    closed.controller.close();
    first.resolve(response(material()));
    await settle();
    assert.strictEqual(closed.dialog.hidden, true);
    assert.strictEqual(closed.nodes.get('link-pairing-material').hidden, true);

    const renewed = createEnvironment(manifestDir);
    const stale = deferred();
    renewed.fetchQueue.push(stale);
    renewed.click(renewed.opener);
    const current = deferred();
    renewed.fetchQueue.push(current);
    renewed.click(renewed.nodes.get('link-pairing-retry'));
    stale.resolve(response(material({ pair_link: 'https://old.example/p#OLD' })));
    current.resolve(response(material({ pair_link: 'https://new.example/p#NEW' })));
    await settle();
    await settle();
    assert.strictEqual(renewed.nodes.get('link-pairing-link-value').textContent, 'https://new.example/p#NEW');
    const nonceClosed = createEnvironment(manifestDir);
    nonceClosed.fetchQueue.push(response(material()));
    nonceClosed.click(nonceClosed.opener);
    await settle();
    const staleAfterClose = deferred();
    nonceClosed.fetchQueue.push(staleAfterClose);
    nonceClosed.click(nonceClosed.nodes.get('link-pairing-check'));
    nonceClosed.controller.close();
    staleAfterClose.resolve(response({ present: true, used: true }));
    await settle();
    assert.strictEqual(nonceClosed.dialog.hidden, true);
    assert.strictEqual(nonceClosed.nodes.get('link-pairing-complete').hidden, true);

    const nonceRenewed = createEnvironment(manifestDir);
    nonceRenewed.fetchQueue.push(response(material()));
    nonceRenewed.click(nonceRenewed.opener);
    await settle();
    const staleAfterRenewal = deferred();
    nonceRenewed.fetchQueue.push(staleAfterRenewal);
    nonceRenewed.click(nonceRenewed.nodes.get('link-pairing-check'));
    const expiry = [...nonceRenewed.timers.values()][0];
    expiry.callback();
    nonceRenewed.fetchQueue.push(response(material({ nonce: 'nonce-next', pair_link: 'https://new.example/p#NEW' })));
    nonceRenewed.click(nonceRenewed.nodes.get('link-pairing-expired-action'));
    await settle();
    staleAfterRenewal.resolve(response({ present: true, used: true }));
    await settle();
    assert.strictEqual(nonceRenewed.nodes.get('link-pairing-material').hidden, false);
    assert.strictEqual(nonceRenewed.nodes.get('link-pairing-link-value').textContent, 'https://new.example/p#NEW');
  });

  await testCase('keyboard operation traps Tab, Escape and cancel restore focus after a re-render', async () => {
    const env = createEnvironment(manifestDir);
    env.fetchQueue.push(response(material()));
    env.opener.focus();
    env.click(env.opener);
    await settle();
    const buttons = env.dialog.querySelectorAll('button');
    const lastVisible = env.nodes.get('link-pairing-material-close');
    assert.strictEqual(env.document.activeElement, buttons[0]);
    lastVisible.focus();
    let prevented = false;
    env.document.emit('keydown', { key: 'Tab', preventDefault() { prevented = true; }, stopPropagation() {} });
    assert.strictEqual(prevented, true);
    assert.strictEqual(env.document.activeElement, buttons[0]);
    env.document.emit('keydown', { key: 'Tab', shiftKey: true, preventDefault() {}, stopPropagation() {} });
    assert.strictEqual(env.document.activeElement, lastVisible);
    env.document.emit('keydown', { key: 'Escape', preventDefault() {}, stopPropagation() {} });
    assert.strictEqual(env.dialog.hidden, true);
    assert.strictEqual(env.document.activeElement, env.opener);
    env.fetchQueue.push(response(material()));
    env.click(env.opener);
    await settle();
    env.click(env.nodes.get('link-pairing-material-close'));
    assert.strictEqual(env.dialog.hidden, true);
    assert.strictEqual(env.document.activeElement, env.opener);
    env.fetchQueue.push(response(material()));
    env.click(env.opener);
    await settle();
    env.root.removeChild(env.opener);
    env.controller.close();
    assert.strictEqual(env.document.activeElement, env.root);
  });

  await testCase('copy link uses the shared clipboard helper and reports both outcomes', async () => {
    const success = createEnvironment(manifestDir);
    success.fetchQueue.push(response(material()));
    success.click(success.opener);
    await settle();
    success.click(success.nodes.get('link-pairing-copy'));
    await settle();
    assert.deepStrictEqual(success.clipboardWrites, ['https://go.example/p#ALPHANUMERIC']);
    assert.deepStrictEqual(success.toasts, [success.window.LinkCopy.PAIR_LINK_COPY_SUCCESS_TOAST]);

    const failure = createEnvironment(manifestDir, { clipboardResult: false });
    failure.fetchQueue.push(response(material()));
    failure.click(failure.opener);
    await settle();
    failure.click(failure.nodes.get('link-pairing-copy'));
    await settle();
    assert.deepStrictEqual(failure.clipboardWrites, ['https://go.example/p#ALPHANUMERIC']);
    assert.deepStrictEqual(failure.toasts, [failure.window.LinkCopy.PAIR_LINK_COPY_FAIL_TOAST]);
  });

  await testCase('copy bindings are keys with non-empty values, not owner prose', async () => {
    const env = createEnvironment(manifestDir);
    for (const key of REQUIRED_COPY_KEYS) {
      assert.ok(typeof env.window.LinkCopy[key] === 'string' && env.window.LinkCopy[key]);
    }
    for (const key of MARKUP_COPY_KEYS) assert.ok(workspace.includes(`data-copy="${key}"`));
    const emptyState = renderEmptyStateHTML(workspace, env.window.LinkCopy);
    assert.ok(emptyState.includes(env.window.LinkCopy.DEVICE_EMPTY_TITLE));
    assert.ok(emptyState.includes(env.window.LinkCopy.DEVICE_EMPTY_BODY));
  });

  await testCase('active asset pin distinguishes the network ceremony from retired link init', async () => {
    assert.ok(activeAsset.endsWith('/assets/network/network.js'));
    assert.ok(workspace.includes('/app/network/static/network.js'));
    assert.ok(fs.readFileSync(activeAsset, 'utf8').includes('initPairingCeremony'));
    assert.ok(!fs.readFileSync(retiredAsset, 'utf8').includes('initPairingCeremony'));
    assert.ok(workspace.includes('role="dialog" aria-modal="true"'));
    assert.ok(workspace.includes('id="link-device-label"'));
  });

  console.log(`DOM CASES: ${cases} passed`);
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
