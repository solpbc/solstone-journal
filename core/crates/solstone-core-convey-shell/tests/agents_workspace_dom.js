// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Drives the agents app's embedded script against a stubbed journal in each state the journal can
// report: no local door, a local door that is not listening (every reason), a listening door, and
// solstone.me off, turning on, on and turned off, and the network door beside the local one. The
// two ways in are chosen and switched separately.

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
  mark: {
    icon1: {svg: '<path d="M12 6v16" />', color: {name: 'teal', hex: '#0f766e'}, rot: 0},
    icon2: {svg: '<circle cx="5" cy="5" r="3" />', color: {name: 'amber', hex: '#b45309'}, rot: 45},
    words: ['afoot', 'unfixed'],
  },
};
const ME_ON = {enabled: true, status: 'on', owner_state: {address: 'k7q2m9xa.solstone.me', status: 'on'}};
const FALSE_WITH_A_DOOR = ['nothing is reachable until you turn it on', 'this is off until you turn it on', 'stay connected on paper'];

function baseState(overrides = {}) {
  return {enabled: false, status: 'off', owner_state: null, certificate: {current: false}, connections: [], facets: [], pairing: null, ...overrides};
}
function door(extra) { return {local_door: {enabled: true, address: ADDRESS, ...extra}}; }
const LAN_A = 'https://192.168.4.47:7660/mcp';
const LAN_B = 'https://[fd7a:115c:a1e0::5]:7660/mcp';
const FP = 'AB:CD:EF:01';
const BYO_URI = 'https://acme-v02.api.letsencrypt.org/acme/acct/12345';
function byo(extra = {}) {
  return {byo: {hostname: 'journal.example.com', enabled: false, account_uri: BYO_URI, dns_verdict: 'unchecked', socket_listening: false, certificate_active: false, socket_path: '/home/owner/journal/mcp-endpoint/byo/ingress.sock', next_action: 'publish_caa', ...extra}};
}
function lan(extra, addresses = [{url: LAN_A, listening: true}, {url: LAN_B, listening: true}]) {
  return {lan_door: {enabled: true, listening: true, port: 7660, fingerprint: FP, addresses, ...extra}};
}
function connection(kind, id, name, doorName) {
  return {kind, id, key: `${kind}:${id}`, name, door: doorName, created_at: '2026-09-24T12:00:00Z', permission: null, requests_this_week: 2, last_request_at: null, activity_complete: true};
}

async function boot(state, {identityFails = false, enable = null} = {}) {
  const calls = [];
  const opened = [];
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
      if (url === '/app/network/api/identity') { if (identityFails) throw new Error('unavailable'); return MARK; }
      if (url === '/app/agents/api/pairing') return {code: 'K7Q2M9XA', expires_at: '2026-09-24T12:10:00Z', generation: 1};
      if (url === '/app/agents/api/local-door' || url === '/app/agents/api/lan-door' || url === '/app/agents/api/capability' || url === '/app/agents/api/byo') return {changed: true};
      if (url === '/app/agents/api/byo/account' || url === '/app/agents/api/byo/account/replace') return {changed: true};
      if (url === '/app/agents/api/enable') return method === 'POST' ? {operation: enable || {phase: 'waiting', portal_url: 'https://services.example/consent'}} : {operation: null};
      throw new Error(`unexpected request ${method} ${url}`);
    },
    setInterval() {},
    open(url) { opened.push(url); },
  };
  const document = {getElementById: id => (id === 'agents-view' ? view : null), querySelectorAll: () => []};
  const context = vm.createContext({window, document, navigator: {clipboard: {writeText: async () => {}}}, setTimeout: () => 0, clearTimeout() {}, confirm: () => true, prompt: () => null, URLSearchParams, console});
  vm.runInContext(script, context, {filename: 'agents-workspace.js'});
  await settle();
  const click = async dataset => {
    const target = {dataset, textContent: '', tagName: 'BUTTON', closest(selector) { return selector === '.modal-card' ? {} : this; }};
    for (const listener of view.listeners.click || []) await listener({target, preventDefault() {}});
    await settle();
  };
  return {view, calls, click, opened};
}
async function settle() { for (let i = 0; i < 6; i += 1) await new Promise(resolve => setImmediate(resolve)); }
function has(view, text, why) { assert(view.innerHTML.includes(text), why || `missing: ${text}`); }
function lacks(view, text, why) { assert(!view.innerHTML.includes(text), why || `unexpected: ${text}`); }
async function test(name, body) {
  try { await body(); } catch (error) { error.message = `${name}: ${error.message}`; throw error; }
  cases += 1;
}

