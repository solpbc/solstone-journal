// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const vm = require('vm');

const modalLayerPath = process.argv[2];
const caseName = process.argv[3];

// Focusability the browser grants without an author-supplied tabindex.
const NATIVELY_FOCUSABLE_TAGS = new Set([
  'BUTTON',
  'INPUT',
  'SELECT',
  'TEXTAREA',
  'SUMMARY',
  'IFRAME',
]);

class FakeClassList {
  constructor(element) {
    this.element = element;
    this.values = new Set();
  }

  add(...names) {
    let changed = false;
    names.forEach((name) => {
      if (name && !this.values.has(String(name))) {
        this.values.add(String(name));
        changed = true;
      }
    });
    if (changed) this.element._syncClassAttribute();
  }

  remove(...names) {
    let changed = false;
    names.forEach((name) => {
      if (this.values.delete(String(name))) {
        changed = true;
      }
    });
    if (changed) this.element._syncClassAttribute();
  }

  contains(name) {
    return this.values.has(String(name));
  }

  toggle(name, force) {
    if (force === true) {
      this.add(name);
      return true;
    }
    if (force === false) {
      this.remove(name);
      return false;
    }
    if (this.contains(name)) {
      this.remove(name);
      return false;
    }
    this.add(name);
    return true;
  }

  setFromString(value) {
    this.values = new Set(String(value || '').split(/\s+/).filter(Boolean));
  }

  toString() {
    return Array.from(this.values).join(' ');
  }
}

class FakeElement {
  constructor(document, tagName = 'div') {
    this.ownerDocument = document;
    this.nodeType = 1;
    this.tagName = String(tagName).toUpperCase();
    this.attributes = {};
    this.children = [];
    this.parentElement = null;
    this.style = {};
    this.hidden = false;
    this.inert = false;
    this.disabled = false;
    this.listeners = {};
    this.classList = new FakeClassList(this);
  }

  get id() {
    return this.attributes.id || '';
  }

  set id(value) {
    this.setAttribute('id', value);
  }

  get className() {
    return this.classList.toString();
  }

  set className(value) {
    this.classList.setFromString(value);
    this._syncClassAttribute();
  }

  get isConnected() {
    for (let node = this; node; node = node.parentElement) {
      if (node === this.ownerDocument.body) return true;
    }
    return false;
  }

  _syncClassAttribute() {
    const value = this.classList.toString();
    if (value) {
      this.attributes.class = value;
    } else {
      delete this.attributes.class;
    }
    this.ownerDocument._notifyMutation('class');
  }

  appendChild(child) {
    if (child.parentElement) {
      child.parentElement.removeChild(child);
    }
    this.children.push(child);
    child.parentElement = this;
    this.ownerDocument._notifyMutation();
    return child;
  }

  removeChild(child) {
    const index = this.children.indexOf(child);
    assert(index !== -1, 'child is not attached to parent');
    const active = this.ownerDocument.activeElement;
    this.children.splice(index, 1);
    child.parentElement = null;
    if (active && child.contains(active)) {
      this.ownerDocument.activeElement = null;
    }
    this.ownerDocument._notifyMutation();
    return child;
  }

  setAttribute(name, value) {
    const attr = String(name);
    this.attributes[attr] = String(value);
    if (attr === 'hidden') this.hidden = true;
    if (attr === 'inert') this.inert = true;
    if (attr === 'class') this.classList.setFromString(value);
    this.ownerDocument._notifyMutation(attr);
  }

  removeAttribute(name) {
    const attr = String(name);
    delete this.attributes[attr];
    if (attr === 'hidden') this.hidden = false;
    if (attr === 'inert') this.inert = false;
    if (attr === 'class') this.classList.setFromString('');
    this.ownerDocument._notifyMutation(attr);
  }

  hasAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, String(name));
  }

  getAttribute(name) {
    const attr = String(name);
    return this.hasAttribute(attr) ? this.attributes[attr] : null;
  }

  addEventListener(type, listener) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push(listener);
  }

  dispatchEvent(event) {
    const listeners = this.listeners[event.type] || [];
    listeners.forEach((listener) => listener(event));
  }

  contains(target) {
    for (let node = target; node; node = node.parentElement) {
      if (node === this) return true;
    }
    return false;
  }

  isFocusable() {
    if (this.disabled) return false;
    if (this.hasAttribute('tabindex')) return true;
    if (this.hasAttribute('contenteditable')) return true;
    if (this.tagName === 'A' || this.tagName === 'AREA') return this.hasAttribute('href');
    return NATIVELY_FOCUSABLE_TAGS.has(this.tagName);
  }

  focus() {
    if (!this.isFocusable()) return;
    if (this.ownerDocument.activeElement === this) return;
    this.ownerDocument.activeElement = this;
    this.ownerDocument.focusLog.push(this);
    this.ownerDocument.dispatchEvent(makeEvent('focusin', { target: this }));
  }

  matches(selector) {
    return selector.split(',').some((part) => this._matchesSingle(part.trim()));
  }

  _matchesSingle(selector) {
    if (!selector) return false;
    if (selector === '[role="dialog"][aria-modal="true"]') {
      return this.getAttribute('role') === 'dialog'
        && this.getAttribute('aria-modal') === 'true';
    }
    if (selector === '.report-error-modal') {
      return this.classList.contains('report-error-modal');
    }
    if (selector.startsWith('#')) {
      return this.id === selector.slice(1);
    }
    if (selector.startsWith('.')) {
      return this.classList.contains(selector.slice(1));
    }
    if (selector === '[tabindex]') {
      return this.hasAttribute('tabindex');
    }
    if (selector === '[contenteditable="true"]') {
      return this.getAttribute('contenteditable') === 'true';
    }
    if (selector === '[contenteditable=""]') {
      return this.getAttribute('contenteditable') === '';
    }
    if (selector === 'a[href]' || selector === 'area[href]') {
      return this.tagName.toLowerCase() === selector.slice(0, selector.indexOf('['))
        && this.hasAttribute('href');
    }
    return this.tagName.toLowerCase() === selector;
  }

  querySelectorAll(selector) {
    const results = [];
    const visit = (node) => {
      node.children.forEach((child) => {
        if (child.matches(selector)) {
          results.push(child);
        }
        visit(child);
      });
    };
    visit(this);
    return results;
  }
}

class FakeDocument {
  constructor() {
    this.listeners = {};
    this.readyState = 'complete';
    this.activeElement = null;
    this.observers = [];
    this.focusLog = [];
    this.body = new FakeElement(this, 'body');
  }

  createElement(tagName) {
    return new FakeElement(this, tagName);
  }

  addEventListener(type, listener, useCapture) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push({ listener, capture: Boolean(useCapture) });
  }

  // document sits in both the capture and the bubble path of a descendant
  // dispatch, so its capture listeners run before any of its bubble listeners.
  dispatchEvent(event) {
    const entries = this.listeners[event.type] || [];
    const phases = [
      entries.filter((entry) => entry.capture),
      entries.filter((entry) => !entry.capture),
    ];
    phases.forEach((phase) => {
      if (event.propagationStopped) return;
      phase.forEach((entry) => {
        if (event.immediatePropagationStopped) return;
        entry.listener(event);
      });
    });
    return event;
  }

  listenerCount(type) {
    return (this.listeners[type] || []).length;
  }

  _notifyMutation(attributeName = null) {
    this.observers.forEach((observer) => {
      const filter = observer.options.attributeFilter || null;
      if (attributeName && filter && !filter.includes(attributeName)) return;
      observer.callback();
    });
  }
}

class FakeMutationObserver {
  constructor(callback) {
    this.callback = callback;
  }

