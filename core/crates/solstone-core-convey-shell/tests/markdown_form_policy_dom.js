// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Rendered markdown carries captured and generated text, so it must never become a working form.
// Drives the shell's renderMarkdown with the vendored marked and a sanitizer that records what the
// shell asks of it: every form control marked can emit is forbidden, a task list is a mark rather
// than a control, and no submission target survives, even one pointing back at this journal.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');
const staticDir = path.join(crateDir, 'assets/static');
const marked = require(path.join(staticDir, 'vendor/marked/marked.min.js'));
let cases = 0;

const FORM_CONTROLS = ['form', 'input', 'button', 'select', 'textarea'];
const PLANTED = [
  '<form action="/app/thinking/api/local/endpoint" method="post" enctype="text/plain">',
  '<input name=\'{"endpoint_url":"http://attacker.example/v1","served_model_id":"x","pad":"\' value=\'"}\'>',
  '<button>show full message</button></form>',
  '<button form="elsewhere" formaction="/app/thinking/api/local/endpoint">go</button>',
  '<select name="a"><option>1</option></select><textarea name="b">t</textarea>',
  '- [ ] open task',
  '- [x] done task',
].join('\n\n');

function harness() {
  const sanitized = [];
  const hooks = {};
  const window = {
    location: { href: 'http://localhost:5015/app/transcripts/', origin: 'http://localhost:5015', protocol: 'http:' },
    ConveyIcons: { svg() { return ''; } },
    convey: {},
  };
  window.window = window;
  const document = {
    readyState: 'complete',
    addEventListener() {},
    querySelector() { return null; },
    getElementById() { return null; },
    createElement() { return { innerHTML: '' }; },
    createTreeWalker() { return { nextNode() { return false; } }; },
  };
  const DOMPurify = {
    addHook(name, hook) { hooks[name] = hook; },
    sanitize(html, options) {
      sanitized.push({ html, options });
      return html;
    },
  };
  const context = vm.createContext({
    window, document, DOMPurify, marked, URL, NodeFilter: { SHOW_TEXT: 4 },
    localStorage: { getItem() { return null; }, setItem() {} },
    navigator: {}, console, Map, Set, Date, JSON, Promise,
    setTimeout, clearTimeout, setInterval, clearInterval,
  });
  vm.runInContext(fs.readFileSync(path.join(staticDir, 'app.js'), 'utf8'), context, { filename: 'app.js' });
  return { AppServices: window.AppServices, sanitized, hooks };
}

function controlsIn(html) {
  return FORM_CONTROLS.filter((tag) => new RegExp(`<${tag}[\\s>/]`, 'i').test(html));
}

function testEveryEmittedControlIsForbidden() {
  const { AppServices, sanitized } = harness();
  AppServices.renderMarkdown(PLANTED);
  assert.strictEqual(sanitized.length, 1, 'renderMarkdown sanitizes exactly once');
  const [{ html, options }] = sanitized;
  const forbidden = new Set((options.FORBID_TAGS || []).map((tag) => tag.toLowerCase()));
  const emitted = controlsIn(html);
  assert.deepStrictEqual(emitted, FORM_CONTROLS, `the corpus reaches the sanitizer with every control: ${emitted}`);
  for (const tag of FORM_CONTROLS) {
    assert.ok(forbidden.has(tag), `<${tag}> in rendered text is forbidden`);
  }
  assert.ok(!('ADD_TAGS' in options) && !('ALLOWED_TAGS' in options), 'no option re-admits a forbidden tag');
  cases += 1;
}

function testTaskListIsAMarkNotAControl() {
  const { AppServices, sanitized } = harness();
  AppServices.renderMarkdown('- [ ] open\n- [x] done');
  const [{ html }] = sanitized;
  assert.deepStrictEqual(controlsIn(html), [], `a task list renders no form control: ${html}`);
  assert.ok(html.includes('☐ open') && html.includes('☑ done'), `a task list keeps its state: ${html}`);
  cases += 1;
}

function testNoSubmissionTargetSurvives() {
  const { AppServices, hooks } = harness();
  AppServices.renderMarkdown('');
  const hook = hooks.uponSanitizeAttribute;
  assert.strictEqual(typeof hook, 'function', 'the shell installs its attribute policy');
  const verdict = (attrName, attrValue) => {
    const data = { attrName, attrValue, keepAttr: true };
    hook({}, data);
    return data.keepAttr;
  };
  for (const name of ['action', 'formaction', 'ACTION', 'FormAction']) {
    for (const value of ['/app/thinking/api/local/endpoint', 'http://localhost:5015/app/x', 'http://attacker.example/']) {
      assert.strictEqual(verdict(name, value), false, `${name}="${value}" is dropped`);
    }
  }
  assert.strictEqual(verdict('href', '/app/news/work/20260930'), true, 'a same-origin link survives');
  assert.strictEqual(verdict('href', 'http://attacker.example/'), false, 'a remote link does not');
  cases += 1;
}

testEveryEmittedControlIsForbidden();
testTaskListIsAMarkNotAControl();
testNoSubmissionTargetSurvives();
console.log(`DOM CASES: ${cases} passed`);