(async () => {
  await test('no local door: your own is coming later, solstone.me is the live way in', async () => {
    const {view, click} = await boot(baseState());
    has(view, 'coming later');
    has(view, 'set up solstone.me →');
    has(view, 'neither way is on right now.');
    for (const copy of FALSE_WITH_A_DOOR) lacks(view, copy);
    await click({lane: 'own'});
    has(view, "this isn't available in this version of your journal yet.");
    lacks(view, 'set up solstone.me', 'your own must not teach solstone.me');
  });

  await test('no local door, solstone.me on: connect offers only the solstone.me address, beside the mark', async () => {
    const {view, click, calls} = await boot(baseState({...ME_ON, connections: [connection('oauth', 'g1', 'Claude', null)]}));
    has(view, 'agents can reach your journal through solstone.me.');
    has(view, 'Claude<span class="chip">solstone.me</span>');
    await click({action: 'connect'});
    has(view, 'https://k7q2m9xa.solstone.me/mcp');
    lacks(view, ADDRESS);
    has(view, 'aria-label="teal, amber · afoot unfixed"');
    await click({action: 'new-code'});
    assert(calls.some(call => call.url === '/app/agents/api/pairing' && call.method === 'POST'));
    has(view, 'K7Q2M9XA');
  });

  await test('your hostname shows the account CAA pin, protected socket and separate local readiness', async () => {
    const {view, click, calls} = await boot(baseState({...door({listening: true}), ...byo()}));
    await click({lane: 'byo'});
    has(view, 'journal.example.com');
    has(view, `accounturi=${BYO_URI}; validationmethods=tls-alpn-01`);
    has(view, '/home/owner/journal/mcp-endpoint/byo/ingress.sock');
    has(view, 'these checks cannot show whether the public route reaches this journal');
    has(view, 'turn on public route');
    await click({action: 'byo-on'});
    assert(calls.some(call => call.url === '/app/agents/api/byo' && call.method === 'PUT' && call.body.enabled === true));
  });

  await test('a ready owner hostname gets its own pairing code and never borrows another door code', async () => {
    const {view, click, calls} = await boot(baseState({...door({listening: true}), ...byo({enabled:true, dns_verdict:'admitted', socket_listening:true, certificate_active:true, next_action:'none'}), connections:[connection('oauth', 'b1', 'cloud agent', 'byo')]}));
    has(view, 'cloud agent<span class="chip">your hostname</span>');
    await click({action:'connect'});
    await click({way:'byo'});
    has(view, 'https://journal.example.com/mcp');
    await click({action:'new-code'});
    const post = calls.find(call => call.url === '/app/agents/api/pairing');
    assert(post && post.body.door === 'byo');
    has(view, 'this code works only at your hostname');
  });

  const reasons = {
    disabled: {enabled: false, tag: 'off', text: "agents on this computer can't reach your journal. the ones you connected are kept", action: 'own-on'},
    config_invalid: {enabled: false, tag: 'needs attention', text: 'turning this on replaces that value.', action: 'own-on'},
    config_unreadable: {enabled: true, tag: 'needs attention', text: "your journal can&#39;t read its settings", alt: "your journal can't read its settings", action: 'refresh'},
    not_running: {enabled: true, tag: 'not running', text: "the part of your journal that serves agents isn't running", action: 'refresh'},
    port_in_use: {enabled: true, tag: 'needs attention', text: 'something else is using this address.', action: 'refresh'},
  };
  for (const [reason, expected] of Object.entries(reasons)) {
    await test(`your own not listening (${reason})`, async () => {
      const {view} = await boot(baseState(door({enabled: expected.enabled, listening: false, reason})));
      has(view, `<span class="tag ${reason === 'port_in_use' ? 'bad' : expected.tag === 'off' ? '' : 'warn'}">${expected.tag}</span>`);
      assert(view.innerHTML.includes(expected.text) || (expected.alt && view.innerHTML.includes(expected.alt)), `missing panel text for ${reason}`);
      has(view, `data-action="${expected.action}"`);
      lacks(view, 'set up solstone.me', 'your own must not teach solstone.me');
      for (const copy of FALSE_WITH_A_DOOR) lacks(view, copy);
    });
  }

  await test('address taken: red status, the mark, and "or none"', async () => {
    const {view, click} = await boot(baseState(door({listening: false, reason: 'port_in_use'})));
    has(view, 'class="sdot r"');
    has(view, 'a program pretending to be your journal');
    has(view, 'aria-label="teal, amber · afoot unfixed"');
    has(view, 'if it shows a different mark, or none, close that page.');
    await click({action: 'connect'});
    lacks(view, `<code>${ADDRESS}</code><button class="btn sm" data-copy="${ADDRESS}">copy</button></div></div>`, 'connect must not offer a taken address');
    has(view, 'neither way is on right now.');
  });

  await test('address taken without a readable mark still says to check it', async () => {
    const {view} = await boot(baseState(door({listening: false, reason: 'port_in_use'})), {identityFails: true});
    has(view, "check the page asking for it shows your journal's mark. if it doesn't, close that page.");
    lacks(view, 'class="journal-mark"');
  });

  await test('your own on: address, agents by lane, turn off through a confirm', async () => {
    const state = baseState({...door({listening: true}), connections: [connection('oauth', 'g1', 'tiles', 'local'), connection('oauth', 'g2', 'Claude', 'relay'), connection('bearer', 't1', 'my script', null)]});
    const {view, click, calls} = await boot(state);
    has(view, 'agents on this computer can reach your journal.');
    has(view, `data-copy="${ADDRESS}"`);
    has(view, 'tiles<span class="chip">this computer</span>');
    has(view, 'Claude<span class="chip">solstone.me</span>');
    has(view, '<strong>my script</strong><small>a key you created · ');
    await click({action: 'own-off'});
    has(view, 'turn off agents on this computer?');
    await click({action: 'door-off'});
    const put = calls.find(call => call.url === '/app/agents/api/local-door');
    assert(put && put.method === 'PUT' && put.body.enabled === false);
    await click({connection: 'oauth:g1'});
    has(view, '<span class="chip">this computer</span>paired in your browser');
    has(view, 'data-revoke="oauth:g1"');
  });

  await test('your own off turns back on through its own switch', async () => {
    const {click, calls} = await boot(baseState(door({enabled: false, listening: false, reason: 'disabled'})));
    await click({action: 'own-on'});
    const put = calls.find(call => call.url === '/app/agents/api/local-door');
    assert(put && put.body.enabled === true);
  });

  await test('solstone.me set up: the consent hand-off, then approve', async () => {
    const {view, click, calls, opened} = await boot(baseState(door({listening: true})));
    await click({lane: 'me'});
    has(view, 'set up solstone.me →');
    has(view, 'public certificate logs');
    has(view, 'could get another valid certificate');
    lacks(view, ADDRESS, 'solstone.me must not teach your own');
    await click({action: 'turn-on'});
    assert(calls.some(call => call.url === '/app/agents/api/enable' && call.method === 'POST'));
    assert.deepStrictEqual(opened, ['https://services.example/consent']);
    has(view, 'waiting for you to approve solstone.me in the services portal.');
    has(view, '<span class="tag warn">setting up</span>');
  });

  await test('solstone.me turning on shows its three legs', async () => {
    const {view, click} = await boot(baseState({enabled: true, status: 'turning_on', owner_state: {status: 'turning_on', address_leg: 'done', certificate_leg: 'in_progress', relay_leg: 'waiting'}, ...door({listening: true})}));
    await click({lane: 'me'});
    has(view, 'getting your journal its address');
    has(view, 'connecting to the relay');
    has(view, '<span class="tag warn">turning on</span>');
  });

  await test('both on: status names both, turning solstone.me off keeps local agents', async () => {
    const {view, click, calls} = await boot(baseState({...ME_ON, ...door({listening: true})}));
    has(view, 'agents on this computer, and anywhere through solstone.me, can reach your journal.');
    await click({lane: 'me'});
    has(view, 'https://k7q2m9xa.solstone.me/mcp');
    has(view, 'could get another valid certificate');
    await click({action: 'me-off'});
    has(view, 'agents on this computer keep working.');
    has(view, 'the address and your agents are kept: turning back on uses the same address, with no new certificate');
    await click({action: 'turn-off'});
    const put = calls.find(call => call.url === '/app/agents/api/capability');
    assert(put && put.body.enabled === false);
    await click({action: 'connect'});
    has(view, `<code>${ADDRESS}</code>`);
    has(view, 'in Claude, add it as a custom connector');
  });

  await test('solstone.me turned off turns back on without a new consent', async () => {
    const {view, click, calls} = await boot(baseState({enabled: false, status: 'off', owner_state: {address: 'k7q2m9xa.solstone.me', status: 'off'}, ...door({listening: true})}));
    await click({lane: 'me'});
    has(view, "agents connected through solstone.me can't reach your journal while it's off.");
    for (const copy of FALSE_WITH_A_DOOR) lacks(view, copy);
    await click({action: 'turn-on'});
    const put = calls.find(call => call.url === '/app/agents/api/capability');
    assert(put && put.body.enabled === true);
    assert(!calls.some(call => call.url === '/app/agents/api/enable' && call.method === 'POST'));
  });

  await test('network door off: a preview of its addresses and its own switch', async () => {
    const {view, click, calls} = await boot(baseState({...door({listening: true}), ...lan({enabled: false, listening: false, reason: 'disabled', fingerprint: undefined}, [{url: LAN_A, listening: false}])}));
    has(view, 'agents on your wifi or VPN can too, once you turn that on.');
    has(view, '<span class="tag on">on</span>', 'the local door keeps the lane on');
    has(view, 'agents on your network');
    has(view, "while it's on, any device on a network this computer is on can reach its addresses");
    has(view, `<code>${LAN_A}</code>`);
    lacks(view, `data-copy="${LAN_A}"`, 'an address that is not open is never copyable');
    lacks(view, 'certificate fingerprint');
    await click({action: 'lan-on'});
    const put = calls.find(call => call.url === '/app/agents/api/lan-door');
    assert(put && put.method === 'PUT' && put.body.enabled === true);
  });

  await test('network door on: addresses, the fingerprint as detail, its agents, and a confirmed turn-off', async () => {
    const state = baseState({...door({listening: true}), ...lan({ca_fingerprint: '12:34:56:78'}), connections: [connection('oauth', 'n1', 'tiles on the laptop', 'lan')]});
    const {view, click, calls} = await boot(state);
    has(view, 'agents on this computer can reach your journal. agents on your network can reach your journal too.');
    has(view, `data-copy="${LAN_A}"`);
    has(view, `data-copy="${LAN_B}"`);
    has(view, '<details><summary>certificate fingerprint</summary>');
    has(view, 'AB CD EF 01');
    has(view, '/app/agents/api/lan-door/ca.pem');
    has(view, '12 34 56 78');
    has(view, 'NODE_EXTRA_CA_CERTS');
    has(view, 'CODEX_CA_CERTIFICATE');
    has(view, "a key you created doesn't work at these addresses");
    has(view, 'tiles on the laptop<span class="chip">your network</span>');
    await click({action: 'lan-off'});
    has(view, 'turn off agents on your network?');
    has(view, 'agents on this computer keep working.');
    await click({action: 'lan-door-off'});
    const put = calls.find(call => call.url === '/app/agents/api/lan-door');
    assert(put && put.body.enabled === false);
  });

  await test('connect with both doors open: pick where the agent is, then a code for that door only', async () => {
    const {view, click, calls} = await boot(baseState({...door({listening: true}), ...lan({})}));
    await click({action: 'connect'});
    has(view, 'where is this agent?');
    has(view, 'pick where the agent is first. the pairing code works only for the one you pick.');
    lacks(view, 'make a pairing code');
    await click({way: 'lan'});
    const modal = () => view.innerHTML.slice(view.innerHTML.indexOf('<div class="modal"'));
    assert(modal().includes(`<code>${LAN_A}</code>`));
    assert(!modal().includes(`<code>${ADDRESS}</code>`), 'the network way offers only network addresses');
    has(view, "it may warn about the certificate unless you have trusted your journal's LAN CA there.");
    has(view, 'aria-label="teal, amber · afoot unfixed"', 'the mark is the check on the network way too');
    has(view, 'AB CD EF 01');
    await click({action: 'new-code'});
    const post = calls.find(call => call.url === '/app/agents/api/pairing');
    assert(post && post.method === 'POST' && post.body.door === 'lan', 'a network code is made for the network door');
    has(view, 'this code works only for an agent on your network.');
    await click({way: 'other'});
    has(view, 'the open code is for your network. make a new one for this agent.');
    assert(modal().includes(`<code>${ADDRESS}</code>`));
    assert(!modal().includes(`<code>${LAN_A}</code>`), 'the other way offers no network address');
    await click({action: 'new-code'});
    const posts = calls.filter(call => call.url === '/app/agents/api/pairing');
    assert.strictEqual(posts[1].body, undefined, 'the other way makes an unbound code');
  });

  await test('network door alone: connect goes straight to the network way', async () => {
    const {view, click} = await boot(baseState({...door({enabled: false, listening: false, reason: 'disabled'}), ...lan({})}));
    has(view, 'agents on your network can reach your journal.');
    await click({action: 'connect'});
    lacks(view, 'where is this agent?');
    assert(view.innerHTML.slice(view.innerHTML.indexOf('<div class="modal"')).includes(`<code>${LAN_A}</code>`));
  });

  await test('one network address taken while another serves: red status, named address, no copy for it', async () => {
    const {view} = await boot(baseState({...door({listening: true}), ...lan({}, [{url: LAN_A, listening: true}, {url: LAN_B, listening: false, reason: 'port_in_use'}])}));
    has(view, 'class="sdot r"');
    has(view, "something else on this computer is using one of your journal's addresses for agents on your network.");
    has(view, '<span class="tag bad">needs attention</span>');
    has(view, `something else on this computer is using ${LAN_B}.`);
    has(view, '<span class="tag bad">something else is using it</span>');
    lacks(view, `data-copy="${LAN_B}"`);
    has(view, `data-copy="${LAN_A}"`);
  });

  const lanReasons = {
    no_address: "on, but this computer isn't on a network your journal can use",
    enumeration_failed: "on, but your journal can't read this computer's addresses",
    tls_unavailable: "on, but your journal's certificate isn't ready",
    bind_failed: "on, but your journal couldn't open its addresses",
    retrying: "on, reopening your journal's addresses",
    not_running: 'on, not running right now',
    config_invalid: "off until your journal's settings are fixed",
  };
  for (const [reason, sub] of Object.entries(lanReasons)) {
    await test(`network door not listening (${reason})`, async () => {
      const {view} = await boot(baseState({...door({listening: true}), ...lan({listening: false, reason, fingerprint: undefined, enabled: reason !== 'config_invalid'}, [])}));
      assert(view.innerHTML.includes(sub) || view.innerHTML.includes(sub.replaceAll("'", '&#39;')), `missing: ${sub}`);
      lacks(view, 'certificate fingerprint');
      if (reason !== 'config_invalid' && reason !== 'no_address') has(view, "your journal's addresses on your network aren't open right now.");
    });
  }

  console.log(`DOM CASES: ${cases} passed`);
})().catch(error => { console.error(error); process.exit(1); });