  observe(target, options) {
    target.ownerDocument.observers.push({ callback: this.callback, target, options });
  }
}

function makeContext() {
  const document = new FakeDocument();
  const frames = [];
  const context = {
    console,
    document,
    MutationObserver: FakeMutationObserver,
    requestAnimationFrame(callback) {
      frames.push(callback);
      return frames.length;
    },
    getComputedStyle(element) {
      return {
        display: computedDisplay(element),
        position: computedPosition(element),
      };
    },
  };
  context.window = context;
  context.globalThis = context;
  context.whenShellReady = (callback) => callback({});
  context.flushFrames = function flushFrames() {
    while (frames.length) {
      const frame = frames.shift();
      frame();
    }
  };
  vm.createContext(context);
  vm.runInContext(fs.readFileSync(modalLayerPath, 'utf8'), context, {
    filename: modalLayerPath,
  });
  return context;
}

function computedDisplay(element) {
  if (element.style.display) return element.style.display;
  if (element.classList.contains('modal-backdrop')) {
    return element.classList.contains('show') ? 'flex' : 'none';
  }
  if (element.classList.contains('color-modal')) return 'none';
  if (element.classList.contains('modal')) return 'none';
  if (element.id === 'trDeleteSegmentModal') return 'none';
  return 'block';
}

function computedPosition(element) {
  if (element.classList.contains('position-relative')) {
    return 'relative';
  }
  if (
    element.classList.contains('link-modal')
    || element.classList.contains('modal')
    || element.classList.contains('modal-backdrop')
    || element.classList.contains('color-modal')
    || element.classList.contains('tr-screenshot-modal')
    || element.classList.contains('fixed-dialog')
    || element.classList.contains('spk-who-backdrop')
    || element.id === 'trDeleteSegmentModal'
  ) {
    return 'fixed';
  }
  return 'static';
}

function makeEvent(type, props = {}) {
  return {
    type,
    ...props,
    defaultPrevented: false,
    propagationStopped: false,
    immediatePropagationStopped: false,
    preventDefault() {
      this.defaultPrevented = true;
    },
    stopPropagation() {
      this.propagationStopped = true;
    },
    stopImmediatePropagation() {
      this.propagationStopped = true;
      this.immediatePropagationStopped = true;
    },
  };
}

function el(document, tagName, attrs = {}) {
  const element = document.createElement(tagName);
  Object.entries(attrs).forEach(([name, value]) => {
    if (name === 'class') {
      element.className = value;
    } else if (name === 'hidden') {
      if (value) element.setAttribute('hidden', '');
    } else if (name === 'disabled') {
      element.disabled = Boolean(value);
    } else if (name === 'styleDisplay') {
      element.style.display = value;
    } else {
      element.setAttribute(name, value);
    }
  });
  return element;
}

function dialog(document, attrs = {}) {
  return el(document, 'div', {
    role: 'dialog',
    'aria-modal': 'true',
    ...attrs,
  });
}

function shell(document) {
  const skip = el(document, 'a', { id: 'skip-link', href: '#main-content' });
  const menu = el(document, 'nav', { id: 'menu-bar' });
  const chrome = el(document, 'nav', { id: 'shell-chrome' });
  const notifications = el(document, 'div', { id: 'notification-center' });
  const workspace = el(document, 'main', { id: 'main-content', class: 'workspace' });
  const report = dialog(document, { class: 'modal report-error-modal', styleDisplay: 'block' });
  document.body.appendChild(skip);
  document.body.appendChild(menu);
  document.body.appendChild(chrome);
  document.body.appendChild(notifications);
  document.body.appendChild(workspace);
  document.body.appendChild(report);
  return { skip, menu, chrome, notifications, workspace, report };
}

