// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Drives the agents app's embedded script against a stubbed journal, in each state the journal can
// report: no local door at all, a local door that is not listening (every reason), and a listening
// local door, each with solstone.me off and on.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const root = process.argv[2] || path.join(__dirname, '..');
const html = fs.readFileSync(path.join(root, 'assets/agents/workspace.html'), 'utf8');
const script = html.slice(html.indexOf('<script>') + '<script>'.length, html.indexOf('</script>'));
let cases = 0;

const ADDRESS = 'http://127.0.0.1:7659/mcp';
const MARK = {
  committed: true,
  instance_id: '0f8fad5b-d9cb-469f-a165-70867728950e',
  mark: {
    icon1: {name: 'anchor', svg: '<path d="M12 6v16" />', color: {name: 'teal', hex: '#0f766e'}, rot: 0},
    icon2: {name: 'bike', svg: '<circle cx="5" cy="5" r="3" />', color: {name: 'amber', hex: '#b45309'}, rot: 45},
    words: ['afoot', 'unfixed'],
  },
};

// Copy that is only true while nothing but solstone.me can reach the journal.
const SOLSTONE_ME_ONLY = [
  'nothing is reachable until you turn it on',
  'this is off until you turn it on',
  'stay connected on paper',
];

function baseState(overrides = {}) {
  return {
    enabled: false,
    status: 'off',
    owner_state: null,
    certificate: {current: false},
    connections: [],
    facets: [],
    pairing: null,
    ...overrides,
  };
}

function connection(kind, id, name, door) {
  return {
    kind, id, key: `${kind}:${id}`, name, door, created_at: '2026-09-24T12:00:00Z',
    permission: null, requests_this_week: 2, last_request_at: null, activity_complete: true,
  };
}

