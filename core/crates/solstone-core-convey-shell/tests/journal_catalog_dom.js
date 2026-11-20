// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function script(file) {
  return new vm.Script(fs.readFileSync(path.join(__dirname, file), 'utf8'), { filename: file });
}

const bundle = script('../../../../i18n/generated/window/classic/journal-window/api.js');
const runtime = script('../assets/static/convey_copy.js');

function loadWindowCatalog(context) {
  if (!context.window.JournalI18n) {
    bundle.runInContext(context);
    runtime.runInContext(context);
  }
}

function journalContext(sandbox) {
  sandbox.window ??= sandbox;
  const context = vm.createContext(sandbox);
  loadWindowCatalog(context);
  return context;
}

function runWithJournalCatalog(source, sandbox = {}, options) {
  return vm.runInContext(source, journalContext(sandbox), options);
}

function readCopyTable(file) {
  const render = journalContext({}).JournalI18n.renderEnglish;
  function resolve(value) {
    if (Array.isArray(value)) return value.map(resolve);
    if (value && typeof value === 'object') {
      return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, resolve(entry)]));
    }
    return typeof value === 'string' && /^!?journal\.window\./.test(value)
      ? render(value.replace(/^!/, '')) : value;
  }
  return resolve(JSON.parse(fs.readFileSync(file, 'utf8')));
}

// The markup renderer uses text nodes, fragments, and small inline elements.
function markupNode(tag = '') {
  function escapeText(value) {
    return String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  }
  let body = '';
  return {
    attrs: {},
    className: '',
    setAttribute(name, value) { this.attrs[name] = String(value); },
    append(...parts) {
      for (const part of parts) body += typeof part === 'string' ? escapeText(part) : part.outerHTML;
    },
    appendChild(part) { this.append(part); return part; },
    set textContent(value) { body = escapeText(value); },
    get innerHTML() { return body; },
    set innerHTML(value) { body = String(value); },
    get outerHTML() {
      const attrs = Object.entries(this.attrs);
      if (this.className) attrs.unshift(['class', this.className]);
      for (const name of ['target', 'rel']) if (this[name]) attrs.push([name, this[name]]);
      const suffix = attrs.map(([name, value]) =>
        ` ${name}="${escapeText(value).replaceAll('"', '&quot;')}"`).join('');
      return tag ? `<${tag}${suffix}>${body}</${tag}>` : body;
    },
  };
}

module.exports = { loadWindowCatalog, journalContext, runWithJournalCatalog, readCopyTable, markupNode };