function makeWorkspaceShape(context, kind) {
  const document = context.document;
  const base = shell(document);
  const root = el(document, 'section', { id: `${kind}-root` });
  const panel = el(document, 'section', { id: `${kind}-panel` });
  const backgroundButton = el(document, 'button', { id: `${kind}-background` });
  panel.appendChild(backgroundButton);
  root.appendChild(panel);
  base.workspace.appendChild(root);

  if (kind === 'network-pair' || kind === 'network-unpair') {
    const modal = dialog(document, {
      id: kind === 'network-pair' ? 'link-pair-modal' : 'link-unpair-modal',
      class: 'link-modal',
      hidden: true,
    });
    modal.appendChild(el(document, 'div', { class: 'link-modal-box' }));
    panel.appendChild(modal);
    return {
      ...base,
      dialog: modal,
      expectedMarker: modal,
      open() { modal.removeAttribute('hidden'); },
      close() { modal.setAttribute('hidden', ''); },
    };
  }

  if (kind === 'import') {
    const modal = dialog(document, { id: 'detectModal', class: 'modal' });
    panel.appendChild(modal);
    return {
      ...base,
      dialog: modal,
      expectedMarker: modal,
      open() { modal.style.display = 'block'; document._notifyMutation('style'); },
      close() { modal.style.display = 'none'; document._notifyMutation('style'); },
    };
  }

  if (kind === 'observer') {
    const modal = dialog(document, { id: 'keyModal', class: 'modal' });
    panel.appendChild(modal);
    return {
      ...base,
      dialog: modal,
      expectedMarker: modal,
      open() { modal.style.display = 'block'; document._notifyMutation('style'); },
      close() { modal.style.display = 'none'; document._notifyMutation('style'); },
    };
  }

  if (kind === 'transcripts-delete') {
    const modal = dialog(document, {
      id: 'trDeleteSegmentModal',
      class: 'modal',
      styleDisplay: 'none',
    });
    panel.appendChild(modal);
    return {
      ...base,
      dialog: modal,
      expectedMarker: modal,
      open() { modal.style.display = 'flex'; document._notifyMutation('style'); },
      close() { modal.style.display = 'none'; document._notifyMutation('style'); },
    };
  }

  if (kind === 'transcripts-screenshot') {
    const modal = dialog(document, {
      id: 'trImageModal',
      class: 'tr-screenshot-modal',
    });
    modal.style.display = 'none';
    document.body.appendChild(modal);
    return {
      ...base,
      dialog: modal,
      expectedMarker: modal,
      open() { modal.style.display = 'flex'; document._notifyMutation('style'); },
      close() { modal.style.display = 'none'; document._notifyMutation('style'); },
    };
  }

  if (kind === 'sol') {
    const modal = dialog(document, { id: 'preview-modal', class: 'modal-backdrop' });
    panel.appendChild(modal);
    return {
      ...base,
      dialog: modal,
      expectedMarker: modal,
      open() { modal.classList.add('show'); },
      close() { modal.classList.remove('show'); },
    };
  }

  if (kind.startsWith('settings-')) {
    const modal = dialog(document, { id: kind, class: 'color-modal' });
    panel.appendChild(modal);
    return {
      ...base,
      dialog: modal,
      expectedMarker: modal,
      open() { modal.style.display = kind.endsWith('cleanup') ? 'flex' : 'block'; document._notifyMutation('style'); },
      close() { modal.style.display = 'none'; document._notifyMutation('style'); },
    };
  }

  if (kind === 'speakers') {
    const backdrop = el(document, 'div', { class: 'spk-who-backdrop', hidden: true });
    // who_is_this.js authors this tabindex when it builds the dialog shell.
    const modal = dialog(document, { class: 'spk-who-dialog', tabindex: '-1' });
    backdrop.appendChild(modal);
    document.body.appendChild(backdrop);
    return {
      ...base,
      backdrop,
      dialog: modal,
      expectedMarker: modal,
      expectedHost: backdrop,
      open() { backdrop.removeAttribute('hidden'); },
      close() { backdrop.setAttribute('hidden', ''); },
    };
  }

  throw new Error(`unknown shape: ${kind}`);
}