async function boot(state, {identity = MARK, identityFails = false} = {}) {
  const calls = [];
  const view = {
    innerHTML: '',
    listeners: {},
    addEventListener(name, listener) { (this.listeners[name] ||= []).push(listener); },
    querySelector() { return null; },
    insertAdjacentHTML(_, text) { this.innerHTML += text; },
  };
  const window = {
    AppServices: {escapeHtml: value => value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')},
    apiJson: async (url, options = {}) => {
      const method = options.method || 'GET';
      calls.push({url, method, body: options.body ? JSON.parse(options.body) : undefined});
      if (url === '/app/agents/api/state') return JSON.parse(JSON.stringify(state));
      if (url === '/app/network/api/identity') {
        if (identityFails) throw new Error('unavailable');
        return identity;
      }
      if (url === '/app/agents/api/pairing') return {code: 'K7Q2M9XA', expires_at: '2026-09-24T12:10:00Z', generation: 1};
      if (url === '/app/agents/api/local-door') return {enabled: options.body && JSON.parse(options.body).enabled, changed: true};
      if (url === '/app/agents/api/enable') return {operation: null};
      throw new Error(`unexpected request ${method} ${url}`);
    },
    setInterval() {},
    open() {},
  };
  const document = {getElementById: id => (id === 'agents-view' ? view : null)};
  const context = vm.createContext({
    window, document, navigator: {clipboard: {writeText: async () => {}}},
    setTimeout, clearTimeout, confirm: () => true, prompt: () => null, URLSearchParams, console,
  });
  vm.runInContext(script, context, {filename: 'agents-workspace.js'});
  await settle();
  const click = async dataset => {
    const target = {dataset, textContent: '', closest() { return this; }};
    for (const listener of view.listeners.click || []) await listener({target, preventDefault() {}});
    await settle();
  };
  return {view, calls, click};
}

async function settle() {
  for (let i = 0; i < 5; i += 1) await new Promise(resolve => setImmediate(resolve));
}

function assertNoSolstoneMeOnlyCopy(text, where) {
  for (const copy of SOLSTONE_ME_ONLY) assert(!text.includes(copy), `${where}: false with a local door: "${copy}"`);
}

async function test(name, body) {
  await body();
  cases += 1;
}

(async () => {
  await test('no local door: the solstone.me-only app is unchanged', async () => {
    const {view} = await boot(baseState());
    assert(view.innerHTML.includes('off by default. nothing is reachable until you turn it on'));
    assert(!view.innerHTML.includes('same computer'));
  });

  await test('no local door: connect page shows the mark beside the pairing code', async () => {
    const {view, click, calls} = await boot(baseState({enabled: true, status: 'on', owner_state: {address: 'k7q2m9xa.solstone.me', status: 'on'}}));
    assert(view.innerHTML.includes('your agents'));
    await click({route: 'connect'});
    assert(view.innerHTML.includes('in Claude'));
    assert(view.innerHTML.includes('https://k7q2m9xa.solstone.me/mcp'));
    assert(view.innerHTML.includes("enter it only on a page that shows your journal's mark"));
    assert(view.innerHTML.includes('aria-label="teal, amber · afoot unfixed"'));
    await click({action: 'new-code'});
    assert(calls.some(call => call.url === '/app/agents/api/pairing' && call.method === 'POST'));
    assert(view.innerHTML.includes('K7Q2M9XA'));
  });

  const reasons = {
    disabled: {enabled: false, reason: 'disabled', title: 'agents on the same computer</p><h2><span class="dot wait"></span>off<', action: 'door-on'},
    config_invalid: {enabled: false, reason: 'config_invalid', title: "off until your journal&#39;s settings are fixed", action: 'door-on'},
    config_unreadable: {enabled: true, reason: 'config_unreadable', title: "your journal can't read its settings", action: 'refresh'},
    not_running: {enabled: true, reason: 'not_running', title: 'not running right now', action: 'refresh'},
    port_in_use: {enabled: true, reason: 'port_in_use', title: 'something else is using this address', action: 'refresh'},
  };
  for (const [reason, expected] of Object.entries(reasons)) {
    await test(`local door not listening (${reason}), solstone.me off`, async () => {
      const {view, click} = await boot(baseState({local_door: {enabled: expected.enabled, address: ADDRESS, listening: false, reason}}));
      const title = expected.title.replace('&#39;', "'");
      assert(view.innerHTML.includes(title), `missing title for ${reason}`);
      assert(view.innerHTML.includes(`data-action="${expected.action}"`), `missing recovery for ${reason}`);
      assert(view.innerHTML.includes('connect an agent'), 'the pairing code must be reachable without solstone.me');
      assertNoSolstoneMeOnlyCopy(view.innerHTML, `home (${reason})`);
      await click({route: 'me'});
      assertNoSolstoneMeOnlyCopy(view.innerHTML, `solstone.me page (${reason})`);
      assert(view.innerHTML.includes('turn on solstone.me'));
    });
  }

  await test('port in use: the warning carries the mark and does not reassure', async () => {
    const {view, click} = await boot(baseState({local_door: {enabled: true, address: ADDRESS, listening: false, reason: 'port_in_use'}}));
    assert(view.innerHTML.includes('a program pretending to be your journal'));
    assert(view.innerHTML.includes('aria-label="teal, amber · afoot unfixed"'));
    assert(view.innerHTML.includes("don't enter the code"));
    await click({route: 'connect'});
    assert(view.innerHTML.includes('something else is using this address'), 'the connect page must repeat the warning');
    assert.strictEqual(view.innerHTML.split('class="journal-mark"').length - 1, 1, 'one screen shows the mark once');
  });

  await test('port in use without a readable mark still says to check it', async () => {
    const {view} = await boot(baseState({local_door: {enabled: true, address: ADDRESS, listening: false, reason: 'port_in_use'}}), {identityFails: true});
    assert(view.innerHTML.includes("check that the page asking for it shows your journal's mark"));
    assert(!view.innerHTML.includes('journal-mark'));
  });

  await test('a disabled door turns back on through its own switch', async () => {
    const {click, calls} = await boot(baseState({local_door: {enabled: false, address: ADDRESS, listening: false, reason: 'disabled'}}));
    await click({action: 'door-on'});
    const put = calls.find(call => call.url === '/app/agents/api/local-door');
    assert(put && put.method === 'PUT' && put.body.enabled === true);
  });

  await test('listening, solstone.me off: address, pairing and connections by door', async () => {
    const state = baseState({
      local_door: {enabled: true, address: ADDRESS, listening: true},
      connections: [connection('oauth', 'g1', 'tiles', 'local'), connection('oauth', 'g2', 'Claude', 'relay'), connection('bearer', 't1', 'my script', null)],
    });
    const {view, click, calls} = await boot(state);
    assert(view.innerHTML.includes('agents on the same computer</p><h2><span class="dot "></span>open to agents'));
    assert(view.innerHTML.includes(`data-copy="${ADDRESS}"`));
    assertNoSolstoneMeOnlyCopy(view.innerHTML, 'home (listening)');
    assert(view.innerHTML.includes('paired in your browser · same computer'));
    assert(view.innerHTML.includes('paired in your browser · through solstone.me'));
    assert(view.innerHTML.includes('a key you created · connected'));
    await click({connection: 'oauth:g1'});
    assert(view.innerHTML.includes('it paired from the same computer.'));
    assert(view.innerHTML.includes('data-revoke="oauth:g1"'));
    await click({route: 'connect'});
    assert(view.innerHTML.includes('an agent on the same computer'));
    assert(view.innerHTML.includes("can't reach an address on the same computer"));
    assert(!view.innerHTML.includes('in Claude'), 'no Claude steps without an internet address');
    await click({action: 'new-code'});
    assert(calls.some(call => call.url === '/app/agents/api/pairing' && call.method === 'POST'));
    assert(view.innerHTML.includes('K7Q2M9XA'));
    await click({route: 'home'});
    await click({route: 'door-off'});
    assert(view.innerHTML.includes('turn off for agents on the same computer?'));
    await click({action: 'door-off'});
    const put = calls.find(call => call.url === '/app/agents/api/local-door');
    assert(put && put.body.enabled === false);
  });

  await test('listening, solstone.me on: both doors and the solstone.me flow', async () => {
    const {view, click} = await boot(baseState({
      enabled: true, status: 'on', owner_state: {address: 'k7q2m9xa.solstone.me', status: 'on'},
      local_door: {enabled: true, address: ADDRESS, listening: true},
    }));
    assert(view.innerHTML.includes('agents on the same computer</p><h2><span class="dot "></span>open to agents'));
    assert(view.innerHTML.includes('agents on other computers · solstone.me</p><h2><span class="dot "></span>open to agents'));
    await click({route: 'me'});
    assert(view.innerHTML.includes('open to your agents'));
    assert(view.innerHTML.includes('data-route="off"'));
    await click({route: 'off'});
    assert(view.innerHTML.includes('agents connected through solstone.me stop reaching your journal'));
    assert(view.innerHTML.includes('agents on the same computer keep working.'));
    await click({route: 'connect'});
    assert(view.innerHTML.includes('in Claude'));
    assert(view.innerHTML.includes('https://k7q2m9xa.solstone.me/mcp'));
    assert(view.innerHTML.includes('an agent on the same computer'));
  });

  await test('listening, solstone.me turned off: kept address, local agents keep working', async () => {
    const {view, click} = await boot(baseState({
      enabled: false, status: 'off', owner_state: {address: 'k7q2m9xa.solstone.me', status: 'off'},
      local_door: {enabled: true, address: ADDRESS, listening: true},
    }));
    assert(view.innerHTML.includes('turned off'));
    await click({route: 'me'});
    assert(view.innerHTML.includes("agents connected through solstone.me can't reach your journal while it's off. agents on the same computer keep working."));
    assertNoSolstoneMeOnlyCopy(view.innerHTML, 'turned-off solstone.me page');
  });

  console.log(`DOM CASES: ${cases} passed`);
})().catch(error => {
  console.error(error);
  process.exit(1);
});
