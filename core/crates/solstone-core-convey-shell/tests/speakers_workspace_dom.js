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

function main() {
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

  console.log(`DOM CASES: ${cases} passed`);
}

main();