function assertInactive(context, shape) {
  context.flushFrames();
  assert(!context.document.body.classList.contains('has-managed-dialog'));
  assert(!shape.dialog.hasAttribute('data-convey-active-dialog'));
  if (shape.expectedHost) {
    assert(!shape.expectedHost.hasAttribute('data-convey-active-dialog-host'));
  }
}

function assertActive(context, shape) {
  context.flushFrames();
  assert(context.document.body.classList.contains('has-managed-dialog'));
  assert(shape.expectedMarker.hasAttribute('data-convey-active-dialog'));
  if (shape.expectedHost) {
    assert(shape.expectedHost.hasAttribute('data-convey-active-dialog-host'));
  }
}

function testVisibilityShapes() {
  [
    'network-pair',
    'network-unpair',
    'import',
    'observer',
    'transcripts-delete',
    'transcripts-screenshot',
    'sol',
    'settings-custom',
    'settings-icon',
    'settings-color',
    'settings-cleanup',
    'settings-cleanupLogs',
    'speakers',
  ].forEach((kind) => {
    const context = makeContext();
    const shape = makeWorkspaceShape(context, kind);
    context.ConveyModalLayer.reconcile();
    assertInactive(context, shape);
    shape.open();
    assertActive(context, shape);
    shape.close();
    assertInactive(context, shape);
  });
}

function testActivationDeactivation() {
  const context = makeContext();
  const shape = makeWorkspaceShape(context, 'network-pair');
  shape.open();
  assertActive(context, shape);
  shape.close();
  assertInactive(context, shape);
}

function testInertRestoration() {
  const context = makeContext();
  const shape = makeWorkspaceShape(context, 'network-pair');
  const preInert = shape.menu;
  preInert.setAttribute('inert', '');
  const sibling = el(context.document, 'button', { id: 'dialog-sibling' });
  shape.dialog.parentElement.appendChild(sibling);

  shape.open();
  assertActive(context, shape);
  assert(shape.skip.inert);
  assert(shape.chrome.inert);
  assert(preInert.inert);
  assert(sibling.inert);
  assert(!shape.workspace.inert);
  assert(!shape.dialog.inert);

  shape.close();
  assertInactive(context, shape);
  assert(!shape.skip.inert);
  assert(!shape.chrome.inert);
  assert(preInert.inert);
  assert(preInert.hasAttribute('inert'));
  assert(!sibling.inert);
  assert(!sibling.hasAttribute('inert'));
}

function testInitialFocusSkipsInvalidCandidates() {
  const context = makeContext();
  const shape = makeWorkspaceShape(context, 'network-pair');
  const hidden = el(context.document, 'button', { hidden: true });
  const inert = el(context.document, 'button');
  inert.setAttribute('inert', '');
  const disabled = el(context.document, 'button', { disabled: true });
  const nonRendered = el(context.document, 'button', { styleDisplay: 'none' });
  const target = el(context.document, 'button', { id: 'first-valid' });
  shape.dialog.appendChild(hidden);
  shape.dialog.appendChild(inert);
  shape.dialog.appendChild(disabled);
  shape.dialog.appendChild(nonRendered);
  shape.dialog.appendChild(target);
  shape.skip.focus();

  shape.open();
  assertActive(context, shape);
  assert.strictEqual(context.document.activeElement, target);
}

function openDialogWithCandidates(context, count) {
  const shape = makeWorkspaceShape(context, 'network-pair');
  const candidates = [];
  for (let index = 0; index < count; index += 1) {
    const button = el(context.document, 'button', { id: `candidate-${index}` });
    shape.dialog.appendChild(button);
    candidates.push(button);
  }
  shape.open();
  assertActive(context, shape);
  return { shape, candidates };
}

