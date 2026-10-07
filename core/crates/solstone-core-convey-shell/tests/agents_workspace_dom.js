// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Drives the agents app's embedded script against a stubbed journal in each state the journal can
// report: no local door, a local door that is not listening (every reason), a listening door, and
// solstone.me off, turning on, on and turned off, and the network door beside the local one. The
// two ways in are chosen and switched separately. The connect dialog follows the code it made, on
// every door, until an agent connects with it or it stops working.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const root = process.argv[2] || path.join(__dirname, '..');
const html = fs.readFileSync(path.join(root, 'assets/agents/workspace.html'), 'utf8');
const script = html.slice(html.indexOf('<script>') + '<script>'.length, html.indexOf('</script>'));
let cases = 0;

const ADDRESS = 'http://127.0.0.1:7659/mcp';
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

// `journal` is what the watch endpoint answers: the open code and the agents paired in the browser.
// A test changes it between ticks to play out what happens at the journal while the dialog is open.
async function boot(state, {enable = null, pairingResponse, journal = null, activity = null, capable = false, capability = false} = {}) {
  const calls = [];
  const opened = [];
  const popups = [];
  const timers = new Map();
  let timerId = 0;
  const watched = journal || {now: '2026-09-24T12:00:00Z', today: '20260924', pairing: null, connections: []};
  const view = {
    innerHTML: '',
    listeners: {},
    addEventListener(name, listener) { (this.listeners[name] ||= []).push(listener); },
    querySelector() { return null; },
    insertAdjacentHTML(_, text) { this.innerHTML += text; },
  };
  const location = {
    origin: 'http://127.0.0.1:8080',
    assigned: null,
    set href(v) { this.assigned = v; },
    assign(v) { this.assigned = v; },
  };
  const navigator = {
    userAgent: 'Mozilla/5.0 (X11; Linux x86_64)',
    clipboard: { writeText: async () => {} },
  };
  const window = {
    location,
    top: null,
    AppServices: {escapeHtml: value => value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')},
    apiJson: async (url, options = {}) => {
      const method = options.method || 'GET';
      calls.push({url, method, body: options.body ? JSON.parse(options.body) : undefined});
      if (url === '/app/agents/api/state') return JSON.parse(JSON.stringify(state));
      if (url === '/app/agents/api/pairing' && method === 'GET') return JSON.parse(JSON.stringify(watched));
      if (url === '/app/agents/api/pairing') {
        const made = pairingResponse !== undefined ? pairingResponse : {code: 'K7Q2M9XA', expires_at: '2026-09-24T12:10:00Z', generation: 1, door: options.body ? JSON.parse(options.body).door : 'local'};
        // Like the journal, the new code is the open one from now on, in the state and in the watch.
        state.pairing = {expires_at: made.expires_at, generation: made.generation, locked: false, door: made.door ?? null};
        watched.pairing = {...state.pairing};
        return JSON.parse(JSON.stringify(made));
      }
      if (url === '/app/agents/api/local-door' || url === '/app/agents/api/lan-door' || url === '/app/agents/api/capability' || url === '/app/agents/api/byo') return {changed: true};
      if (url === '/app/agents/api/byo/account' || url === '/app/agents/api/byo/account/replace') return {changed: true};
      if (url.startsWith('/app/agents/api/activity')) return activity ? JSON.parse(JSON.stringify(activity)) : {entries: [], counts: {}, examination_complete: true};
      if (url === '/app/agents/api/enable') return {operation: method === 'POST' ? (enable || {phase: 'waiting', portal_url: 'https://services.example/consent'}) : (enable || null)};
      throw new Error(`unexpected request ${method} ${url}`);
    },
    setInterval() {},
    open(url) {
      const popup = {
        closed: false,
        opener: {},
        location: {
          replaced: null,
          replace(v) { this.replaced = v; opened.push(v); },
          set href(v) { this.replaced = v; opened.push(v); },
        },
        close() { this.closed = true; },
      };
      if (url) opened.push(url);
      popups.push(popup);
      return popup;
    },
  };
  window.window = window;
  window.top = window;
  const document = {
    getElementById: id => (id === 'agents-view' ? view : id.startsWith('a-') ? {value: ''} : null),
    querySelectorAll: () => [],
    addEventListener(name, listener) { (view.listeners[name] ||= []).push(listener); },
  };
  const context = vm.createContext({window, document, navigator, location, URL, setTimeout: callback => { timerId += 1; timers.set(timerId, callback); return timerId; }, clearTimeout(id) { timers.delete(id); }, confirm: () => true, prompt: () => null, URLSearchParams, console});
  const hostJs = fs.readFileSync(path.join(root, 'assets/static/journal-web-host.js'), 'utf8');
  const navJs = fs.readFileSync(path.join(root, 'assets/static/external-navigation.js'), 'utf8');
  vm.runInContext(hostJs, context, {filename: 'journal-web-host.js'});
  if (capable) {
    navigator.userAgent = `${navigator.userAgent} ${window.solstoneJournalWebHost.userAgentProduct}`;
  }
  if (capability) {
    window[window.solstoneJournalWebHost.javascriptCapability] = 1;
  }
  vm.runInContext(navJs, context, {filename: 'external-navigation.js'});
  vm.runInContext(script, context, {filename: 'agents-workspace.js'});
  await settle();
  const click = async dataset => {
    const target = {
      dataset,
      textContent: '',
      tagName: 'BUTTON',
      getAttribute(name) { return this.dataset ? this.dataset[name.replace(/^data-/, '')] : null; },
      closest(selector) {
        if (selector === '.modal-card') return {};
        if (selector.includes('a[data-solstone-outside]')) return null;
        return this;
      },
    };
    for (const listener of view.listeners.click || []) await listener({target, preventDefault() {}});
    await settle();
  };
  // Run the timers that are due now, as the page's own clock would, then let their requests settle.
  const tick = async () => {
    const due = [...timers.values()];
    timers.clear();
    for (const callback of due) await callback();
    await settle();
  };
  return {view, calls, click, opened, popups, location, tick, journal: watched};
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
    for (const copy of FALSE_WITH_A_DOOR) lacks(view, copy);
    await click({lane: 'own'});
    has(view, "this isn't available in this version of your journal yet.");
    lacks(view, 'set up solstone.me', 'your own must not teach solstone.me');
  });

  await test('replay notice names the connection and door without an attack claim', async () => {
    const {view} = await boot(baseState({replay_notices: [{id: 'grant-1', name: 'Claude Desktop', door: 'solstone.me', dismissed: false}]}));
    has(view, 'Claude Desktop');
    has(view, 'solstone.me');
    has(view, 'used twice');
    assert(!/intercept|attack|stolen/i.test(view.innerHTML), 'notice avoids unsupported claims');
  });

  await test('no local door, solstone.me on: connect offers only the solstone.me address, code first, no mark', async () => {
    const {view, click, calls} = await boot(baseState({...ME_ON, connections: [connection('oauth', 'g1', 'Claude', null)]}));
    has(view, 'agents can reach your journal through solstone.me.');
    has(view, 'Claude<span class="chip">solstone.me</span>');
    await click({action: 'connect'});
    has(view, 'https://k7q2m9xa.solstone.me/mcp');
    lacks(view, ADDRESS);
    const modal = view.innerHTML.slice(view.innerHTML.indexOf('<div class="modal"'));
    const first = modal.indexOf('make a pairing code first.');
    assert(first !== -1 && first < modal.indexOf('https://k7q2m9xa.solstone.me/mcp'), 'the code comes before the address');
    has(view, '<span class="n">3</span>', 'the page step follows the address');
    has(view, 'check the address starts with <code>https://k7q2m9xa.solstone.me/</code>.', 'the address is the check on solstone.me');
    assert(!/mark/i.test(view.innerHTML), 'connect never shows or mentions a mark');
    await click({action: 'new-code'});
    assert(calls.some(call => call.url === '/app/agents/api/pairing' && call.method === 'POST'));
    has(view, 'K7Q2M9XA');
  });

  await test('a Windows journal says solstone.me and a hostname are not there yet, and connect offers only this computer', async () => {
    const {view, click, calls} = await boot(baseState({...door({listening: true}), relay_available: false}));
    has(view, 'agents on this computer can reach your journal.');
    await click({lane: 'me'});
    has(view, "this isn't available on windows yet.");
    lacks(view, 'set up solstone.me', 'an unavailable relay must not offer to be set up');
    await click({lane: 'byo'});
    has(view, "this isn't available on windows yet.");
    lacks(view, 'save hostname');
    has(view, '<span class="tag ">not available</span>');
    await click({action: 'connect'});
    lacks(view, 'anywhere through solstone.me');
    has(view, ADDRESS);
    assert(!calls.some(call => call.url === '/app/agents/api/enable'), 'nothing started a solstone.me consent');
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

  await test('a Windows hostname route names its loopback address and an OpenSSH command, not a socket', async () => {
    const {view, click} = await boot(baseState({...door({listening: true}), ...byo({ingress: 'loopback', socket_path: '127.0.0.1:7661'})}));
    await click({lane: 'byo'});
    has(view, '<code>127.0.0.1:7661</code>');
    has(view, '<code>ssh -N -R 443:127.0.0.1:7661 you@your-server</code>');
    has(view, 'set GatewayPorts to yes');
    has(view, '<dt>local address</dt><dd>closed here</dd>');
    lacks(view, 'same login as your journal', 'a loopback address is not limited to one login');
    lacks(view, "isn't available on windows");
  });

  await test('a Windows hostname route blocked by another program says so', async () => {
    const {view, click} = await boot(baseState({...door({listening: true}), ...byo({ingress: 'loopback', socket_path: '127.0.0.1:7661', enabled: true, dns_verdict: 'admitted', socket_blocker: 'port_in_use', next_action: 'socket_blocked'})}));
    await click({lane: 'byo'});
    has(view, 'free the local address: another program is using it, or windows has reserved it');
  });

  await test('a ready owner hostname gets its own pairing code and never borrows another door code', async () => {
    const {view, click, calls} = await boot(baseState({...door({listening: true}), ...byo({enabled:true, dns_verdict:'admitted', socket_listening:true, certificate_active:true, next_action:'none'}), connections:[connection('oauth', 'b1', 'cloud agent', 'byo')]}));
    has(view, 'cloud agent<span class="chip">your hostname</span>');
    await click({action:'connect'});
    await click({way:'byo'});
    has(view, 'https://journal.example.com/mcp');
    await click({action:'new-code'});
    const post = calls.find(call => call.url === '/app/agents/api/pairing' && call.method === 'POST');
    assert(post && post.body.door === 'byo');
    has(view, 'data-code-door="byo"');
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

  await test('address taken: red status, and no code goes into a page at that address', async () => {
    const {view, click, calls} = await boot(baseState(door({listening: false, reason: 'port_in_use'})));
    has(view, 'class="sdot r"');
    has(view, 'a program pretending to be your journal');
    has(view, "until this clears, don't enter a pairing code on a page opened through this address, and close any that is open.");
    assert(!/mark/i.test(view.innerHTML), 'the notice never asks for a mark check');
    await click({action: 'connect'});
    lacks(view, `<code>${ADDRESS}</code><button class="btn sm" data-copy="${ADDRESS}">copy</button></div></div>`, 'connect must not offer a taken address');
    lacks(view, 'data-action="new-code"');
    lacks(view, 'data-way="local"');
    await click({action: 'new-code'});
    assert(!calls.some(call => call.url === '/app/agents/api/pairing'), 'no way offered posts nothing');
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
    has(view, 'data-disclosure="relay"', 'solstone.me must say what the relay can see before it is set up');
    has(view, 'data-disclosure="relay-detail"', 'the detail behind it must be one tap away');
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
    has(view, 'data-disclosure="relay"', 'solstone.me must say what the relay can see while it is on');
    has(view, 'data-disclosure="relay-detail"', 'the detail behind it must be one tap away');
    await click({action: 'me-off'});
    has(view, 'agents on this computer keep working.');
    has(view, 'the address and your agents are kept: turning back on uses the same address, with no new certificate while the one you have is still good');
    await click({action: 'turn-off'});
    const put = calls.find(call => call.url === '/app/agents/api/capability');
    assert(put && put.body.enabled === false);
    await click({action: 'connect'});
    await click({way: 'local'});
    has(view, `<code>${ADDRESS}</code>`);
    has(view, 'data-agent="tiles"', 'Tiles is offered on this computer');
    await click({way: 'relay'});
    has(view, 'data-agent="claude"', 'Claude is offered through solstone.me');
    lacks(view, 'data-agent="tiles"', 'Tiles reaches a journal on its own computer only');
    await click({agent: 'claude'});
    has(view, 'data-agent-hint="claude"');
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
    has(view, 'data-way="local"');
    has(view, 'data-way="lan"');
    const before = calls.filter(call => call.url === '/app/agents/api/pairing').length;
    await click({action: 'new-code'});
    assert(calls.filter(call => call.url === '/app/agents/api/pairing').length === before, 'no way chosen posts nothing');
    await click({way: 'lan'});
    const modal = () => view.innerHTML.slice(view.innerHTML.indexOf('<div class="modal"'));
    assert(modal().includes(`<code>${LAN_A}</code>`));
    assert(!modal().includes(`<code>${ADDRESS}</code>`), 'the network way offers only network addresses');
    has(view, "it may warn about the certificate unless you have trusted your journal's LAN CA there.");
    assert(!/mark/i.test(view.innerHTML), 'the network way shows no mark either');
    has(view, 'AB CD EF 01');
    await click({action: 'new-code'});
    const post = calls.find(call => call.url === '/app/agents/api/pairing' && call.method === 'POST');
    assert(post && post.method === 'POST' && post.body.door === 'lan', 'a network code is made for the network door');
    has(view, 'data-code-door="lan"');
    await click({way: 'local'});
    lacks(view, 'class="copy-code"');
    has(view, 'the open code is for your network. make a new one for this agent.');
    assert(modal().includes(`<code>${ADDRESS}</code>`));
    assert(!modal().includes(`<code>${LAN_A}</code>`), 'the local way offers no network address');
    await click({action: 'new-code'});
    const posts = calls.filter(call => call.url === '/app/agents/api/pairing' && call.method === 'POST');
    assert(posts[1] && posts[1].body.door === 'local', 'the local way makes a local code');
  });

  await test('network door alone: connect goes straight to the network way', async () => {
    const {view, click} = await boot(baseState({...door({enabled: false, listening: false, reason: 'disabled'}), ...lan({})}));
    has(view, 'agents on your network can reach your journal.');
    await click({action: 'connect'});
    has(view, 'where is this agent?');
    has(view, 'aria-checked="true" data-way="lan"');
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

  await test('the enable notice can be dismissed and the activity filter asks for activity', async () => {
    const {view, click, calls} = await boot(baseState({enabled: false, ...door({listening: true})}), {enable: {phase: 'error'}});
    await click({lane: 'me'});
    has(view, 'data-action="dismiss-enable"');
    await click({action: 'dismiss-enable'});
    lacks(view, 'data-action="dismiss-enable"');
    await click({action: 'filter-activity'});
    assert(calls.some(call => call.url.startsWith('/app/agents/api/activity') && call.method === 'GET'), 'filter asks for activity');
  });

  await test('connect dialog open pairing states and code door attribute', async () => {
    // 1. None
    const s1 = baseState({...door({listening: true}), ...lan({})});
    const {view: v1, click: c1} = await boot(s1);
    await c1({action: 'connect'});
    has(v1, 'data-open-pairing="none"');
    await c1({way: 'local'});
    has(v1, 'data-open-pairing="none"');

    // 2. Mint code on local -> shows data-code-door="local"
    await c1({action: 'new-code'});
    has(v1, 'data-code-door="local"');

    // 3. Same door open in state -> data-open-pairing="same"
    const s2 = baseState({
      ...door({listening: true}),
      ...lan({}),
      pairing: {generation: 1, expires_at: '2026-09-24T12:10:00Z', locked: false, door: 'local'},
    });
    const {view: v2, click: c2} = await boot(s2);
    await c2({action: 'connect'});
    await c2({way: 'local'});
    has(v2, 'data-open-pairing="same"');
    lacks(v2, 'class="copy-code"');

    // 4. Other door open in state -> data-open-pairing="other"
    await c2({way: 'lan'});
    has(v2, 'data-open-pairing="other"');

    // 5. Locked pairing on same door -> data-open-pairing="locked"
    const s3 = baseState({
      ...door({listening: true}),
      pairing: {generation: 1, expires_at: '2026-09-24T12:10:00Z', locked: true, door: 'local'},
    });
    const {view: v3, click: c3} = await boot(s3);
    await c3({action: 'connect'});
    has(v3, 'data-open-pairing="locked"');

    // 6. Doorless pairing in state -> data-open-pairing="doorless" even when locked
    const s4 = baseState({
      ...door({listening: true}),
      pairing: {generation: 1, expires_at: '2026-09-24T12:10:00Z', locked: true, door: null},
    });
    const {view: v4, click: c4} = await boot(s4);
    await c4({action: 'connect'});
    has(v4, 'data-open-pairing="doorless"');

    // 7. Lapsed solstone.me does not offer relay
    const s5 = baseState({
      enabled: true,
      status: 'needs_subscription',
      owner_state: {address: 'k7q2m9xa.solstone.me', status: 'needs_subscription'},
      ...door({listening: true}),
    });
    const {view: v5, click: c5} = await boot(s5);
    await c5({action: 'connect'});
    lacks(v5, 'data-way="relay"');
    has(v5, 'data-way="local"');

    // 8. Other solstone.me states do not offer relay. on and renewal do.
    const meOffered = new Set(['on', 'renewal_overdue']);
    const meStates = ['offline', 'failed', 'needs_subscription', 'not_accepted', 'update_required', 'account_changed', 'address_not_ready', 'address_refused', 'starting', 'renewal_overdue', 'on'];
    for (const status of meStates) {
      const offered = baseState({
        enabled: true,
        status,
        owner_state: {address: 'k7q2m9xa.solstone.me', status},
        ...door({listening: true}),
      });
      const {view, click} = await boot(offered);
      await click({action: 'connect'});
      if (meOffered.has(status)) has(view, 'data-way="relay"');
      else lacks(view, 'data-way="relay"');
      has(view, 'data-way="local"');
    }
    // 8b. A certificate-account hold shows the journal's own words, never "turning on".
    for (const status of ['update_required', 'account_changed', 'address_not_ready', 'address_refused']) {
      const words = `held words for ${status}`;
      const {view, click} = await boot(baseState({
        enabled: true,
        status,
        owner_state: {address: 'k7q2m9xa.solstone.me', status, detail: words},
        ...door({listening: true}),
      }));
      await click({lane: 'me'});
      has(view, words, status);
      lacks(view, 'turning on', status);
    }
    for (const [label, localDoor] of [
      ['soon', null],
      ['port', door({listening: false, reason: 'port_in_use'})],
      ['off', door({enabled: false, listening: false, reason: 'disabled'})],
      ['down', door({listening: false, reason: 'not_running'})],
      ['invalid', door({enabled: false, listening: false, reason: 'config_invalid'})],
      ['unreadable', door({listening: false, reason: 'config_unreadable'})],
    ]) {
      const state = localDoor
        ? baseState({...ME_ON, ...localDoor})
        : baseState(ME_ON);
      const {view, click} = await boot(state);
      await click({action: 'connect'});
      lacks(view, 'data-way="local"', label);
      has(view, 'data-way="relay"');
    }
    for (const phase of ['waiting', 'needs_subscription', 'error']) {
      const {view, click} = await boot(baseState({enabled: false, ...door({listening: true})}), {enable: {phase}});
      await click({action: 'connect'});
      lacks(view, 'data-way="relay"', phase);
      has(view, 'data-way="local"');
    }
    {
      const {view, click} = await boot(baseState({enabled: false, owner_state: {address: 'k7q2m9xa.solstone.me', status: 'on'}, ...door({listening: true})}));
      await click({action: 'connect'});
      lacks(view, 'data-way="relay"');
      has(view, 'data-way="local"');
    }

    // 9. Response door wins over the requested way. A response with no door has no attribute.
    const mismatch = {code: 'K7Q2M9XA', expires_at: '2026-09-24T12:10:00Z', generation: 1, door: 'local'};
    const {view: vm, click: cm} = await boot(baseState({...door({listening: true}), ...ME_ON}), {pairingResponse: mismatch});
    await cm({action: 'connect'});
    await cm({way: 'relay'});
    await cm({action: 'new-code'});
    assert(vm.innerHTML.split('class="copy-code"').length === 2, 'plaintext once');
    has(vm, 'data-code-door="local"');
    lacks(vm, 'data-code-door="relay"');
    await cm({way: 'local'});
    lacks(vm, 'class="copy-code"');
    has(vm, 'data-open-pairing="same"');

    const omitted = {code: 'K7Q2M9XA', expires_at: '2026-09-24T12:10:00Z', generation: 1};
    const {view: vo, click: co} = await boot(baseState(door({listening: true})), {pairingResponse: omitted});
    await co({action: 'connect'});
    await co({action: 'new-code'});
    assert(vo.innerHTML.split('class="copy-code"').length === 2, 'plaintext once without a door');
    lacks(vo, 'data-code-door');

    await co({closeModal: ''});
    lacks(vo, 'class="copy-code"');
    await co({action: 'connect'});
    lacks(vo, 'class="copy-code"');
    has(vo, 'data-open-pairing="doorless"');

    const {view: vc, click: cc} = await boot(baseState(door({listening: true})));
    await cc({action: 'connect'});
    await cc({action: 'new-code'});
    has(vc, 'class="copy-code"');
    await cc({closeModal: ''});
    await cc({action: 'connect'});
    lacks(vc, 'class="copy-code"');
    has(vc, 'data-open-pairing="same"');
    await cc({way: 'local'});
    lacks(vc, 'class="copy-code"');
  });

  const EVERY_DOOR = baseState({...door({listening: true}), ...lan({}), ...ME_ON, ...byo({enabled: true, socket_listening: true, certificate_active: true, dns_verdict: 'admitted', next_action: 'none'})});
  const DOOR_ADDRESS = {local: ADDRESS, lan: LAN_A, relay: 'https://k7q2m9xa.solstone.me/mcp', byo: 'https://journal.example.com/mcp'};
  const WHOLE = {read: {categories: ['transcripts', 'entities'], scope: {kind: 'whole_journal'}}};
  function grant(id, doorName, extra = {}) { return {kind: 'oauth', id, key: `oauth:${id}`, name: 'Tiles', created_at: '2026-09-24T12:01:00Z', permission: WHOLE, door: doorName, ...extra}; }
  function watching(doorName, extra = {}) {
    return {now: '2026-09-24T12:00:05Z', today: '20260924', pairing: {expires_at: '2026-09-24T12:10:00Z', generation: 1, locked: false, door: doorName}, connections: [grant('old', doorName)], ...extra};
  }

  for (const way of ['local', 'lan', 'relay', 'byo']) {
    await test(`the open dialog follows its code to the connection and its first requests (${way})`, async () => {
      const journal = watching(way);
      const activity = {entries: [{timestamp: '2026-09-24T12:01:30Z', tool: 'search', outcome: 'served', request: {arguments: {query: 'standup'}}}], counts: {served: 1}, examination_complete: true};
      const {view, click, calls, tick} = await boot(EVERY_DOOR, {journal, activity});
      await click({action: 'connect'});
      await click({way});
      await click({action: 'new-code'});
      has(view, 'data-connect-phase="waiting"');
      assert(calls.some(call => call.url === '/app/agents/api/pairing' && call.method === 'GET'), 'the dialog watches right after making the code');
      // A grant that was there before the code, or one at another door, is not this code's.
      journal.pairing = null;
      journal.connections.push(grant('elsewhere', way === 'local' ? 'relay' : 'local'));
      await tick();
      has(view, 'data-connect-phase="entered"');
      lacks(view, 'data-connect-phase="connected"');
      journal.connections.push(grant('new', way));
      await tick();
      has(view, 'data-connect-phase="connected"');
      has(view, `data-connection-door="${way}"`);
      has(view, 'Tiles is connected.');
      lacks(view, 'class="copy-code"', 'the spent code is not shown again');
      const asked = calls.filter(call => call.url.startsWith('/app/agents/api/activity'));
      assert(asked.length && asked.at(-1).url.includes('connection=oauth%3Anew') && asked.at(-1).url.includes('from=20260923'), 'it asks for this connection\'s own requests');
      has(view, 'data-first-request');
      const made = calls.findIndex(call => call.url === '/app/agents/api/pairing' && call.method === 'POST');
      assert(!calls.slice(made).some(call => call.url === '/app/agents/api/state'), 'watching never re-reads the full state');
      // It keeps following while open: what the agent may see is re-read from the stored grant.
      journal.connections = journal.connections.map(item => item.key === 'oauth:new' ? {...item, permission: {read: {categories: ['entities'], scope: {kind: 'facets', ids: ['f1']}}}} : item);
      await tick();
      has(view, 'entities in 1 chosen facet');
      const reads = () => calls.filter(call => call.url === '/app/agents/api/state').length;
      const readsBefore = reads();
      await click({closeModal: ''});
      lacks(view, 'data-connect-phase');
      assert(reads() > readsBefore, 'closing refreshes the list of agents');
      const before = calls.length;
      await tick();
      assert(!calls.slice(before).some(call => call.url === '/app/agents/api/pairing' || call.url.startsWith('/app/agents/api/activity')), 'closing stops watching');
    });
  }

  await test('a code that expires, is replaced or gets locked says so in the open dialog, with a new code one click away', async () => {
    for (const [phase, change] of [
      ['expired', journal => { journal.pairing = null; journal.now = '2026-09-24T12:10:01Z'; }],
      ['replaced', journal => { journal.pairing = {...journal.pairing, generation: 2}; }],
      ['locked', journal => { journal.pairing = {...journal.pairing, locked: true}; }],
    ]) {
      const journal = watching('local');
      const {view, click, calls, tick} = await boot(baseState(door({listening: true})), {journal});
      await click({action: 'connect'});
      await click({action: 'new-code'});
      has(view, 'data-connect-phase="waiting"');
      change(journal);
      await tick();
      has(view, `data-connect-phase="${phase}"`, phase);
      lacks(view, 'class="copy-code"', `${phase}: a code that no longer works is not shown`);
      has(view, 'data-action="new-code"', `${phase}: the way forward is in the dialog`);
      const before = calls.length;
      await tick();
      const watchedAfter = calls.slice(before).some(call => call.url === '/app/agents/api/pairing');
      if (phase === 'expired') {
        // A code used just before it expired can still finish: the dialog keeps following it for a while.
        assert(watchedAfter, 'expired: its agent can still finish, so it is still followed');
        journal.connections.push(grant('late', 'local'));
        await tick();
        has(view, 'data-connect-phase="connected"', 'expired: a late sign-in still shows as connected');
        await click({closeModal: ''});
        await click({action: 'connect'});
      } else {
        assert(!watchedAfter, `${phase}: a code that opens nothing more is not watched`);
      }
      await click({action: 'new-code'});
      has(view, 'class="copy-code"', `${phase}: a new code starts over`);
    }
  });

  await test('an expired code is followed for ten minutes past its expiry, then no longer', async () => {
    const journal = watching('local');
    const {view, click, calls, tick} = await boot(baseState(door({listening: true})), {journal});
    await click({action: 'connect'});
    await click({action: 'new-code'});
    journal.pairing = null;
    journal.now = '2026-09-24T12:10:01Z';
    await tick();
    has(view, 'data-connect-phase="expired"');
    journal.now = '2026-09-24T12:20:01Z';
    await tick();
    const before = calls.length;
    await tick();
    assert(!calls.slice(before).some(call => call.url === '/app/agents/api/pairing'), 'following ends');
  });

  await test('a code replaced by one made elsewhere says replaced, even when an agent connects with the newer code', async () => {
    const journal = watching('local');
    const {view, click, tick} = await boot(baseState(door({listening: true})), {journal});
    await click({action: 'connect'});
    await click({action: 'new-code'});
    journal.pairing = {...journal.pairing, generation: 2};
    journal.connections.push(grant('other', 'local', {name: 'Someone else'}));
    await tick();
    has(view, 'data-connect-phase="replaced"');
    lacks(view, 'Someone else is connected.');
  });

  await test('the agents paired before the code are read before it is made', async () => {
    const journal = watching('local');
    const {view, click, calls, tick} = await boot(baseState(door({listening: true})), {journal});
    await click({action: 'connect'});
    await click({action: 'new-code'});
    const get = calls.findIndex(call => call.url === '/app/agents/api/pairing' && call.method === 'GET');
    const post = calls.findIndex(call => call.url === '/app/agents/api/pairing' && call.method === 'POST');
    assert(get !== -1 && get < post, 'the snapshot comes first');
    has(view, 'class="copy-code"', 'the code shows without waiting for the watch');
    journal.pairing = null;
    await tick();
    lacks(view, 'data-connect-phase="connected"', 'an agent paired before the code is not this one');
  });

  await test('the dialog names the next step for the agent being connected', async () => {
    const {view, click} = await boot(EVERY_DOOR);
    await click({action: 'connect'});
    await click({way: 'local'});
    has(view, 'data-agent-hint="none"');
    await click({agent: 'tiles'});
    has(view, 'data-copy="/mcp-auth solstone__journal"');
    await click({agent: 'codex'});
    has(view, `data-copy="codex mcp add journal --url &quot;${ADDRESS}&quot;"`);
    await click({agent: 'gemini-cli'});
    has(view, `data-copy="gemini mcp add --transport http journal &quot;${ADDRESS}&quot;"`);
    has(view, '/mcp auth journal');
    await click({agent: 'grok-build'});
    has(view, `data-copy="grok mcp add --scope project --transport http journal &quot;${ADDRESS}&quot;"`);
    has(view, 'type <code>/mcps</code>');
    await click({agent: 'codex'});
    await click({way: 'lan'});
    lacks(view, 'data-agent="tiles"');
    lacks(view, 'data-agent="gemini-cli"');
    lacks(view, 'data-agent="grok-build"');
    has(view, `data-copy="codex mcp add journal --url &quot;${LAN_A}&quot;"`, 'the choice carries across doors that offer it');
    await click({agent: 'claude-code'});
    has(view, `data-copy="claude mcp add --transport http journal &quot;${LAN_A}&quot;"`);
    for (const way of ['relay', 'byo']) {
      await click({way});
      has(view, `data-copy="claude mcp add --transport http journal &quot;${DOOR_ADDRESS[way]}&quot;"`);
      has(view, 'data-agent="chatgpt"');
      has(view, 'data-agent="gemini-cli"');
      has(view, 'data-agent="grok-build"');
    }
  });

  await test('capable host turnOn navigates top location and does not call window.open', async () => {
    const {view, click, calls, popups, location} = await boot(baseState(door({listening: true})), {capable: true});
    await click({lane: 'me'});
    await click({action: 'turn-on'});
    assert(calls.some(call => call.url === '/app/agents/api/enable' && call.method === 'POST'));
    assert.strictEqual(popups.length, 0, 'capable host does not open popups');
    assert.strictEqual(location.assigned, 'https://services.example/consent');
    has(view, 'waiting for you to approve solstone.me in the services portal.');
    has(view, 'data-solstone-outside', 'approve link has data-solstone-outside attribute');
  });

  await test('clicking rendered approve anchor assigns portal url on capable host and does not open window', async () => {
    const approveAnchor = {
      tagName: 'A',
      dataset: { solstoneOutside: '' },
      getAttribute(name) {
        if (name === 'href') return 'https://services.example/consent';
        if (name === 'data-solstone-outside') return '';
        return null;
      },
      closest(selector) {
        if (selector === 'a[data-solstone-outside]') return this;
        if (selector === '.modal-card') return null;
        return this;
      },
    };

    // 1. Browser mode
    const browserSession = await boot(baseState(door({listening: true})), {capable: false});
    let defaultPrevented = false;
    for (const listener of browserSession.view.listeners.click || []) {
      await listener({ target: approveAnchor, preventDefault() { defaultPrevented = true; } });
    }
    assert.strictEqual(browserSession.location.assigned, null, 'browser mode does not assign location');
    assert.strictEqual(browserSession.popups.length, 0, 'browser mode does not open window');
    assert.strictEqual(defaultPrevented, false, 'browser mode does not prevent default');

    // 2. Capable mode
    const capableSession = await boot(baseState(door({listening: true})), {capable: true});
    defaultPrevented = false;
    for (const listener of capableSession.view.listeners.click || []) {
      await listener({ target: approveAnchor, preventDefault() { defaultPrevented = true; } });
    }
    assert.strictEqual(capableSession.location.assigned, 'https://services.example/consent', 'capable mode assigns portal url');
    assert.strictEqual(capableSession.popups.length, 0, 'capable mode does not open window');
    assert.strictEqual(defaultPrevented, true, 'capable mode prevents default');
  });

  console.log(`DOM CASES: ${cases} passed`);
})().catch(error => { console.error(error); process.exit(1); });
