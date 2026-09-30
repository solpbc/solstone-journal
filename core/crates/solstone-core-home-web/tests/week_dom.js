// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2] || path.resolve(__dirname, '..');
const weekJsPath = path.join(crateDir, 'assets/week.js');
const weekJsCode = fs.readFileSync(weekJsPath, 'utf8');

class MockElement {
  constructor(tagName = 'div') {
    this.tagName = tagName.toUpperCase();
    this.attributes = {};
    this.style = {};
    this.childNodes = [];
    this.listeners = {};
    this.hidden = false;
    this.focused = false;
    this._innerHTML = '';
  }

  get innerHTML() {
    return this._innerHTML;
  }

  set innerHTML(val) {
    this._innerHTML = val;
    this.parseChildren(val);
  }

  get textContent() {
    return this.childNodes.map(c => typeof c === 'string' ? c : c.textContent).join('');
  }

  set textContent(val) {
    this.childNodes = [String(val)];
  }

  setAttribute(k, v) {
    this.attributes[k] = String(v);
  }

  getAttribute(k) {
    return this.attributes[k] || null;
  }

  removeAttribute(k) {
    delete this.attributes[k];
  }

  addEventListener(event, fn) {
    if (!this.listeners[event]) this.listeners[event] = [];
    this.listeners[event].push(fn);
  }

  dispatchEvent(event) {
    const list = this.listeners[event.type] || [];
    list.forEach(fn => fn.call(this, event));
  }

  focus() {
    this.focused = true;
  }

  contains(target) {
    if (this === target) return true;
    for (const child of this.childNodes) {
      if (typeof child !== 'string' && child.contains(target)) return true;
    }
    return false;
  }

  querySelector(selector) {
    const all = this.querySelectorAll(selector);
    return all.length ? all[0] : null;
  }

  querySelectorAll(selector) {
    const matches = [];
    function search(el) {
      if (typeof el === 'string') return;
      if (selector.startsWith('.')) {
        const cls = selector.slice(1);
        const elCls = el.getAttribute('class') || '';
        if (elCls.split(/\s+/).includes(cls)) matches.push(el);
      } else if (selector.startsWith('[data-action="')) {
        const val = selector.match(/\[data-action="([^"]+)"\]/)[1];
        if (el.getAttribute('data-action') === val) matches.push(el);
      } else if (selector === '.week-menu-wrapper') {
        const elCls = el.getAttribute('class') || '';
        if (elCls.split(/\s+/).includes('week-menu-wrapper')) matches.push(el);
      } else if (selector === '.week-menu-btn') {
        const elCls = el.getAttribute('class') || '';
        if (elCls.split(/\s+/).includes('week-menu-btn')) matches.push(el);
      } else if (selector === '.week-menu-dropdown') {
        const elCls = el.getAttribute('class') || '';
        if (elCls.split(/\s+/).includes('week-menu-dropdown')) matches.push(el);
      } else if (selector === '.week-error-banner') {
        const elCls = el.getAttribute('class') || '';
        if (elCls.split(/\s+/).includes('week-error-banner')) matches.push(el);
      } else if (selector.startsWith('[data-action]')) {
        if (el.getAttribute('data-action')) matches.push(el);
      }
      (el.childNodes || []).forEach(search);
    }
    (this.childNodes || []).forEach(search);
    return matches;
  }

  parseChildren(html) {
    // Simple structural mockup for test cases
    this.childNodes = [];
    if (html.includes('week-menu-wrapper')) {
      const wrapper = new MockElement('div');
      wrapper.setAttribute('class', 'week-menu-wrapper');
      const btn = new MockElement('button');
      btn.setAttribute('class', 'week-menu-btn');
      btn.setAttribute('aria-expanded', 'false');
      const dropdown = new MockElement('div');
      dropdown.setAttribute('class', 'week-menu-dropdown');
      dropdown.hidden = true;
      dropdown.style.display = 'none';
      const leaveOutBtn = new MockElement('button');
      leaveOutBtn.setAttribute('data-action', 'leave-out');
      leaveOutBtn.setAttribute('data-key', 'k1');
      dropdown.childNodes.push(leaveOutBtn);
      wrapper.childNodes.push(btn, dropdown);
      this.childNodes.push(wrapper);
    }
    if (html.includes('week-undo-btn')) {
      const undoBtn = new MockElement('button');
      undoBtn.setAttribute('class', 'week-undo-btn');
      undoBtn.setAttribute('data-action', 'undo');
      undoBtn.setAttribute('data-key', 'k1');
      this.childNodes.push(undoBtn);
    }
    const errBanner = new MockElement('div');
    errBanner.setAttribute('class', 'week-error-banner');
    errBanner.style.display = 'none';
    this.childNodes.push(errBanner);
  }
}

class MockDocument extends MockElement {
  constructor() {
    super('document');
  }
}

const mockDoc = new MockDocument();
const mockWindow = {
  document: mockDoc,
  AppServices: {
    renderMarkdown: (t) => '<p>' + t + '</p>'
  }
};