function pressTab(context, shiftKey = false) {
  return context.document.dispatchEvent(makeEvent('keydown', { key: 'Tab', shiftKey }));
}

// Mirrors the unconditional index-cycling document-bubble trap that Observer
// (workspace.html:939) and the Transcripts screenshot modal (l.5559) install.
function installDownstreamTrap(context, container) {
  const trap = { entries: [] };
  context.document.addEventListener('keydown', (event) => {
    if (event.key !== 'Tab') return;
    trap.entries.push({ defaultPrevented: event.defaultPrevented });
    const focusable = container.querySelectorAll('button');
    if (!focusable.length) return;
    const index = focusable.indexOf(context.document.activeElement);
    event.preventDefault();
    if (event.shiftKey) {
      focusable[index <= 0 ? focusable.length - 1 : index - 1].focus();
    } else {
      focusable[index >= focusable.length - 1 ? 0 : index + 1].focus();
    }
  });
  return trap;
}

function testTabWrapsForwardAtLastCandidate() {
  const context = makeContext();
  const { candidates } = openDialogWithCandidates(context, 3);
  candidates[candidates.length - 1].focus();

  const before = context.document.focusLog.length;
  const event = pressTab(context);

  assert.strictEqual(event.defaultPrevented, true);
  assert.strictEqual(context.document.activeElement, candidates[0]);
  assert.strictEqual(context.document.focusLog.length, before + 1);
}

function testTabWrapsReverseAtFirstCandidate() {
  const context = makeContext();
  const { candidates } = openDialogWithCandidates(context, 3);
  assert.strictEqual(context.document.activeElement, candidates[0]);

  const before = context.document.focusLog.length;
  const event = pressTab(context, true);

  assert.strictEqual(event.defaultPrevented, true);
  assert.strictEqual(context.document.activeElement, candidates[candidates.length - 1]);
  assert.strictEqual(context.document.focusLog.length, before + 1);
}

function testTabFromOutsideEntersDialog() {
  const context = makeContext();
  const { shape, candidates } = openDialogWithCandidates(context, 3);

  // Removing the focused control drops focus outside the dialog without a focusin.
  shape.dialog.removeChild(candidates[0]);
  context.flushFrames();
  assert.strictEqual(context.document.activeElement, null);

  const forward = pressTab(context);
  assert.strictEqual(forward.defaultPrevented, true);
  assert.strictEqual(context.document.activeElement, candidates[1]);

  context.document.activeElement = null;
  const reverse = pressTab(context, true);
  assert.strictEqual(reverse.defaultPrevented, true);
  assert.strictEqual(context.document.activeElement, candidates[2]);
}

function testTabWithoutCandidatesFocusesDialog() {
  const context = makeContext();
  const shape = makeWorkspaceShape(context, 'network-pair');
  const disabled = el(context.document, 'button', { disabled: true });
  shape.dialog.appendChild(disabled);
  shape.open();
  assertActive(context, shape);

  const event = pressTab(context);
  assert.strictEqual(event.defaultPrevented, true);
  assert.strictEqual(context.document.activeElement, shape.dialog);
  assert.strictEqual(shape.dialog.getAttribute('tabindex'), '-1');

  shape.close();
  assertInactive(context, shape);
  assert(!shape.dialog.hasAttribute('tabindex'));

  shape.open();
  assertActive(context, shape);
  pressTab(context);
  assert.strictEqual(context.document.activeElement, shape.dialog);
  assert(shape.dialog.hasAttribute('tabindex'));

  context.document.body.removeChild(shape.workspace);
  context.flushFrames();
  assert(!shape.dialog.hasAttribute('tabindex'));
}

function testAuthoredDialogTabIndexPreserved() {
  const context = makeContext();
  const shape = makeWorkspaceShape(context, 'speakers');
  assert.strictEqual(shape.dialog.getAttribute('tabindex'), '-1');
  shape.open();
  assertActive(context, shape);

  const event = pressTab(context);
  assert.strictEqual(event.defaultPrevented, true);
  assert.strictEqual(context.document.activeElement, shape.dialog);

  shape.close();
  assertInactive(context, shape);
  assert.strictEqual(shape.dialog.getAttribute('tabindex'), '-1');
}

function testInteriorTabStaysNative() {
  const context = makeContext();
  const { shape, candidates } = openDialogWithCandidates(context, 3);
  const trap = installDownstreamTrap(context, shape.dialog);
  candidates[1].focus();

  const before = context.document.focusLog.length;
  const event = pressTab(context);

  assert.strictEqual(trap.entries.length, 1);
  assert.strictEqual(trap.entries[0].defaultPrevented, false);
  assert.strictEqual(context.document.activeElement, candidates[2]);
  assert.strictEqual(context.document.focusLog.length, before + 1);
  assert.strictEqual(event.propagationStopped, false);
}

function testBoundaryTabBlocksDownstreamTrap() {
  const context = makeContext();
  const { shape, candidates } = openDialogWithCandidates(context, 3);
  const trap = installDownstreamTrap(context, shape.dialog);

  candidates[candidates.length - 1].focus();
  let before = context.document.focusLog.length;
  pressTab(context);
  assert.strictEqual(trap.entries.length, 0);
  assert.strictEqual(context.document.activeElement, candidates[0]);
  assert.strictEqual(context.document.focusLog.length, before + 1);

  before = context.document.focusLog.length;
  pressTab(context, true);
  assert.strictEqual(trap.entries.length, 0);
  assert.strictEqual(context.document.activeElement, candidates[candidates.length - 1]);
  assert.strictEqual(context.document.focusLog.length, before + 1);

  context.document.activeElement = null;
  before = context.document.focusLog.length;
  pressTab(context);
  assert.strictEqual(trap.entries.length, 0);
  assert.strictEqual(context.document.activeElement, candidates[0]);
  assert.strictEqual(context.document.focusLog.length, before + 1);

  candidates.forEach((candidate) => shape.dialog.removeChild(candidate));
  context.flushFrames();
  before = context.document.focusLog.length;
  pressTab(context);
  assert.strictEqual(trap.entries.length, 0);
  assert.strictEqual(context.document.activeElement, shape.dialog);
  assert.strictEqual(context.document.focusLog.length, before + 1);
}

function testOutsideFocusRedirects() {
  const context = makeContext();
  const shape = makeWorkspaceShape(context, 'network-pair');
  const first = el(context.document, 'button', { id: 'inside' });
  shape.dialog.appendChild(first);
  shape.open();
  assertActive(context, shape);

  shape.skip.focus();
  assert.strictEqual(context.document.activeElement, first);
}

function testOpenerRestoresAfterSynchronousAppFocus() {
  const context = makeContext();
  const shape = makeWorkspaceShape(context, 'network-pair');
  const inside = el(context.document, 'button', { id: 'inside' });
  shape.dialog.appendChild(inside);
  shape.skip.focus();

  shape.open();
  inside.focus();
  context.flushFrames();
  assert.strictEqual(context.document.activeElement, inside);

  shape.close();
  context.flushFrames();
  assert.strictEqual(context.document.activeElement, shape.skip);
}

function testRepeatedWorkspaceMountedIsIdempotent() {
  const context = makeContext();
  shell(context.document);
  context.ConveyModalLayer.init();
  context.ConveyModalLayer.init();
  context.document.dispatchEvent(makeEvent('workspace:mounted', { target: context.document }));
  context.document.dispatchEvent(makeEvent('workspace:mounted', { target: context.document }));

  assert.strictEqual(context.document.observers.length, 1);
  assert.strictEqual(context.document.listenerCount('workspace:mounted'), 1);
  assert.strictEqual(context.document.listenerCount('focusin'), 1);
  assert.strictEqual(context.document.listenerCount('keydown'), 1);
}