const sandbox = {
  window: mockWindow,
  document: mockDoc,
  console: console,
  fetch: () => Promise.resolve({ ok: true, json: () => Promise.resolve({}) })
};

vm.createContext(sandbox);
vm.runInContext(weekJsCode, sandbox);

let passedCases = 0;

// Test 1: renderWeekHtml produces expected html structure
const sampleModel = {
  day: '20260810',
  title: 'week of august 10, 2026',
  intro: 'a productive week.',
  cells: [
    { weekday: 'Mon', day_number: '10', state: 'active', accessible_name: 'Mon Aug 10, active' },
    { weekday: 'Tue', day_number: '11', state: 'active', accessible_name: 'Tue Aug 11, active' },
    { weekday: 'Wed', day_number: '12', state: 'active', accessible_name: 'Wed Aug 12, active' },
    { weekday: 'Thu', day_number: '13', state: 'active', accessible_name: 'Thu Aug 13, active' },
    { weekday: 'Fri', day_number: '14', state: 'active', accessible_name: 'Fri Aug 14, active' },
    { weekday: 'Sat', day_number: '15', state: 'quiet', accessible_name: 'Sat Aug 15, quiet' },
    { weekday: 'Sun', day_number: '16', state: 'quiet', accessible_name: 'Sun Aug 16, quiet' }
  ],
  legend: [
    { state: 'active', label: 'active' },
    { state: 'quiet', label: 'quiet' }
  ],
  from_line: 'from 5 days with activity.',
  left_out_notice: null,
  rows: [
    {
      key: 'k1',
      day_label: 'monday · aug 10',
      text: 'Shipped feature.',
      source_href: '/app/transcripts/20260810',
      peek_caption: 'monday briefing',
      peek_label: 'open briefing',
      peek_href: '/app/home#briefing',
      left_out: false
    }
  ],
  prev_href: '/app/home/week/20260803',
  prev_label: 'week of august 3, 2026',
  next_href: null,
  next_label: null,
  end_line: "that's the week."
};

const html = sandbox.window.renderWeekHtml(sampleModel);
assert.ok(html.includes('week of august 10, 2026'), 'title rendered');
assert.ok(html.includes('a productive week.'), 'intro rendered');
assert.ok(html.includes('from 5 days with activity.'), 'from_line rendered');
assert.ok(html.includes('Shipped feature.'), 'row text rendered');
assert.ok(html.includes('week of august 3, 2026'), 'nav rendered');
passedCases++;

// Test 2: renderWeekHtml with left_out row and null notice
const leftOutModel = JSON.parse(JSON.stringify(sampleModel));
leftOutModel.rows[0].left_out = true;
leftOutModel.left_out_notice = null;
const leftOutHtml = sandbox.window.renderWeekHtml(leftOutModel);
assert.ok(leftOutHtml.includes('left out of this week · '), 'left out text rendered');
assert.ok(leftOutHtml.includes('week-undo-btn'), 'undo button rendered');
assert.strictEqual(leftOutHtml.includes('memory left out'), false, 'does not contain memory left out');
assert.strictEqual(leftOutHtml.includes('memories left out'), false, 'does not contain memories left out');
passedCases++;

// Test 2b: renderWeekHtml with unreadable left_out_notice
const unreadableModel = JSON.parse(JSON.stringify(sampleModel));
unreadableModel.left_out_notice = "your left-out memories couldn't be checked, so everything is showing.";
const unreadableHtml = sandbox.window.renderWeekHtml(unreadableModel);
assert.ok(unreadableHtml.includes("your left-out memories couldn&#39;t be checked, so everything is showing.") || unreadableHtml.includes("your left-out memories couldn't be checked, so everything is showing."), 'unreadable notice rendered');
passedCases++;

// Test 3: Escape key closes active menu and restores focus
const container = new MockElement('div');
sandbox.window.renderWeekPage(container, sampleModel);

const wrapper = container.querySelector('.week-menu-wrapper');
const btn = wrapper.querySelector('.week-menu-btn');
const dropdown = wrapper.querySelector('.week-menu-dropdown');

assert.strictEqual(btn.getAttribute('aria-expanded'), 'false');
assert.strictEqual(dropdown.hidden, true);

// Open menu
btn.dispatchEvent({ type: 'click', stopPropagation: () => {} });
assert.strictEqual(btn.getAttribute('aria-expanded'), 'true');
assert.strictEqual(dropdown.hidden, false);
assert.strictEqual(dropdown.style.display, 'block');

// Press Escape on document
mockDoc.dispatchEvent({
  type: 'keydown',
  key: 'Escape',
  preventDefault: () => {}
});

assert.strictEqual(btn.getAttribute('aria-expanded'), 'false');
assert.strictEqual(dropdown.hidden, true);
assert.strictEqual(dropdown.style.display, 'none');
assert.strictEqual(btn.focused, true, 'focus restored to menu button');
passedCases++;

console.log(`DOM CASES: ${passedCases} passed`);