function testWorkspaceRemovalRestoresState() {
  const context = makeContext();
  const shape = makeWorkspaceShape(context, 'network-pair');
  shape.open();
  assertActive(context, shape);
  assert(shape.skip.inert);
  assert(shape.dialog.hasAttribute('data-convey-active-dialog'));

  context.document.body.removeChild(shape.workspace);
  context.flushFrames();

  assert(!context.document.body.classList.contains('has-managed-dialog'));
  assert(!shape.skip.inert);
  assert(!shape.skip.hasAttribute('inert'));
  assert(!shape.dialog.hasAttribute('data-convey-active-dialog'));
}

function testPositionedDialogDoesNotMarkPositionedAncestor() {
  const context = makeContext();
  const base = shell(context.document);
  const positionedAncestor = el(context.document, 'section', {
    id: 'positioned-ancestor',
    class: 'position-relative',
  });
  const modal = dialog(context.document, {
    id: 'fixed-dialog',
    class: 'fixed-dialog',
  });
  modal.appendChild(el(context.document, 'button', { id: 'inside-fixed-dialog' }));
  positionedAncestor.appendChild(modal);
  base.workspace.appendChild(positionedAncestor);

  context.ConveyModalLayer.reconcile();

  assert(context.document.body.classList.contains('has-managed-dialog'));
  assert(modal.hasAttribute('data-convey-active-dialog'));
  assert(!positionedAncestor.hasAttribute('data-convey-active-dialog-host'));
}

function testDetachedInertedElementRestoresBeforeReattach() {
  const context = makeContext();
  const shape = makeWorkspaceShape(context, 'network-pair');
  const parent = shape.dialog.parentElement;
  const detachedSibling = el(context.document, 'section', { id: 'detached-sibling' });
  parent.appendChild(detachedSibling);

  shape.open();
  assertActive(context, shape);
  assert(detachedSibling.inert);
  assert(detachedSibling.hasAttribute('inert'));

  parent.removeChild(detachedSibling);
  shape.close();
  context.flushFrames();
  parent.appendChild(detachedSibling);

  assert(!detachedSibling.inert);
  assert(!detachedSibling.hasAttribute('inert'));
}

const cases = {
  visibility_shapes: testVisibilityShapes,
  activation_deactivation: testActivationDeactivation,
  inert_restoration: testInertRestoration,
  initial_focus_skips_invalid: testInitialFocusSkipsInvalidCandidates,
  tab_wraps_forward_at_last_candidate: testTabWrapsForwardAtLastCandidate,
  tab_wraps_reverse_at_first_candidate: testTabWrapsReverseAtFirstCandidate,
  tab_from_outside_enters_dialog: testTabFromOutsideEntersDialog,
  tab_without_candidates_focuses_dialog: testTabWithoutCandidatesFocusesDialog,
  authored_dialog_tabindex_preserved: testAuthoredDialogTabIndexPreserved,
  interior_tab_stays_native: testInteriorTabStaysNative,
  boundary_tab_blocks_downstream_trap: testBoundaryTabBlocksDownstreamTrap,
  outside_focus_redirect: testOutsideFocusRedirects,
  opener_restoration_after_sync_focus: testOpenerRestoresAfterSynchronousAppFocus,
  repeated_workspace_mounted_idempotent: testRepeatedWorkspaceMountedIsIdempotent,
  workspace_removal_restores_state: testWorkspaceRemovalRestoresState,
  positioned_dialog_does_not_mark_positioned_ancestor:
    testPositionedDialogDoesNotMarkPositionedAncestor,
  detached_inerted_element_restores_before_reattach:
    testDetachedInertedElementRestoresBeforeReattach,
};

if (!cases[caseName]) {
  throw new Error(`unknown modal layer harness case: ${caseName}`);
}

cases[caseName]();
console.log(JSON.stringify({ ok: true, case: caseName }));
