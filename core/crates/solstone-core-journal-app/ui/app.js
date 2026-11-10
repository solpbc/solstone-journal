// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The journal app's window. Every word the owner reads is in COPY; the Mac
// journal app's words are kept wherever they say the same thing here.

import { renderMarkCard, markIconImages, sameMark, validMark, displayableMark } from './mark.js';

const COPY = {
  panes: {
    home: 'home',
    journal: 'location',
    run: 'run state',
    devices: 'devices',
    backup: 'backup',
    startup: 'startup',
    updates: 'updates',
  },
  adminTerminal: 'open a terminal',
  quit: 'quit journal',
  quitting: 'stopping your journal…',
  homeTitle: 'your journal, at a glance',
  homeLine: 'this app keeps your journal running on this PC. your journal itself opens in your browser.',
  openJournal: 'open your journal',
  start: 'start',
  stop: 'stop',
  restart: 'restart',
  runStateLink: 'run state →',
  confirmedLine: 'this is your journal',
  display: {
    starting: 'starting…',
    running: 'running',
    stopping: 'stopping…',
    stopped: 'stopped',
    'not-running': 'not running',
    'other-account': 'not running',
  },
  otherAccountTitle: 'another account\'s journal is running on this computer',
  otherAccountBody: 'one journal runs on a computer at a time, and another account here is running its own. yours can start once that account stops its journal or logs off.',
  health: 'health',
  healthy: 'healthy',
  unknown: 'unknown',
  location: 'location',
  showInExplorer: 'show in File Explorer',
  copyAbout: 'copy',
  copiedAbout: 'copied',
  copyAboutFailed: "couldn't copy. select the text and copy it.",
  diskUsed: 'disk used',
  browserTitle: 'finish setting up in your browser',
  browserLine: 'this window moves on when you finish.',
  openBrowser: 'open the setup page',
  browserFailed: "your browser didn't open.",
  modelsMissing: 'repair or reinstall the journal app, then restart your journal.',
  modelsDownloading: 'downloading…',
  devicesTitle: 'devices',
  yourDevices: 'your devices',
  devicesLoading: 'loading devices…',
  devicesEmptyTitle: 'no devices yet',
  devicesEmptyBody: 'pair a device and it joins your journal.',
  devicesNotRunningTitle: "your journal isn't running",
  devicesNotRunningBody: 'start the journal, then try again.',
  devicesLoadFailed: "couldn't load your devices",
  addDevice: 'pair a device',
  unnamedDevice: 'unnamed device',
  peerJournals: 'peer journals',
  neverConnected: 'never connected',
  lastSeenJustNow: 'last seen just now',
  lastSeenUnknown: 'last seen unknown',
  lastSeen: relative => `last seen ${relative}`,
  paired: date => `paired ${date}`,
  remove: 'remove',
  removeTitle: name => `remove ${name}?`,
  removeBody: name => `${name} loses access to your journal. you can pair it again later.`,
  removeFailed: "couldn't remove this device",
  cancel: 'cancel',
  close: 'close',
  pairThisPc: 'pair the solstone app on this PC',
  pairingOpening: 'opening pairing…',
  pairingInstructions: 'scan this code or open the pairing link on the device you want to add.',
  pairingThisPc: "this pairing link works only for the solstone app on this PC. it's copied, so paste it into the solstone app to pair.",
  pairingThisPcCopy: 'this pairing link works only for the solstone app on this PC. copy it, then paste it into the solstone app to pair.',
  pairingCode: 'pairing code',
  copyLink: 'copy pairing link',
  copied: 'copied ✓',
  countdown: seconds => `expires in ${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`,
  linkExpiredTitle: 'pairing link expired',
  linkExpiredBody: 'open a fresh pairing link to pair a device.',
  openFreshLink: 'open a fresh pairing link',
  pairingFailedTitle: "couldn't open pairing",
  pairedHeading: label => `"${label}" can now reach your journal`,
  pairedHeadingUnnamed: 'a new device can now reach your journal',
  pairedNote: "on that device, check it shows this same mark, then answer there. didn't pair it?",
  pairedNoteAnywhere: "on that device, check it shows this same mark, then answer there. it can reach your journal from anywhere. didn't pair it?",
  pairedNoteThisPc: "if the solstone app asks, check it shows this same mark, then answer there. didn't pair it?",
  pairedDone: 'done',
  pairedUnpair: 'unpair this device',
  pairedRemoved: "unpaired. the device can't reach your journal until it's paired again.",
  pairedRemoveFailed: "couldn't unpair this device. try again, or remove it under devices.",
  pairingClosed: "your journal is closed to devices on your network, so another device can't reach it directly yet. open your journal to devices on your network, or turn on your private network to pair from anywhere. the solstone app on this PC can still pair.",
  relaySetup: 'turn on your private network in your journal →',
  // whether the journal is open to devices on the network (the journal's own words)
  network: {
    closed: 'closed to devices on your network',
    closedNote: 'the solstone app on this PC can still pair from here, and so can another device once your private network is on. to pair a phone or another computer over your own network, open your journal to it.',
    windowsAsks: 'windows may then ask whether journal can use the network, and on a standard account an administrator has to allow it.',
    open: 'open to devices on your network',
    openNote: 'devices on the same network can pair with your journal directly.',
    agentsNote: 'your journal stays open to devices on your network while "agents on your network" is on.',
    agentsClosesNote: 'it closes to them when you turn that off in agents, in your journal.',
    firewallNote: "if windows didn't ask and a device still can't reach your journal, an earlier choice in windows may be blocking journal. an administrator can allow it in Windows Security, under Firewall & network protection › Allow an app through firewall.",
    notReachable: "devices can't reach your journal right now.",
    openCta: 'open to devices on your network',
    closeCta: 'close to devices on your network',
    closeConfirm: 'close your journal to devices on your network? any device connected over your network now will disconnect.',
    failed: "couldn't change whether your journal is open to devices on your network. try again.",
  },
  tryAgain: 'try again',
  backupTitle: 'backup',
  backupLine: 'backup keeps your journal safe. set it up from your journal.',
  openBackup: 'open backup',
  startupTitle: 'startup',
  startAtSignIn: 'start the journal when you sign in',
  // first run
  locationTitle: "let's create your journal on this PC",
  chooseLocation: 'choose',
  continueButton: 'continue',
  continueInSolstone: 'continue in the solstone app',
  downloadSolstone: 'download the solstone app for windows',
  journalFound: 'journal found',
  setupTitle: 'setting up your journal',
  setupSubtitle: 'this can take a minute.',
  setupStopped: "setup stopped before it finished.",
  whatHappened: 'what happened',
  steps: {
    doctor: 'checking this PC',
    journal: 'preparing your journal',
    install_models: 'downloading models',
    service: 'starting your journal',
    brain: 'setting up the model',
  },
  adoptLandingLine: 'nothing moved. your journal was always here.',
  setup: {
    band: 'your journal',
    fitsTitle: 'what this sets up',
    journalTerm: 'your journal',
    journalText: 'lives on this PC and keeps everything that goes into it.',
    appTerm: 'the solstone app',
    appText: 'runs on this PC. your microphone, screen and computer sound each stay off until you turn them on.',
    thinkingTerm: 'thinking',
    thinkingText: 'helps your journal make sense of what it keeps. you choose where during setup, or later.',
    signIn: 'start automatically after you sign in to this PC.',
    start: 'start setup',
    folderTitle: 'where your journal lives',
    folderText: 'everything your journal keeps stays in this folder, on this PC.',
    folderLabel: 'journal folder',
    thinkingTitle: 'where does the thinking happen?',
    thinkingLead: 'your journal stays on this PC whatever you choose. this only decides where thinking happens.',
    onThisPc: 'on this PC',
    localDesc: 'model requests stay on this PC. continue starts the download.',
    step: n => `step ${n} of 8`,
  },
  // updates (the Mac app's update words)
  updates: {
    header: version => `journal ${version}`,
    checkNow: 'check now',
    checkAgain: 'check again',
    checking: 'checking…',
    checkingWithDownloaded: downloaded => `version ${downloaded} is downloaded. checking for a newer one…`,
    neverChecked: 'never checked for updates',
    justNow: 'just now',
    upToDate: 'the journal is up to date',
    lastChecked: relative => `last checked ${relative}`,
    availableTitle: target => `version ${target} is available`,
    availableSubtitle: 'the journal app will download it in the background when this connection allows it.',
    deferral: {
      metered: 'this connection is metered, so the download is waiting.',
      roaming: 'this connection is roaming, so the download is waiting.',
      restricted: 'this connection does not allow a background download, so it is waiting.',
      unknown: 'the connection is not known to allow a background download, so it is waiting.',
    },
    download: 'download',
    downloadingTitle: target => `downloading ${target}`,
    downloadingRetainsDownloaded: downloaded => `version ${downloaded} stays downloaded until the new one finishes.`,
    readyTitle: downloaded => `version ${downloaded} is downloaded. the journal installs it the next time the journal app starts.`,
    install: 'install now',
    installingTitle: downloaded => `installing ${downloaded}`,
    installingSubtitle: 'the journal app is applying the update.',
    failureCheckTitle: 'the check failed',
    failureCheckBody: 'the journal could not finish the check.',
    failureCheckRemains: 'the downloaded update is still here.',
    retryCheck: 'retry check',
    failureDownloadTitle: 'the download failed',
    failureDownloadBody: 'the download did not finish.',
    failureDownloadRetained: downloaded => `version ${downloaded} is still downloaded.`,
    retryDownload: 'retry download',
    failureStopTitle: 'the update did not start',
    failureStopBody: 'the update could not start. the downloaded update is still here.',
    failureInstallTitle: 'the update did not finish',
    failureInstallBody: 'the downloaded update is still here. the journal installs it the next time the journal app starts.',
    failureInstallMissing: 'the downloaded update is no longer available. check for an update again.',
    unavailableTitle: 'updates are unavailable',
    unavailableSubtitle: 'this build cannot check for updates on its own.',
    automatic: 'automatic updates',
    autoCheck: 'check for updates automatically',
    autoDownload: 'download updates in the background',
    howOften: 'how often',
    day: 'every day',
    week: 'every week',
    month: 'every month',
    privacy: 'no usage data is sent. a check fetches the version list. a background download runs only on an unrestricted connection.',
  },
};

// --- the channel to the app ----------------------------------------------

const waiting = new Map();
let nextId = 1;

function call(cmd, args = {}) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    waiting.set(id, { resolve, reject, cmd });
    window.chrome.webview.postMessage(JSON.stringify({ id, cmd, args }));
  });
}

const listeners = { setup: [], update: [], shown: [], flow: [], 'sign-in-launch': [] };

listeners['sign-in-launch'].push(data => {
  state.message = data.error || data.message || 'sign-in launch failed';
  render();
});

window.journalApp = {
  receive(message) {
    if (message.type === 'reply') {
      const entry = waiting.get(message.id);
      if (!entry) return;
      waiting.delete(message.id);
      if (entry.cmd === 'flow' && state.init?.setup_sequence_enabled === true) {
        if (message.ok) {
          renderFlow(message.value);
        } else {
          const text = typeof message.error === 'string' ? message.error : String(message.error ?? '');
          if (state.flow) renderFlow({ ...state.flow, busy: false, error: text });
          else $('loading').replaceChildren(el('p', { class: 'error' }, document.createTextNode(text)));
        }
      }
      if (message.ok) {
        entry.resolve(message.value);
      } else {
        // A journal refusal keeps its status and reason code.
        const error = new Error(message.error);
        error.status = message.status ?? null;
        error.code = message.code ?? null;
        entry.reject(error);
      }
      return;
    }
    for (const listener of listeners[message.type] || []) listener(message);
  },
};

// --- state ---------------------------------------------------------------

const state = {
  init: null,
  status: null,
  pending: null,
  mark: null,
  diskBytes: null,
  devices: null,
  devicesError: null,
  network: null,
  networkBusy: false,
  networkError: null,
  confirmClose: false,
  removing: null,
  update: null,
  pane: 'home',
  message: null,
  flow: null,
  modelsMissing: false,
  downloadingModels: false,
};

const $ = id => document.getElementById(id);

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === 'on') for (const [event, handler] of Object.entries(value)) node.addEventListener(event, handler);
    else if (key === 'class') node.className = value;
    else if (value === true) node.setAttribute(key, '');
    else if (value !== false && value != null) node.setAttribute(key, value);
  }
  for (const child of children.flat()) {
    if (child == null || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

// Append children, skipping the null and false a conditional leaves.
function put(parent, ...children) {
  parent.append(...children.flat().filter(child => child != null && child !== false));
}

// A failure in plain words, with what the journal or setup said one tap down.
function failure(verdict, detail) {
  return [
    el('p', { class: 'error', role: 'alert' }, verdict),
    detail && detail !== verdict
      ? el('details', { class: 'detail' }, el('summary', {}, COPY.whatHappened), el('p', { class: 'faint' }, detail))
      : null,
  ];
}

function port() {
  return state.status?.service?.port ?? null;
}

function display() {
  if (state.pending) return state.pending;
  return state.status?.display ?? 'not-running';
}

function show(section) {
  for (const id of ['loading', 'firstrun', 'main']) $(id).hidden = id !== section;
  syncSetupGround();
}

// The sun arc behind the setup frame. It lives outside #firstrun, which is rebuilt on every
// refresh, and is torn down (its clock and listeners stopped) whenever the frame is not showing.
let setupSun = null;

function syncSetupGround() {
  const ground = $('setup-ground');
  const root = $('firstrun');
  if (!ground || !root) return;
  const framed = !root.hidden && root.getAttribute('data-phase') != null;
  if (framed && !setupSun && window.SunArc) {
    ground.hidden = false;
    setupSun = window.SunArc.mount(ground);
    if (!setupSun) ground.hidden = true;
  } else if (!framed && setupSun) {
    setupSun.teardown();
    setupSun = null;
    ground.hidden = true;
  }
}

// --- status --------------------------------------------------------------

async function refreshStatus(throwOnError = false) {
  try {
    state.status = await call('status');
  } catch (error) {
    state.message = error.message;
    render();
    if (throwOnError) throw error;
    return null;
  }
  render();
  return state.status;
}

let syncRetryNeeded = false;
async function syncAppRunEntry(on) {
  try {
    await call('syncSignInLaunch', { on });
    syncRetryNeeded = false;
    state.message = null;
  } catch (error) {
    state.message = error.message;
    syncRetryNeeded = true;
  }
  render();
}

async function retrySyncSignIn() {
  try {
    const status = await call('status');
    state.status = status;
    const on = status?.service?.starts_at_sign_in;
    if (typeof on !== 'boolean') throw new Error("couldn't read journal status");
    await call('syncSignInLaunch', { on });
    syncRetryNeeded = false;
    state.message = null;
  } catch (error) {
    state.message = error.message;
    syncRetryNeeded = true;
  }
  render();
}

async function serviceAction(action) {
  state.pending = action === 'stop' ? 'stopping' : 'starting';
  state.message = null;
  render();
  try {
    state.status = await call(action);
  } catch (error) {
    state.message = error.message;
    state.pending = null;
    render();
    return;
  }
  state.pending = null;
  render();
  if (state.init?.setup_sequence_enabled === true && (action === 'start' || action === 'restart')) {
    call('flow', { intent: 'retry-state' }).catch(() => {});
    return;
  }
  if (display() !== 'running') return;
  if (state.init?.setup_sequence_enabled !== true && !await firstRunDone()) {
    waitForFirstRun();
    return;
  }
  await loadJournalFacts();
}

async function loadJournalFacts() {
  try {
    const identity = await call('convey', { method: 'GET', path: '/app/link/api/identity', port: port() });
    if (identity?.committed === true && displayableMark(identity.mark)) {
      state.mark = identity.mark;
      state.markUnavailable = false;
      await applyIcon(state.mark);
    } else if (identity?.committed === true) {
      holdMarkAfterFailedRead();
    } else {
      state.mark = null;
      state.markUnavailable = false;
    }
  } catch {
    holdMarkAfterFailedRead();
  }
  render();
}

// A read that failed says nothing new about the journal's identity: keep the
// locked mark already held, and with none say so instead of "not set up yet".
function holdMarkAfterFailedRead() {
  if (!displayableMark(state.mark)) state.mark = null;
  state.markUnavailable = state.mark === null;
}

let iconMark = null;
async function applyIcon(mark) {
  if (sameMark(mark, iconMark)) return;
  try {
    const images = await markIconImages(mark, [16, 20, 24, 32, 40, 48, 64, 256]);
    await call('setIcon', { mark, images });
    iconMark = mark;
  } catch { /* the app keeps its own icon */ }
}

// A cheap check that the journal answers, often; the full reading of the
// scheduled task, now and then and after anything changes.
let polling = false;
function startPolling() {
  if (polling) return;
  polling = true;
  let ticks = 0;
  setInterval(async () => {
    ticks += 1;
    if (state.pending || $('main').hidden) return;
    if (ticks % 10 === 0) {
      await refreshStatus();
      return;
    }
    try {
      const ping = await call('ping', { port: port() });
      const answering = Boolean(ping.answering);
      if (answering !== Boolean(state.status?.answering)) {
        await refreshStatus();
        if (display() === 'running') await loadJournalFacts();
      }
    } catch { /* the next tick tries again */ }
  }, 3000);
}

function renderFirstRun(step) {
  const root = $('firstrun');
  root.replaceChildren();
  const card = el('div', { class: 'firstrun-card' });
  root.append(card);
  if (step === 'location') {
    const locationInput = el('input', { type: 'text', value: firstRun.location, 'aria-label': COPY.location });
    locationInput.addEventListener('change', async () => {
      firstRun.location = locationInput.value;
      firstRun.holdsJournal = await call('holdsJournal', { path: firstRun.location });
      renderFirstRun('location');
    });
    put(card,
      el('h1', {}, COPY.locationTitle),
      el('div', { class: 'field' },
        el('h2', {}, COPY.location),
        el('div', { class: 'row' }, locationInput,
          el('button', { type: 'button', on: { click: async () => {
            const picked = await call('pickFolder', { start: firstRun.location });
            if (picked) {
              firstRun.location = picked.path;
              firstRun.holdsJournal = picked.holds_journal;
              renderFirstRun('location');
            }
          } } }, COPY.chooseLocation)),
        firstRun.holdsJournal ? el('p', { class: 'faint' }, COPY.journalFound) : null),
      firstRun.error ? el('p', { class: 'error' }, firstRun.error) : null,
      el('div', { class: 'actions' },
        el('button', { type: 'button', class: 'primary', on: { click: runSetup } }, COPY.continueButton)),
    );
    locationInput.focus();
  } else if (step === 'setup') {
    put(card,
      el('h1', {}, COPY.setupTitle),
      el('p', { class: 'muted' }, COPY.setupSubtitle),
      // Only the steps that do something on Windows have words; the rest
      // pass by unseen.
      el('div', {}, el('ul', { class: 'steps' }, firstRun.steps.filter(item => COPY.steps[item.step]).map(item =>
        el('li', { class: item.state }, COPY.steps[item.step])))),
      firstRun.error ? failure(COPY.setupStopped, firstRun.error) : null,
      firstRun.error ? el('div', { class: 'actions centered' },
        el('button', { type: 'button', on: { click: runSetup } }, COPY.tryAgain)) : null,
    );
  } else if (step === 'browser') {
    put(card,
      el('h1', {}, COPY.browserTitle),
      el('p', { class: 'muted' }, COPY.browserLine),
      firstRun.error ? el('p', { class: 'error', role: 'alert' }, firstRun.error) : null,
      el('div', { class: 'actions centered' },
        el('button', { type: 'button', on: { click: openFirstRunPage } }, COPY.openBrowser)),
    );
  }
  show('firstrun');
}

function beginFirstRun(journalPath) {
  firstRun.location = journalPath || state.init.default_location;
  firstRun.error = null;
  call('holdsJournal', { path: firstRun.location }).then(holds => {
    firstRun.holdsJournal = holds;
    renderFirstRun('location');
  });
  renderFirstRun('location');
}

async function runSetup() {
  if (!firstRun.location.trim()) return;
  firstRun.adopted = firstRun.holdsJournal;
  firstRun.steps = [];
  firstRun.error = null;
  renderFirstRun('setup');
  try {
    const result = await call('setup', { path: firstRun.location });
    state.status = result.status;
    setModelsMissing(result.models_missing != null);
  } catch (error) {
    firstRun.error = error.message;
    renderFirstRun('setup');
    return;
  }
  await routeAfterSetup();
}

// The same reading of the journal's first run the Mac app makes.
async function routeAfterSetup() {
  let probe;
  try {
    probe = await call('initProbe', { port: port() });
  } catch (error) {
    firstRun.error = error.message;
    renderFirstRun('setup');
    return;
  }
  if (probe === 'complete') {
    try { await landLegacyHome(); } catch { waitForFirstRun(); }
    return;
  }
  waitForFirstRun();
}

// A completed reading is required; a failed probe keeps browser setup available.
async function firstRunDone() {
  return await call('initProbe').catch(() => 'incomplete') === 'complete';
}

// The rest of the first run is the journal's own page, the one every journal
// shows, in the owner's browser. The window waits until the journal says it
// is done, then lands home. A sign-in start opens the page only once the
// owner opens the window.
const firstRunWait = { timer: null, opened: false, checking: false };

function waitForFirstRun() {
  firstRun.error = null;
  renderFirstRun('browser');
  if (firstRunWait.timer === null) firstRunWait.timer = setInterval(checkFirstRunDone, 2000);
  if (!firstRunWait.opened && state.init.launch !== 'sign-in') openFirstRunPage();
}

async function openFirstRunPage() {
  firstRunWait.opened = true;
  firstRun.error = null;
  try {
    await call('open', { target: 'init', base: state.status?.base });
  } catch {
    firstRun.error = COPY.browserFailed;
  }
  if (firstRunWait.timer !== null) renderFirstRun('browser');
}

async function checkFirstRunDone() {
  if (firstRunWait.checking || firstRunWait.timer === null) return;
  firstRunWait.checking = true;
  try {
    if (await call('initProbe', { port: port() }) === 'complete' && firstRunWait.timer !== null) {
      await landLegacyHome();
      clearInterval(firstRunWait.timer);
      firstRunWait.timer = null;
    }
  } catch { /* the next check asks again */ } finally {
    firstRunWait.checking = false;
  }
}

async function landLegacyHome() {
  if (firstRun.adopted) state.message = COPY.adoptLandingLine;
  state.pane = 'home';
  await refreshStatus(true);
  const on = state.status?.service?.starts_at_sign_in;
  if (state.status?.service?.installed === true && typeof on === 'boolean') {
    await syncAppRunEntry(on);
  }
  show('main');
  startPolling();
  await loadJournalFacts();
}


// --- first-run flow: the window finishes setup ----------------------------

const firstRun = {
  location: '',
  holdsJournal: false,
  steps: [],
  adopted: false,
  error: null,
};

let downloadWatchTimer = null;
function syncDownloadWatch(watch) {
  if (watch && downloadWatchTimer === null) {
    downloadWatchTimer = setInterval(() => {
      call('flow', { intent: 'refresh' }).catch(() => {});
    }, 2000);
  } else if (!watch && downloadWatchTimer !== null) {
    clearInterval(downloadWatchTimer);
    downloadWatchTimer = null;
  }
}

let signInTouched = false;
let signInOn = true;
let signInScreen = false;
let flowFocus = null;
let paneFocus = null;

function continueThinking(choice) {
  let timezone = null;
  try { timezone = Intl.DateTimeFormat().resolvedOptions().timeZone; } catch { /* the journal can place the day */ }
  return call('flow', { intent: 'continue-thinking', choice, timezone });
}
let homeLanded = false;

function clearSetupFrame() {
  const root = $('firstrun');
  if (root) {
    root.replaceChildren();
    root.removeAttribute('data-phase');
  }
  syncSetupGround();
}

function setupStep(view) {
  if (view.phase === 'welcome' || view.phase === 'sign-in') return 1;
  if (view.phase === 'folder') return 2;
  if (view.phase === 'setup' && (view.setup_running || view.setup_failed)) return 2;
  if (view.phase === 'mark') return 3;
  if (view.phase === 'thinking') return 4;
  return null;
}

// Line glyphs for the welcome card's tiles (24-unit boxes, stroked in the orange text colour).
const SETUP_GLYPHS = {
  journal: ['M5 4h11a3 3 0 0 1 3 3v13H8a3 3 0 0 1-3-3z', 'M5 17a3 3 0 0 1 3-3h11'],
  app: ['M12 8a4 4 0 1 0 0 8a4 4 0 1 0 0-8z', 'M12 2v3', 'M12 19v3', 'M2 12h3', 'M19 12h3', 'M4.9 4.9l2.1 2.1', 'M17 17l2.1 2.1', 'M4.9 19.1l2.1-2.1', 'M17 7l2.1-2.1'],
  thinking: ['M9 18h6', 'M10 21h4', 'M12 3a6 6 0 0 0-3.5 10.9c.6.5 1 1.2 1 2.1h5c0-.9.4-1.6 1-2.1A6 6 0 0 0 12 3z'],
};

function setupGlyph(name) {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  for (const d of SETUP_GLYPHS[name]) {
    const path = document.createElementNS(ns, 'path');
    path.setAttribute('d', d);
    svg.append(path);
  }
  return el('span', { class: 'setup-tile' }, svg);
}

function renderProgress(step) {
  if (step == null) return null;
  const dots1 = [];
  for (let i = 1; i <= 4; i++) {
    const cls = i < step ? 'setup-dot-done' : (i === step ? 'setup-dot-current' : 'setup-dot-future');
    dots1.push(el('span', { class: `setup-dot ${cls}` }));
  }
  const dots2 = [];
  for (let i = 5; i <= 8; i++) {
    dots2.push(el('span', { class: 'setup-dot setup-dot-future' }));
  }
  return el('div', { class: 'setup-progress', role: 'img', 'aria-label': COPY.setup.step(step) },
    ...dots1,
    el('span', { class: 'setup-progress-gap' }),
    ...dots2,
    el('span', { class: 'setup-progress-label', 'aria-hidden': 'true' }, document.createTextNode(COPY.setup.step(step))),
  );
}

function renderFooterDownloads(view) {
  const flow = view;
  const copy = flow.copy?.downloads || {};
  const rows = [];

  // Speech row
  const speech = flow.downloads?.speech;
  if (speech && (speech.state === 'downloading' || speech.state === 'error' || flow.speech_retry)) {
    const parts = [el('span', { class: 'setup-download-name' }, document.createTextNode(copy.speech || ''))];
    if (speech.state === 'downloading') {
      if (typeof speech.bytes_total === 'number' && speech.bytes_total > 0) {
        const done = speech.bytes_done || 0;
        const total = speech.bytes_total;
        parts.push(el('span', { class: 'setup-download-bytes' }, document.createTextNode(`${formatBytes(done)} of ${formatBytes(total)}`)));
        const pct = Math.max(0, Math.min(100, Math.round((done / total) * 100)));
        const fill = el('span', { class: 'setup-download-fill' });
        fill.style.width = `${pct}%`;
        parts.push(el('div', {
          class: 'setup-download-bar',
          role: 'progressbar',
          'aria-valuemin': '0',
          'aria-valuemax': '100',
          'aria-valuenow': String(pct)
        }, fill));
      } else {
        if (typeof speech.bytes_done === 'number' && speech.bytes_done > 0) {
          parts.push(el('span', { class: 'setup-download-bytes' }, document.createTextNode(formatBytes(speech.bytes_done))));
        }
        parts.push(el('div', { class: 'setup-download-bar setup-download-bar-indeterminate' }));
      }
    } else if (speech.state === 'error') {
      if (speech.error?.detail) {
        parts.push(el('span', { class: 'setup-download-detail' }, document.createTextNode(speech.error.detail)));
      }
      if (flow.speech_retry) {
        parts.push(el('button', {
          type: 'button',
          'data-focus-key': 'speech:retry',
          disabled: Boolean(flow.busy) || flow.authoritative === false,
          on: {
            click: () => {
              if (flow.authoritative === false) return;
              call('flow', { intent: 'retry-speech-download' });
            }
          }
        }, document.createTextNode(copy.retry || COPY.tryAgain)));
      }
    } else if (flow.speech_retry) {
      parts.push(el('button', {
        type: 'button',
        'data-focus-key': 'speech:retry',
        disabled: Boolean(flow.busy) || flow.authoritative === false,
        on: {
          click: () => {
            if (flow.authoritative === false) return;
            call('flow', { intent: 'retry-speech-download' });
          }
        }
      }, document.createTextNode(copy.retry || COPY.tryAgain)));
    }
    rows.push(el('div', { class: 'setup-download', 'data-download': 'speech' }, ...parts));
  }

  // Thinking row
  const thinking = flow.downloads?.thinking;
  if (thinking && (thinking.state === 'downloading' || thinking.state === 'error' || flow.thinking_retry)) {
    const parts = [el('span', { class: 'setup-download-name' }, document.createTextNode(copy.thinking || ''))];
    if (thinking.state === 'downloading') {
      if (typeof thinking.bytes_total === 'number' && thinking.bytes_total > 0) {
        const done = thinking.bytes_done || 0;
        const total = thinking.bytes_total;
        parts.push(el('span', { class: 'setup-download-bytes' }, document.createTextNode(`${formatBytes(done)} of ${formatBytes(total)}`)));
        const pct = Math.max(0, Math.min(100, Math.round((done / total) * 100)));
        const fill = el('span', { class: 'setup-download-fill' });
        fill.style.width = `${pct}%`;
        parts.push(el('div', {
          class: 'setup-download-bar',
          role: 'progressbar',
          'aria-valuemin': '0',
          'aria-valuemax': '100',
          'aria-valuenow': String(pct)
        }, fill));
      } else {
        if (typeof thinking.bytes_done === 'number' && thinking.bytes_done > 0) {
          parts.push(el('span', { class: 'setup-download-bytes' }, document.createTextNode(formatBytes(thinking.bytes_done))));
        }
        parts.push(el('div', { class: 'setup-download-bar setup-download-bar-indeterminate' }));
      }
      const paintedAttempt = typeof thinking.attempt_id === 'string' ? thinking.attempt_id : '';
      if (flow.thinking_cancel && paintedAttempt.trim()) {
        parts.push(el('button', {
          type: 'button',
          'data-focus-key': 'thinking:cancel',
          disabled: Boolean(flow.busy) || flow.authoritative === false,
          on: {
            click: () => {
              if (flow.authoritative === false) return;
              call('flow', { intent: 'cancel-thinking-download', attempt_id: paintedAttempt });
            }
          }
        }, document.createTextNode(copy.cancel || COPY.cancel)));
      }
    } else if (thinking.state === 'error') {
      if (thinking.error?.detail) {
        parts.push(el('span', { class: 'setup-download-detail' }, document.createTextNode(thinking.error.detail)));
      }
      if (flow.thinking_retry) {
        parts.push(el('button', {
          type: 'button',
          'data-focus-key': 'thinking:retry',
          disabled: Boolean(flow.busy) || flow.authoritative === false,
          on: {
            click: () => {
              if (flow.authoritative === false) return;
              call('flow', { intent: 'start-thinking-download' });
            }
          }
        }, document.createTextNode(copy.retry || COPY.tryAgain)));
      }
    } else if (flow.thinking_retry) {
      parts.push(el('button', {
        type: 'button',
        'data-focus-key': 'thinking:retry',
        disabled: Boolean(flow.busy) || flow.authoritative === false,
        on: {
          click: () => {
            if (flow.authoritative === false) return;
            call('flow', { intent: 'start-thinking-download' });
          }
        }
      }, document.createTextNode(copy.retry || COPY.tryAgain)));
    }
    rows.push(el('div', { class: 'setup-download', 'data-download': 'thinking' }, ...parts));
  }

  // Models row
  if (flow.models?.showing) {
    const parts = [
      el('span', { class: 'setup-download-detail' }, document.createTextNode(flow.models.detail || '')),
      el('button', {
        type: 'button',
        'data-focus-key': 'models:retry',
        disabled: Boolean(flow.models.pending) || Boolean(flow.busy),
        on: { click: () => call('flow', { intent: 'retry-models' }) }
      }, document.createTextNode(COPY.tryAgain))
    ];
    rows.push(el('div', { class: 'setup-download', 'data-download': 'models' }, ...parts));
  }

  if (rows.length === 0) return null;
  return el('div', { class: 'setup-downloads' }, ...rows);
}

let setupRunningArmed = false;

async function finishSetupToHome(view) {
  let status = null;
  try {
    status = await refreshStatus(true);
  } catch (error) {
    const root = $('firstrun');
    root.setAttribute('data-phase', 'home');
    root.replaceChildren();

    const band = el('header', { class: 'setup-band' },
      el('img', { class: 'setup-lockup', src: '/lockup-solstone-horizontal.svg', alt: 'solstone' }),
      el('span', { class: 'setup-band-label' }, document.createTextNode(COPY.setup.band))
    );
    const lead = el('div', { class: 'setup-lead' });
    const card = el('div', { class: 'setup-card' },
      el('p', { class: 'error' }, document.createTextNode(error.message || String(error))),
      el('button', {
        type: 'button',
        class: 'primary',
        on: { click: () => finishSetupToHome(view) },
      }, document.createTextNode(COPY.tryAgain))
    );
    const columns = el('div', { class: 'setup-columns' }, lead, card);
    const sheet = el('div', { class: 'setup-sheet' }, columns);
    sheet.append(el('footer', { class: 'setup-footer' }));
    root.append(band, sheet);
    show('firstrun');
    return;
  }
  setupRunningArmed = false;
  landHome(view);
  if (status?.service?.installed === true && typeof status.service.starts_at_sign_in === 'boolean') {
    await syncAppRunEntry(status.service.starts_at_sign_in);
  }
}

function renderFlow(view) {
  if (!view) return;
  const previousPhase = state.flow?.phase ?? null;
  state.flow = view;
  syncDownloadWatch(view.watch_downloads);

  if (view.phase !== 'welcome' && view.phase !== 'sign-in') {
    signInTouched = false;
    signInScreen = false;
  } else if (!signInScreen && !signInTouched) {
    signInOn = view.sign_in !== false;
    signInScreen = true;
  }

  if (view.phase === 'setup' && view.setup_running === true) {
    setupRunningArmed = true;
  }

  if (view.resting) {
    clearSetupFrame();
    show('main');
    render();
    return;
  }
  if (view.phase === 'home') {
    if (setupRunningArmed) {
      finishSetupToHome(view);
      return;
    }
    landHome(view);
    return;
  }
  if (view.phase === 'opening') {
    const installed = state.status?.service?.installed === true;
    const hasPort = state.status?.service?.port != null;
    const disp = display();
    if (installed && hasPort && (disp === 'stopped' || disp === 'not-running')) {
      clearSetupFrame();
      show('main');
      render();
      return;
    }
  }

  const root = $('firstrun');
  const focused = document.activeElement;
  if (view.phase === previousPhase && focused && focused.tagName === 'INPUT' && root.contains(focused)) {
    return;
  }

  if (view.phase !== previousPhase) {
    flowFocus = null;
  } else if (focused && root.contains(focused)) {
    const key = focused.getAttribute ? focused.getAttribute('data-focus-key') : null;
    flowFocus = key || null;
  } else if (focused && focused !== document.body) {
    flowFocus = null;
  }

  root.replaceChildren();
  root.setAttribute('data-phase', view.phase);

  const band = el('header', { class: 'setup-band' },
    el('img', { class: 'setup-lockup', src: '/lockup-solstone-horizontal.svg', alt: 'solstone' }),
    el('span', { class: 'setup-band-label' }, document.createTextNode(COPY.setup.band))
  );

  const lead = el('div', { class: 'setup-lead' });
  const card = el('div', { class: 'setup-card' });
  const columns = el('div', { class: 'setup-columns' }, lead, card);
  const sheet = el('div', { class: 'setup-sheet' }, columns);

  let footerProgress = null;
  let footerDownloads = null;
  let footerPrimary = null;

  const step = setupStep(view);
  if (step !== null) {
    footerProgress = renderProgress(step);
  }

  const copy = view.copy || {};

  if (view.phase === 'welcome' || view.phase === 'sign-in') {
    const welcome = copy.welcome || {};
    const login = copy.login || {};
    put(lead,
      el('h1', {}, document.createTextNode(welcome.title || copy.heading || '')),
      welcome.body ? el('p', {}, document.createTextNode(welcome.body)) : null,
      Array.isArray(welcome.how_it_fits) && typeof welcome.how_it_fits[2] === 'string'
        ? el('p', {}, document.createTextNode(welcome.how_it_fits[2]))
        : null
    );

    const fit = (glyph, term, text) => el('div', { class: 'setup-fit' },
      setupGlyph(glyph),
      el('div', {},
        el('span', { class: 'setup-fit-term' }, document.createTextNode(term)),
        el('span', { class: 'setup-fit-text' }, document.createTextNode(text))
      )
    );
    put(card,
      el('h2', { class: 'setup-card-h' }, document.createTextNode(COPY.setup.fitsTitle)),
      fit('journal', COPY.setup.journalTerm, COPY.setup.journalText),
      fit('app', COPY.setup.appTerm, COPY.setup.appText),
      fit('thinking', COPY.setup.thinkingTerm, COPY.setup.thinkingText),
      el('hr', { class: 'setup-sep' }),
      el('button', {
        type: 'button',
        id: 'sign-in-check',
        class: 'setup-switch-row',
        role: 'switch',
        'aria-checked': signInOn ? 'true' : 'false',
        'aria-label': login.label || COPY.startAtSignIn,
        'data-focus-key': 'sign-in',
        disabled: Boolean(view.busy),
        on: {
          click: () => {
            signInTouched = true;
            signInOn = !signInOn;
            renderFlow(state.flow);
          }
        }
      },
        el('span', { class: 'setup-switch', 'aria-hidden': 'true' }),
        el('span', { class: 'setup-switch-text' },
          el('span', { class: 'setup-switch-label' }, document.createTextNode(login.label || COPY.startAtSignIn)),
          el('span', { class: 'setup-switch-help' }, document.createTextNode(COPY.setup.signIn))
        )
      ),
      view.error ? el('p', { class: 'error' }, document.createTextNode(view.error)) : null
    );

    footerPrimary = el('button', {
      type: 'button',
      class: 'primary',
      'data-focus-key': 'primary',
      disabled: Boolean(view.busy),
      on: {
        click: async () => {
          const on = signInOn;
          try {
            if (view.phase === 'welcome') await call('flow', { intent: 'acknowledge' });
            await call('flow', { intent: 'choose-sign-in', on });
            await call('flow', { intent: 'continue-sign-in' });
          } catch {
            // receive() already painted the error on this phase. Stop the sequence.
          }
        }
      }
    }, document.createTextNode(COPY.setup.start));
  } else if (view.phase === 'folder') {
    const folderCopy = copy.folder || {};
    put(lead,
      el('h1', {}, document.createTextNode(COPY.setup.folderTitle)),
      el('p', {}, document.createTextNode(COPY.setup.folderText))
    );

    if (!firstRun.location) {
      firstRun.location = state.init?.default_location || '';
    }
    const locationInput = el('input', {
      type: 'text',
      value: firstRun.location,
      'aria-label': COPY.location,
      disabled: Boolean(view.busy),
    });
    locationInput.addEventListener('change', async () => {
      firstRun.location = locationInput.value;
      firstRun.holdsJournal = await call('holdsJournal', { path: firstRun.location });
      renderFlow(state.flow);
    });

    const pickBtn = el('button', {
      type: 'button',
      'data-focus-key': 'folder:change',
      disabled: Boolean(view.busy),
      on: {
        click: async () => {
          const picked = await call('pickFolder', { start: firstRun.location });
          if (picked) {
            firstRun.location = picked.path;
            firstRun.holdsJournal = picked.holds_journal;
            renderFlow(state.flow);
          }
        }
      }
    }, document.createTextNode(folderCopy.change || COPY.chooseLocation));

    put(card,
      el('h2', { class: 'setup-card-h' }, document.createTextNode(COPY.setup.folderLabel)),
      el('div', { class: 'row setup-folder-row' }, locationInput, pickBtn),
      firstRun.holdsJournal ? el('p', { class: 'faint' }, document.createTextNode(COPY.journalFound)) : null,
      view.error ? el('p', { class: 'error' }, document.createTextNode(view.error)) : null
    );

    footerPrimary = el('button', {
      type: 'button',
      class: 'primary',
      'data-focus-key': 'primary',
      disabled: Boolean(view.busy),
      on: {
        click: () => {
          firstRun.adopted = firstRun.holdsJournal;
          firstRun.steps = [];
          call('flow', { intent: 'start-setup', path: firstRun.location });
        }
      }
    }, document.createTextNode(folderCopy.continue || COPY.continueButton));
  } else if (view.phase === 'setup') {
    if (view.setup_running || view.setup_failed) {
      put(lead,
        el('h1', {}, document.createTextNode(COPY.setupTitle)),
        el('p', { class: 'muted' }, document.createTextNode(COPY.setupSubtitle))
      );
      put(card,
        el('div', {}, el('ul', { class: 'steps' },
          firstRun.steps.filter(item => COPY.steps[item.step]).map(item =>
            el('li', { class: item.state }, document.createTextNode(COPY.steps[item.step]))
          )
        )),
        view.error ? failure(COPY.setupStopped, view.error) : null,
        view.error ? el('button', {
          type: 'button',
          class: 'primary',
          disabled: Boolean(view.busy),
          'data-focus-key': 'retry-setup',
          on: { click: () => call('flow', { intent: 'retry-setup' }) }
        }, document.createTextNode(COPY.tryAgain)) : null
      );
    } else {
      // Between steps (setup just finished, state loading) nothing has stopped.
      put(lead,
        el('h1', {}, document.createTextNode(COPY.setupTitle))
      );
      if (view.error) put(card,
        failure(COPY.setupStopped, view.error),
        el('button', {
          type: 'button',
          class: 'primary',
          disabled: Boolean(view.busy),
          'data-focus-key': 'retry-state',
        on: { click: () => call('flow', { intent: 'retry-state' }) }
        }, document.createTextNode(COPY.tryAgain))
      );
    }
  } else if (view.phase === 'opening') {
    // Opening is a moment, not a step: no heading unless something went wrong.
    if (view.error) put(lead, el('h1', {}, document.createTextNode(COPY.setupStopped)));
    if (view.error) {
      put(card,
        failure(COPY.setupStopped, view.error),
        el('button', {
          type: 'button',
          class: 'primary',
          disabled: Boolean(view.busy),
          'data-focus-key': 'retry-state',
        on: { click: () => call('flow', { intent: 'retry-state' }) }
        }, document.createTextNode(COPY.tryAgain))
      );
    }
  } else if (view.phase === 'mark') {
    const markCopy = copy.mark || {};
    const proposalId = typeof view.mark?.proposal_id === 'string' ? view.mark.proposal_id : '';
    const markBox = el('div');
    const markMissing = view.mark?.render == null;
    if (!markMissing) renderMarkCard(markBox, view.mark.render);
    const canCommit = view.authoritative !== false && displayableMark(view.mark?.render);

    put(lead,
      el('h1', {}, document.createTextNode(markCopy.title || '')),
      markCopy.help ? el('p', { class: 'muted' }, document.createTextNode(markCopy.help)) : null
    );

    card.classList.add('setup-card-center');
    put(card,
      markMissing
        ? (view.busy
          ? el('progress', { 'aria-label': markCopy.title || COPY.setupTitle })
          : (markCopy.error ? el('p', { class: 'error' }, document.createTextNode(markCopy.error)) : null))
        : markBox,
      markCopy.permanence ? el('p', { class: 'faint' }, document.createTextNode(markCopy.permanence)) : null
    );

    if (view.authoritative === false) {
      card.append(el('button', {
        type: 'button',
        disabled: Boolean(view.busy),
        'data-focus-key': 'retry-state',
        on: { click: () => call('flow', { intent: 'retry-state' }) }
      }, document.createTextNode(COPY.tryAgain)));
    }
    if (markCopy.try_another) {
      card.append(el('button', {
        type: 'button',
        class: 'setup-secondary',
        'data-focus-key': 'mark:try',
        disabled: Boolean(view.busy) || view.authoritative === false,
        on: {
          click: () => {
            if (view.authoritative === false) return;
            call('flow', { intent: 'regenerate-mark' });
          }
        }
      }, document.createTextNode(markCopy.try_another)));
    }
    if (view.error) {
      card.append(el('p', { class: 'error' }, document.createTextNode(view.error)));
    }

    footerPrimary = el('button', {
      type: 'button',
      class: 'primary',
      'data-focus-key': 'primary',
      disabled: Boolean(view.busy) || !canCommit,
      on: {
        click: () => {
          if (!canCommit) return;
          call('flow', { intent: 'commit-mark', proposal_id: proposalId });
        }
      }
    }, document.createTextNode(markCopy.continue || COPY.continueButton));
  } else if (view.phase === 'thinking') {
    const thinkingCopy = copy.thinking || {};
    const choices = Array.isArray(thinkingCopy.choices) ? thinkingCopy.choices : [];
    const saved = Boolean(view.thinking_saved) && ['local', 'spp', 'byo', 'later'].includes(view.thinking_choice);
    const localCapable = view.thinking_capability === 'ok';
    const showLocal = localCapable || (Boolean(view.thinking_saved) && view.thinking_choice === 'local');

    let selectedChoice = null;
    if (Boolean(view.thinking_saved) && ['local', 'spp', 'byo'].includes(view.thinking_choice)) {
      selectedChoice = view.thinking_choice;
    } else if (!view.thinking_saved && localCapable) {
      selectedChoice = 'local';
    }

    put(lead,
      el('h1', {}, document.createTextNode(COPY.setup.thinkingTitle)),
      el('p', {}, document.createTextNode(COPY.setup.thinkingLead))
    );

    if (!saved && !localCapable && typeof thinkingCopy.choose_later === 'string' && thinkingCopy.choose_later.length > 0) {
      card.append(el('p', {}, document.createTextNode(thinkingCopy.choose_later)));
    }

    for (const choice of choices) {
      if (choice.id === 'local' && !showLocal) continue;
      const isSelected = selectedChoice === choice.id;
      const label = choice.id === 'local' ? COPY.setup.onThisPc : (choice.label || choice.id);
      let sub = null;
      if (choice.id === 'local') {
        if (typeof view.thinking_download_bytes === 'number' && view.thinking_download_bytes > 0) {
          sub = formatBytes(view.thinking_download_bytes);
        }
      } else if (typeof choice.sub === 'string' && choice.sub.length > 0) {
        sub = choice.sub;
      }
      const desc = choice.id === 'local'
        ? COPY.setup.localDesc
        : (typeof choice.description === 'string' && choice.description.length > 0 ? choice.description : null);

      card.append(el('button', {
        type: 'button',
        class: 'setup-choice',
        role: 'radio',
        'data-choice-id': choice.id,
        'data-focus-key': choice.id,
        'aria-checked': isSelected ? 'true' : 'false',
        'aria-label': label,
        disabled: Boolean(view.busy) || view.authoritative === false,
        on: {
          click: () => {
            if (view.authoritative === false || Boolean(view.busy)) return;
            call('flow', { intent: 'choose-thinking', choice: choice.id });
          }
        }
      },
        el('span', { class: 'setup-radio', 'aria-hidden': 'true' }),
        el('span', { class: 'setup-choice-body' },
          el('span', { class: 'setup-choice-head' },
            el('span', { class: 'setup-choice-label' }, document.createTextNode(label)),
            sub ? el('span', { class: 'setup-choice-sub' }, document.createTextNode(sub)) : null
          ),
          desc ? el('span', { class: 'setup-choice-desc' }, document.createTextNode(desc)) : null
        )
      ));
    }

    if (typeof thinkingCopy.later === 'string' && thinkingCopy.later.length > 0) {
      const laterSaved = Boolean(view.thinking_saved) && view.thinking_choice === 'later';
      card.append(el('button', {
        type: 'button',
        class: laterSaved ? 'setup-text setup-text-chosen' : 'setup-text',
        'aria-pressed': laterSaved ? 'true' : 'false',
        'data-focus-key': 'later',
        disabled: Boolean(view.busy) || view.authoritative === false,
        on: {
          click: () => {
            if (view.authoritative === false || Boolean(view.busy)) return;
            continueThinking('later');
          }
        }
      }, document.createTextNode(thinkingCopy.later)));
    }

    if (view.authoritative === false) {
      card.append(el('button', {
        type: 'button',
        disabled: Boolean(view.busy),
        'data-focus-key': 'retry-state',
        on: { click: () => call('flow', { intent: 'retry-state' }) }
      }, document.createTextNode(COPY.tryAgain)));
    }

    if (view.error) {
      card.append(el('p', { class: 'error' }, document.createTextNode(view.error)));
    }

    footerDownloads = renderFooterDownloads(view);

    if (!view.thinking_saved && localCapable) {
      footerPrimary = el('button', {
        type: 'button',
        class: 'primary',
        'data-focus-key': 'primary',
        disabled: Boolean(view.busy) || view.authoritative === false,
        on: {
          click: () => {
            if (view.authoritative === false || Boolean(view.busy)) return;
            continueThinking('local');
          }
        }
      }, document.createTextNode(thinkingCopy.continue || COPY.continueButton));
    } else if (Boolean(view.thinking_saved) && ['local', 'spp', 'byo', 'later'].includes(view.thinking_choice) && view.can_finalize) {
      footerPrimary = el('button', {
        type: 'button',
        class: 'primary',
        'data-focus-key': 'primary',
        disabled: Boolean(view.busy) || view.authoritative === false,
        on: {
          click: () => {
            if (view.authoritative === false || Boolean(view.busy)) return;
            continueThinking(view.thinking_choice);
          }
        }
      }, document.createTextNode(thinkingCopy.continue || COPY.continueButton));
    }
  }

  const footerChildren = [];
  if (footerProgress) footerChildren.push(footerProgress);
  if (footerDownloads) footerChildren.push(footerDownloads);
  if (footerPrimary) footerChildren.push(footerPrimary);
  const footer = el('footer', { class: 'setup-footer' }, ...footerChildren);
  sheet.append(footer);

  root.append(band, sheet);

  show('firstrun');
  if (view.phase !== previousPhase) {
    const heading = lead.querySelector('h1');
    if (heading) {
      heading.tabIndex = -1;
      heading.focus();
    }
  } else if (flowFocus) {
    // A control disabled by a busy round-trip keeps its claim until it is enabled again.
    const candidate = root.querySelector(`[data-focus-key="${flowFocus}"]`);
    if (candidate && !candidate.disabled) {
      candidate.focus();
      flowFocus = null;
    } else if (!view.busy) {
      flowFocus = null;
    }
  }
}

function landHome(view) {
  state.flow = view;
  clearSetupFrame();
  if (!homeLanded) {
    homeLanded = true;
    state.pane = 'home';
    show('main');
    if (firstRun.adopted) state.message = COPY.adoptLandingLine;
    render(true);
    const heading = $('pane').querySelector('h1');
    if (heading) {
      heading.tabIndex = -1;
      heading.focus();
    }
    if (display() === 'running') loadJournalFacts();
    return;
  }
  show('main');
  if (state.pane === 'home') render(false);
}

listeners.flow.push(({ view }) => {
  if (state.init?.setup_sequence_enabled === true) renderFlow(view);
});

listeners.setup.push(({ progress }) => {
  if (progress.kind === 'step-started') {
    firstRun.steps = firstRun.steps.filter(item => item.step !== progress.step);
    firstRun.steps.push({ step: progress.step, state: 'active' });
  } else if (progress.kind === 'step-finished') {
    const item = firstRun.steps.find(entry => entry.step === progress.step);
    if (item) item.state = progress.outcome === 'skipped' ? 'skipped' : 'done';
    firstRun.steps = firstRun.steps.filter(entry => entry.state !== 'skipped');
  } else if (progress.kind === 'step-failed') {
    const item = firstRun.steps.find(entry => entry.step === progress.step);
    if (item) item.state = 'failed';
  }
  if (state.init?.setup_sequence_enabled !== true) {
    renderFirstRun('setup');
  } else if (state.flow?.phase === 'setup') {
    renderFlow(state.flow);
  }
});

// --- the main window -----------------------------------------------------

function renderSidebar() {
  const list = $('panes');
  list.replaceChildren(...Object.entries(COPY.panes).map(([key, title]) =>
    el('li', {}, el('button', {
      type: 'button', role: 'tab', 'aria-selected': String(state.pane === key),
      on: { click: () => openPane(key) },
    }, title))));
  $('admin-terminal').textContent = COPY.adminTerminal;
  $('quit').textContent = COPY.quit;
}

function openPane(key) {
  if (key !== 'devices') closePairing();
  state.pane = key;
  state.message = key === 'home' ? state.message : null;
  if (key === 'journal' && state.status?.service?.journal) {
    call('diskUsage', { path: state.status.service.journal }).then(bytes => {
      state.diskBytes = bytes;
      render();
    });
  }
  if (key === 'devices') loadDevices();
  if (key === 'updates') call('update', { action: 'view' }).then(view => { state.update = view; render(); });
  render();
}

function statusLine() {
  const key = display();
  return el('p', { class: `status ${key}` }, COPY.display[key] ?? key);
}

function message() {
  if (syncRetryNeeded) {
    return el('div', { class: 'field' },
      state.message ? el('p', { class: 'error' }, state.message) : null,
      el('button', {
        type: 'button',
        on: { click: () => retrySyncSignIn() }
      }, COPY.tryAgain)
    );
  }
  return state.message ? el('p', { class: 'muted' }, state.message) : null;
}

function flowError() {
  const text = state.flow?.error;
  return text ? el('p', { class: 'error' }, document.createTextNode(text)) : null;
}

function info(label, value) {
  return el('div', { class: 'info' }, el('h2', {}, label), el('div', { class: 'value' }, value));
}

function openPage(target) {
  return call('open', { target, base: state.status?.base }).catch(error => {
    state.message = error.message;
    render();
  });
}

function retryWord(copy) {
  return typeof copy.retry === 'string' && copy.retry ? copy.retry : COPY.tryAgain;
}

function showsBytes(download) {
  return typeof download.bytes_total === 'number'
    || (typeof download.bytes_done === 'number' && download.bytes_done > 0)
    || download.state === 'downloading';
}

function byteText(download) {
  const done = typeof download.bytes_done === 'number' ? download.bytes_done : 0;
  if (typeof download.bytes_total === 'number' && download.bytes_total > 0) {
    return `${formatBytes(done)} of ${formatBytes(download.bytes_total)}`;
  }
  return done > 0 ? formatBytes(done) : '';
}

function speechBlock() {
  const flow = state.flow;
  const speech = flow?.downloads?.speech;
  if (!speech) return null;
  const copy = flow.copy?.downloads || {};
  const parts = [];
  if (copy.speech) parts.push(el('p', {}, document.createTextNode(copy.speech)));
  if (speech.state === 'idle' && copy.idle) parts.push(el('p', { class: 'muted' }, document.createTextNode(copy.idle)));
  if (speech.state === 'complete' && copy.complete) parts.push(el('p', { class: 'muted' }, document.createTextNode(copy.complete)));
  if (speech.state === 'cancelled' && copy.cancelled) parts.push(el('p', { class: 'muted' }, document.createTextNode(copy.cancelled)));
  if (speech.state === 'error' && speech.error?.detail) {
    parts.push(el('p', {}, document.createTextNode(speech.error.detail)));
  }
  if (showsBytes(speech)) parts.push(el('p', { class: 'bytes' }, document.createTextNode(byteText(speech))));
  if (speech.state === 'error' && flow.speech_retry) {
    parts.push(el('button', {
      type: 'button',
      disabled: Boolean(flow.busy) || flow.authoritative === false,
      on: { click: () => {
        if (flow.authoritative === false) return;
        call('flow', { intent: 'retry-speech-download' });
      } },
    }, document.createTextNode(retryWord(copy))));
  }
  if (!parts.length) return null;
  return el('div', { class: 'notice', 'data-download': 'speech' }, ...parts);
}

function thinkingBlock() {
  const flow = state.flow;
  const thinking = flow?.downloads?.thinking;
  if (!thinking) return null;
  const copy = flow.copy?.downloads || {};
  const parts = [];
  if (copy.thinking) parts.push(el('p', {}, document.createTextNode(copy.thinking)));
  if (thinking.state === 'idle' && copy.idle) parts.push(el('p', { class: 'muted' }, document.createTextNode(copy.idle)));
  if (thinking.state === 'complete' && copy.complete) parts.push(el('p', { class: 'muted' }, document.createTextNode(copy.complete)));
  if (thinking.state === 'cancelled' && copy.cancelled) parts.push(el('p', { class: 'muted' }, document.createTextNode(copy.cancelled)));
  if (thinking.state === 'error' && thinking.error?.detail) {
    parts.push(el('p', {}, document.createTextNode(thinking.error.detail)));
  }
  if (showsBytes(thinking)) parts.push(el('p', { class: 'bytes' }, document.createTextNode(byteText(thinking))));
  if (flow.thinking_retry && ['error', 'cancelled', 'idle'].includes(thinking.state)) {
    parts.push(el('button', {
      type: 'button',
      disabled: Boolean(flow.busy) || flow.authoritative === false,
      on: { click: () => {
        if (flow.authoritative === false) return;
        call('flow', { intent: 'start-thinking-download' });
      } },
    }, document.createTextNode(retryWord(copy))));
  }
  const paintedAttempt = typeof thinking.attempt_id === 'string' ? thinking.attempt_id : '';
  if (flow.thinking_cancel && paintedAttempt.trim()) {
    parts.push(el('button', {
      type: 'button',
      disabled: Boolean(flow.busy) || flow.authoritative === false,
      on: { click: () => {
        if (flow.authoritative === false) return;
        call('flow', { intent: 'cancel-thinking-download', attempt_id: paintedAttempt });
      } },
    }, document.createTextNode(copy.cancel || COPY.cancel)));
  }
  if (!parts.length) return null;
  return el('div', { class: 'notice', 'data-download': 'thinking' }, ...parts);
}

function modelsBlock() {
  const flow = state.flow;
  if (!flow?.models?.showing) return null;
  return el('div', { class: 'notice' },
    el('p', {}, document.createTextNode(flow.models.detail || '')),
    el('button', {
      type: 'button',
      disabled: Boolean(flow.models.pending) || Boolean(flow.busy),
      on: { click: () => call('flow', { intent: 'retry-models' }) },
    }, document.createTextNode(COPY.tryAgain))
  );
}

const panes = {
  home() {
    const markBox = el('div');
    renderMarkCard(markBox, state.mark, COPY.confirmedLine, state.markUnavailable);
    const key = display();
    let offer = null;
    if (key === 'running') {
      offer = el('button', {
        type: 'button',
        'data-intent': 'open-journal',
        disabled: Boolean(state.flow?.busy),
        on: { click: () => state.init?.setup_sequence_enabled === true
          ? call('flow', { intent: 'open-journal' }) : openPage('journal') }
      }, COPY.openJournal);
    } else if (key === 'stopped' || key === 'not-running') {
      offer = el('button', { type: 'button', on: { click: () => serviceAction('start') } }, COPY.start);
    } else {
      offer = el('button', { type: 'button', on: { click: () => openPane('run') } }, COPY.runStateLink);
    }
    const continueBtn = state.init?.setup_sequence_enabled === true ? el('button', {
      type: 'button',
      class: 'primary',
      'data-intent': 'continue-solstone',
      disabled: state.flow?.authoritative !== true || Boolean(state.flow?.busy),
      on: { click: () => call('flow', { intent: 'continue-solstone' }) }
    }, COPY.continueInSolstone) : null;
    const downloadBtn = state.flow?.solstone_download ? el('button', {
      type: 'button',
      'data-intent': 'download-solstone',
      on: { click: () => call('flow', { intent: 'download-solstone' }) }
    }, COPY.downloadSolstone) : null;
    const authRetry = state.flow?.authoritative === false ? el('div', { class: 'actions centered' },
      el('button', {
        type: 'button',
        disabled: Boolean(state.flow?.busy),
        'data-focus-key': 'retry-state',
        on: { click: () => call('flow', { intent: 'retry-state' }) },
      }, COPY.tryAgain)
    ) : null;
    return [
      el('h1', {}, COPY.homeTitle),
      markBox,
      statusLine(),
      message(),
      authRetry,
      flowError(),
      speechBlock(),
      thinkingBlock(),
      state.init?.setup_sequence_enabled === true ? modelsBlock() :
        state.modelsMissing ? el('div', { class: 'notice' },
          el('p', {}, COPY.modelsMissing),
          el('button', { type: 'button', disabled: state.downloadingModels, on: { click: downloadModels } }, COPY.tryAgain)) : null,
      el('p', { class: 'muted' }, COPY.homeLine),
      el('div', { class: 'actions' }, continueBtn, downloadBtn, offer),
    ];
  },

  journal() {
    const journalPath = state.status?.service?.journal ?? '';
    return [
      el('h1', {}, COPY.panes.journal),
      info(COPY.location, journalPath),
      el('div', { class: 'actions tight' },
        el('button', { type: 'button', on: { click: () =>
          call('open', { target: 'folder', path: journalPath }).catch(() => {}) } }, COPY.showInExplorer)),
      info(COPY.diskUsed, state.diskBytes == null ? COPY.unknown : formatBytes(state.diskBytes)),
    ];
  },

  run() {
    const busy = Boolean(state.pending);
    const key = display();
    const about = state.status?.about || state.init.about;
    return [
      el('h1', {}, COPY.panes.run),
      statusLine(),
      message(),
      flowError(),
      el('div', { class: 'actions spaced' },
        el('button', { type: 'button', disabled: busy, on: { click: () => serviceAction('start') } }, COPY.start),
        el('button', { type: 'button', disabled: busy, on: { click: () => serviceAction('stop') } }, COPY.stop),
        el('button', { type: 'button', disabled: busy, on: { click: () => serviceAction('restart') } }, COPY.restart)),
      info(COPY.health, key === 'running' ? COPY.healthy : (key === 'stopped' ? COPY.display.stopped : COPY.unknown)),
      el('pre', { class: 'about-block' }, about),
      el('button', { type: 'button', on: { click: async (event) => {
        const button = event.currentTarget;
        if (await copyText(about)) button.textContent = COPY.copiedAbout;
        else { state.message = COPY.copyAboutFailed; render(); }
      } } }, COPY.copyAbout),
    ];
  },

  devices() {
    return renderDevices();
  },

  backup() {
    return [
      el('h1', {}, COPY.backupTitle),
      el('p', { class: 'muted' }, COPY.backupLine),
      message(),
      el('div', { class: 'actions' },
        el('button', { type: 'button', on: { click: () => openPage('backup') } }, COPY.openBackup)),
    ];
  },

  startup() {
    const on = state.status?.service?.starts_at_sign_in ?? true;
    const toggle = el('input', { type: 'checkbox', checked: on, disabled: Boolean(state.pending) });
    toggle.addEventListener('change', async () => {
      toggle.disabled = true;
      try {
        state.status = await call('signIn', { on: toggle.checked });
      } catch (error) {
        state.message = error.message;
      }
      render();
    });
    return [
      el('h1', {}, COPY.startupTitle),
      el('label', { class: 'switch' }, toggle, COPY.startAtSignIn),
      message(),
    ];
  },

  updates() {
    return renderUpdates();
  },
};

function setModelsMissing(missing) {
  state.modelsMissing = missing;
  try {
    if (missing) localStorage.setItem('modelsMissing', '1'); else localStorage.removeItem('modelsMissing');
  } catch { /* the notice still shows for this run */ }
}

async function downloadModels() {
  state.downloadingModels = true;
  render();
  try {
    await call('installModels');
    setModelsMissing(false);
    state.message = null;
  } catch (error) {
    state.message = error.message;
  }
  state.downloadingModels = false;
  render();
}

// --- devices and pairing --------------------------------------------------

// The device list, and whether the journal is open to devices on the
// network, read together each time the pane opens.
async function loadDevices() {
  state.devicesError = null;
  state.removing = null;
  const [devices, network] = await Promise.allSettled([
    call('convey', { method: 'GET', path: '/app/network/api/devices', port: port() }),
    call('convey', { method: 'GET', path: '/app/network/api/local-network', port: port() }),
  ]);
  if (devices.status === 'fulfilled') {
    state.devices = Array.isArray(devices.value?.devices) ? devices.value.devices : [];
  } else {
    state.devices = null;
    state.devicesError = COPY.devicesLoadFailed;
  }
  state.network = network.status === 'fulfilled' ? network.value : null;
  render();
}

function isPeerJournal(device) {
  return String(device.role ?? '').trim().toLowerCase() === 'peer';
}

function deviceName(device) {
  if (isPeerJournal(device)) return 'another journal';
  const name = [device.display_label, device.device_label, device.observer_handle]
    .map(value => String(value ?? '').trim()).find(Boolean);
  return name || COPY.unnamedDevice;
}

// The journal writes these as UTC timestamps; a number is seconds.
function deviceTime(value) {
  if (value == null || value === '') return null;
  const time = typeof value === 'number' ? value * 1000 : Date.parse(String(value).trim());
  return Number.isFinite(time) ? new Date(time) : null;
}

function deviceDetail(device) {
  const parts = [];
  if (device.last_seen_at == null) {
    parts.push(COPY.neverConnected);
  } else {
    const seen = deviceTime(device.last_seen_at);
    if (!seen) parts.push(COPY.lastSeenUnknown);
    else if (Date.now() - seen.getTime() < 60000) parts.push(COPY.lastSeenJustNow);
    else parts.push(COPY.lastSeen(relative(seen.getTime() / 1000)));
  }
  const paired = deviceTime(device.paired_at);
  if (paired) {
    parts.push(COPY.paired(paired.toLocaleDateString('en', { month: 'short', day: 'numeric', year: 'numeric' })));
  }
  return parts.join(' · ');
}

// Devices that have connected first, as the Mac app lists them.
function sortedDevices(devices) {
  return [...devices].sort((left, right) => {
    const leftNever = left.last_seen_at == null;
    const rightNever = right.last_seen_at == null;
    if (leftNever !== rightNever) return leftNever ? 1 : -1;
    return String(left.fingerprint).localeCompare(String(right.fingerprint));
  });
}

function deviceRow(device) {
  const name = deviceName(device);
  const detail = deviceDetail(device);
  const asking = state.removing?.fingerprint === device.fingerprint;
  const busy = asking && state.removing.busy;
  const row = el('li', { class: device.last_seen_at == null ? 'never' : null },
    el('div', { class: 'device-line' },
      el('div', {}, el('div', { class: 'device-name' }, name), el('div', { class: 'faint' }, detail)),
      asking ? null : el('button', { type: 'button', on: { click: () => {
        state.removing = { fingerprint: device.fingerprint, busy: false, error: null };
        render();
      } } }, COPY.remove)));
  if (asking) {
    row.append(el('div', { class: 'confirm', role: 'alertdialog', 'aria-label': COPY.removeTitle(name) },
      el('h2', {}, COPY.removeTitle(name)),
      el('p', { class: 'muted' }, COPY.removeBody(name)),
      state.removing.error ? el('p', { class: 'error' }, state.removing.error) : null,
      el('div', { class: 'actions tight' },
        el('button', { type: 'button', class: 'danger', disabled: busy, on: { click: () => removeDevice(device) } }, COPY.remove),
        el('button', { type: 'button', disabled: busy, on: { click: () => { state.removing = null; render(); } } }, COPY.cancel))));
  }
  return row;
}

async function removeDevice(device) {
  state.removing = { fingerprint: device.fingerprint, busy: true, error: null };
  render();
  try {
    await call('convey', { method: 'POST', path: '/app/network/unpair', port: port(),
      body: { fingerprint: device.fingerprint } });
  } catch (error) {
    // Already gone is what the owner asked for.
    if (error.code !== 'paired_device_not_found') {
      state.removing = { fingerprint: device.fingerprint, busy: false, error: COPY.removeFailed };
      render();
      return;
    }
  }
  await loadDevices();
}

function networkSection() {
  const network = state.network;
  // Shown where it is a choice, as on the journal's own page.
  if (!network || !(network.windows_asks === true || network.open === false)) return null;
  const n = COPY.network;
  const windows = network.windows_asks === true;
  let title = n.notReachable;
  let note = '';
  if (network.listening_on === 'local_network') {
    title = n.open;
    // Agents is what holds it open only when the owner's own choice is closed.
    const agents = network.agents_on_network === true
      ? [n.agentsNote, network.source === 'agents_on_network' ? n.agentsClosesNote : '']
      : [n.openNote];
    note = [...agents, windows ? n.firewallNote : ''].filter(Boolean).join(' ');
  } else if (network.listening_on === 'this_pc') {
    title = n.closed;
    note = [n.closedNote, windows ? n.windowsAsks : ''].filter(Boolean).join(' ');
  }
  const open = network.open === true;
  const section = el('section', { class: 'network' }, el('h2', {}, title), note ? el('p', { class: 'muted' }, note) : null);
  if (state.confirmClose) {
    section.append(el('div', { class: 'confirm', role: 'alertdialog', 'aria-label': n.closeConfirm },
      el('p', {}, n.closeConfirm),
      el('div', { class: 'actions tight' },
        el('button', { type: 'button', class: 'danger', disabled: state.networkBusy, on: { click: () => setNetwork(false) } }, n.closeCta),
        el('button', { type: 'button', disabled: state.networkBusy, on: { click: () => { state.confirmClose = false; render(); } } }, COPY.cancel))));
  } else if (network.agents_on_network !== true) {
    // Agents on your network keeps the journal open; that is turned off in agents.
    section.append(el('div', { class: 'actions tight' },
      el('button', { type: 'button', disabled: state.networkBusy, on: { click: () => {
        if (open) { state.confirmClose = true; render(); } else setNetwork(true);
      } } }, open ? n.closeCta : n.openCta)));
  }
  if (state.networkError) section.append(el('p', { class: 'error', role: 'alert' }, state.networkError));
  return section;
}

async function setNetwork(open) {
  state.networkBusy = true;
  state.networkError = null;
  render(true);
  let changed = true;
  try {
    await call('convey', { method: 'POST', path: `/app/network/local-network/${open ? 'open' : 'close'}`, port: port() });
  } catch {
    state.networkError = COPY.network.failed;
    changed = false;
  }
  state.networkBusy = false;
  state.confirmClose = false;
  try {
    state.network = await call('convey', { method: 'GET', path: '/app/network/api/local-network', port: port() });
  } catch { /* the section keeps what it last read */ }
  render(true);
  return changed;
}

function renderDevices() {
  const body = [el('h1', {}, COPY.devicesTitle)];
  if (display() !== 'running') {
    body.push(el('h2', {}, COPY.devicesNotRunningTitle), el('p', { class: 'muted' }, COPY.devicesNotRunningBody));
    return body;
  }
  if (pairing.phase) return [...body, pairingPanel()];
  if (state.devicesError) {
    body.push(el('p', { class: 'error' }, state.devicesError),
      el('button', { type: 'button', on: { click: loadDevices } }, COPY.tryAgain));
    return body;
  }
  if (state.devices == null) {
    body.push(el('p', { class: 'muted' }, COPY.devicesLoading));
    return body;
  }
  const yours = sortedDevices(state.devices.filter(device => !isPeerJournal(device)));
  const peers = sortedDevices(state.devices.filter(isPeerJournal));
  if (state.devices.length === 0) {
    body.push(el('h2', {}, COPY.devicesEmptyTitle), el('p', { class: 'muted' }, COPY.devicesEmptyBody));
  }
  for (const [title, rows] of [[COPY.yourDevices, yours], [COPY.peerJournals, peers]]) {
    if (rows.length) body.push(el('h2', {}, title), el('ul', { class: 'devices' }, rows.map(deviceRow)));
  }
  body.push(
    el('div', { class: 'actions' },
      el('button', { type: 'button', class: 'primary', on: { click: () => openPairing('device') } }, COPY.addDevice),
      el('button', { type: 'button', 'data-pairing': 'this-pc', on: { click: () => openPairing('this-pc') } }, COPY.pairThisPc)),
    networkSection());
  return body;
}

// --- a pairing link ---------------------------------------------------------

// One link at a time. `phase` is null while no link is open; `generation`
// retires the answers to a link the owner has moved past.
const pairing = { generation: 0, kind: null, phase: null, link: null, nonce: null, deadline: 0, copied: false,
  relay: false, device: null, unpair: null };
let pairingTimer = null;

function stopPairingTimer() {
  if (pairingTimer !== null) clearInterval(pairingTimer);
  pairingTimer = null;
}

function closePairing() {
  stopPairingTimer();
  pairing.generation += 1;
  Object.assign(pairing, { kind: null, phase: null, link: null, nonce: null, copied: false,
    relay: false, device: null, unpair: null });
}

function remainingSeconds() {
  return Math.max(0, Math.ceil((pairing.deadline - Date.now()) / 1000));
}

async function openPairing(kind) {
  stopPairingTimer();
  const generation = ++pairing.generation;
  Object.assign(pairing, { kind, phase: 'opening', link: null, nonce: null, copied: false,
    relay: false, device: null, unpair: null });
  render(true);
  let material;
  try {
    material = await call('convey', { method: 'POST', path: '/app/network/pair-start', port: port(),
      body: kind === 'this-pc' ? { same_machine: true } : {} });
  } catch (error) {
    if (generation !== pairing.generation) return;
    pairing.phase = error.code === 'local_network_closed' ? 'network-closed' : 'failed';
    render(true);
    return;
  }
  if (generation !== pairing.generation) return;
  if (!material || typeof material.pair_link !== 'string' || !material.pair_link ||
      typeof material.nonce !== 'string' || !material.nonce || !(material.expires_in > 0)) {
    pairing.phase = 'failed';
    render(true);
    return;
  }
  Object.assign(pairing, { phase: 'open', link: material.pair_link, nonce: material.nonce,
    deadline: Date.now() + material.expires_in * 1000, relay: material.relay === true });
  render(true);
  // The link for this PC is copied at once: the solstone app is where it goes.
  if (kind === 'this-pc') {
    pairing.copied = await copyText(material.pair_link);
    if (generation !== pairing.generation) return;
    render(true);
  }
  let tick = 0;
  pairingTimer = setInterval(async () => {
    if (generation !== pairing.generation || pairing.phase !== 'open') return;
    const remaining = remainingSeconds();
    if (remaining <= 0) {
      stopPairingTimer();
      pairing.phase = 'expired';
      render(true);
      return;
    }
    const countdown = $('pairing-countdown');
    if (countdown) countdown.textContent = COPY.countdown(remaining);
    tick += 1;
    if (tick % 2 !== 0) return;
    try {
      const status = await call('pairingStatus', { nonce: pairing.nonce, port: port() });
      if (generation !== pairing.generation || pairing.phase !== 'open') return;
      // The other device used the link. It asks its owner to confirm this
      // journal's mark before it sends anything, so the mark stays on screen.
      if (status?.used) {
        stopPairingTimer();
        Object.assign(pairing, { phase: 'paired', link: null, nonce: null, device: pairedDevice(status) });
        render(true);
        await loadDevices();
      }
    } catch { /* the next tick asks again */ }
  }, 1000);
}

// The device a used link added, when the journal still holds it.
function pairedDevice(status) {
  const device = status.device;
  if (!device || typeof device.cid !== 'string' || !device.cid) return null;
  return { cid: device.cid, label: typeof device.display_label === 'string' ? device.display_label.trim() : '' };
}

async function unpairPairedDevice() {
  const generation = pairing.generation;
  pairing.unpair = 'busy';
  render(true);
  let removed = true;
  try {
    await call('convey', { method: 'POST', path: '/app/network/unpair', port: port(),
      body: { fingerprint: pairing.device.cid } });
  } catch (error) {
    // Already gone is what the owner asked for.
    removed = error.code === 'paired_device_not_found';
  }
  if (generation !== pairing.generation) return;
  pairing.unpair = removed ? 'removed' : 'failed';
  render(true);
  if (removed) await loadDevices();
}

function pairedPanel(panel, markBox) {
  const removed = pairing.unpair === 'removed';
  const label = pairing.device?.label;
  let note = COPY.pairedNote;
  if (pairing.kind === 'this-pc') note = COPY.pairedNoteThisPc;
  else if (pairing.relay) note = COPY.pairedNoteAnywhere;
  put(panel,
    removed ? null : el('h2', { tabindex: '-1' }, label ? COPY.pairedHeading(label) : COPY.pairedHeadingUnnamed),
    markBox,
    removed ? null : el('p', { class: 'muted', 'data-pairing': 'paired-note' }, note),
    removed ? el('p', { role: 'status', 'data-pairing': 'unpaired' }, COPY.pairedRemoved) : null,
    pairing.unpair === 'failed' ? el('p', { class: 'error', role: 'alert' }, COPY.pairedRemoveFailed) : null,
    el('div', { class: 'pair-actions' },
      el('button', { type: 'button', class: 'primary', on: { click: () => { closePairing(); loadDevices(); } } }, COPY.pairedDone),
      pairing.device && !removed
        ? el('button', { type: 'button', class: 'danger', 'data-pairing': 'unpair', disabled: pairing.unpair === 'busy',
          on: { click: unpairPairedDevice } }, COPY.pairedUnpair)
        : null));
  return panel;
}

// The journal's own QR code maker, drawn as shapes rather than markup.
function qrCode(link) {
  const make = (prefix, payload) => {
    const qr = window.qrcode(0, 'M');
    qr.addData(prefix, 'Byte');
    if (payload) qr.addData(payload, 'Alphanumeric');
    qr.make();
    return qr;
  };
  const split = link.indexOf('#');
  let qr;
  try {
    qr = split >= 0 ? make(link.slice(0, split + 1), link.slice(split + 1)) : make(link, '');
  } catch {
    qr = make(link, '');
  }
  const count = qr.getModuleCount();
  const margin = 2;
  let d = '';
  for (let row = 0; row < count; row += 1) {
    for (let column = 0; column < count; column += 1) {
      if (qr.isDark(row, column)) d += `M${column + margin},${row + margin}h1v1h-1z`;
    }
  }
  const svgNs = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(svgNs, 'svg');
  const side = count + margin * 2;
  svg.setAttribute('viewBox', `0 0 ${side} ${side}`);
  svg.setAttribute('shape-rendering', 'crispEdges');
  const plate = document.createElementNS(svgNs, 'rect');
  plate.setAttribute('width', side);
  plate.setAttribute('height', side);
  plate.setAttribute('fill', '#ffffff');
  const modules = document.createElementNS(svgNs, 'path');
  modules.setAttribute('d', d);
  modules.setAttribute('fill', '#000000');
  svg.append(plate, modules);
  return el('div', { class: 'qr', role: 'img', 'aria-label': COPY.pairingCode }, svg);
}

function pairingPanel() {
  const panel = el('section', { class: 'pairing' });
  const thisPc = pairing.kind === 'this-pc';
  // This journal's mark, the whole time a link is open: the device being
  // added shows the same mark and asks its owner to confirm it. With no link
  // there is nothing to confirm, so no mark.
  const markBox = el('div');
  renderMarkCard(markBox, state.mark, COPY.confirmedLine, state.markUnavailable);
  const closeButton = el('button', { type: 'button', on: { click: () => { closePairing(); loadDevices(); } } }, COPY.close);
  if (pairing.phase === 'paired') return pairedPanel(panel, markBox);
  put(panel, el('h2', { tabindex: '-1' }, thisPc ? COPY.pairThisPc : COPY.addDevice));
  if (pairing.phase === 'opening') {
    put(panel, markBox, el('p', { class: 'muted' }, COPY.pairingOpening));
  } else if (pairing.phase === 'open') {
    const copiedNote = el('span', { class: 'faint', 'aria-live': 'polite' }, pairing.copied ? COPY.copied : '');
    closeButton.classList.add('end');
    put(panel,
      el('p', { class: 'muted' }, thisPc ? (pairing.copied ? COPY.pairingThisPc : COPY.pairingThisPcCopy) : COPY.pairingInstructions),
      el('div', { class: 'pair-show' }, thisPc ? null : qrCode(pairing.link), markBox),
      el('p', { class: 'pair-link' }, pairing.link),
      el('div', { class: 'pair-actions' },
        el('button', { type: 'button', on: { click: async () => {
          pairing.copied = await copyText(pairing.link);
          copiedNote.textContent = pairing.copied ? COPY.copied : '';
        } } }, COPY.copyLink),
        copiedNote,
        el('span', { class: 'muted', id: 'pairing-countdown' }, COPY.countdown(remainingSeconds())),
        closeButton));
    return panel;
  } else if (pairing.phase === 'expired') {
    put(panel,
      el('h2', {}, COPY.linkExpiredTitle),
      el('p', { class: 'muted' }, COPY.linkExpiredBody),
      el('div', { class: 'actions tight' },
        el('button', { type: 'button', class: 'primary', on: { click: () => openPairing(pairing.kind) } }, COPY.openFreshLink)));
  } else if (pairing.phase === 'network-closed') {
    put(panel,
      el('p', {}, COPY.pairingClosed),
      state.networkError ? el('p', { class: 'error', role: 'alert' }, state.networkError) : null,
      el('div', { class: 'actions tight' },
        el('button', { type: 'button', class: 'primary', disabled: state.networkBusy, on: { click: async () => {
          if (await setNetwork(true)) openPairing('device');
        } } }, COPY.network.openCta),
        el('button', { type: 'button', on: { click: () => openPage('devices') } }, COPY.relaySetup),
        el('button', { type: 'button', on: { click: () => openPairing('this-pc') } }, COPY.pairThisPc)));
  } else {
    put(panel,
      el('h2', {}, COPY.pairingFailedTitle),
      el('div', { class: 'actions tight' },
        el('button', { type: 'button', on: { click: () => openPairing(pairing.kind) } }, COPY.tryAgain)));
  }
  put(panel, el('div', { class: 'actions' }, closeButton));
  return panel;
}

async function copyText(text) {
  try {
    if (navigator.clipboard?.writeText) {
      // A clipboard the window can't reach may never answer; don't wait on it.
      const copied = await Promise.race([
        navigator.clipboard.writeText(text).then(() => true),
        new Promise(resolve => setTimeout(() => resolve(false), 2000)),
      ]);
      if (copied) return true;
    }
  } catch { /* fall back to a selection copy */ }
  const area = el('textarea');
  area.value = text;
  document.body.append(area);
  area.select();
  try {
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    area.remove();
  }
}

function formatBytes(bytes) {
  const units = ['bytes', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return unit === 0 ? `${value} ${units[0]}` : `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

function relative(seconds) {
  const elapsed = Date.now() / 1000 - seconds;
  if (elapsed < 60) return COPY.updates.justNow;
  const format = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });
  for (const [unit, size] of [['day', 86400], ['hour', 3600], ['minute', 60]]) {
    if (elapsed >= size) return format.format(-Math.floor(elapsed / size), unit);
  }
  return COPY.updates.justNow;
}

function renderUpdates() {
  const u = COPY.updates;
  const view = state.update;
  const body = [el('h1', {}, COPY.panes.updates)];
  if (!view) return body;

  const action = (label, act, primary) => el('button', {
    type: 'button', class: primary ? 'primary' : null,
    on: { click: () => call('update', { action: act }).catch(() => {}) },
  }, label);

  // 1. Installed version
  body.push(el('h2', {}, u.header(view.current ?? state.init?.app_version ?? '')));

  // 2. Last completed check line in every phase including unavailable when last_checked_at is set
  if (view.last_checked_at || view.phase !== 'unavailable') {
    const checked = view.last_checked_at
      ? u.lastChecked(relative(view.last_checked_at))
      : u.neverChecked;
    body.push(el('p', { class: 'muted' }, checked));
  }
  if (view.up_to_date) {
    body.push(el('p', { class: 'muted' }, u.upToDate));
  }

  // 3. Status, progress, actions by phase
  switch (view.phase) {
    case 'unavailable':
      body.push(el('h2', {}, u.unavailableTitle), el('p', { class: 'muted' }, u.unavailableSubtitle));
      break;

    case 'checking':
      if (view.downloaded_version) {
        body.push(el('p', {}, u.checkingWithDownloaded(view.downloaded_version)));
      } else {
        body.push(el('p', {}, u.checking));
      }
      break;

    case 'available': {
      const target = view.target_version ?? '';
      body.push(el('h2', {}, u.availableTitle(target)));
      if (view.downloaded_version && view.downloaded_version !== target) {
        body.push(el('p', { class: 'muted' }, u.downloadingRetainsDownloaded(view.downloaded_version)));
      }
      if (view.auto_download && !view.deferral) {
        body.push(el('p', { class: 'muted' }, u.availableSubtitle));
      } else if (view.deferral) {
        body.push(el('p', { class: 'muted' }, u.deferral[view.deferral] || u.deferral.unknown));
      }
      const actions = [
        action(u.download, 'download', true),
        action(view.last_checked_at ? u.checkAgain : u.checkNow, 'check', false),
      ];
      if (view.downloaded_version && view.next_start) {
        actions.push(action(u.install, 'install', false));
      }
      body.push(el('div', { class: 'actions' }, ...actions));
      break;
    }

    case 'downloading': {
      const target = view.target_version ?? '';
      body.push(el('h2', {}, u.downloadingTitle(target)));
      if (view.downloaded_version && view.downloaded_version !== target) {
        body.push(el('p', { class: 'muted' }, u.downloadingRetainsDownloaded(view.downloaded_version)));
      }
      const pct = view.percent ?? 0;
      body.push(el('progress', {
        'aria-label': u.downloadingTitle(target),
        'aria-valuemin': '0',
        'aria-valuemax': '100',
        'aria-valuenow': String(pct),
        max: 100,
        value: pct,
      }));
      break;
    }

    case 'ready':
      body.push(el('h2', {}, u.readyTitle(view.downloaded_version ?? '')));
      body.push(el('div', { class: 'actions' },
        action(u.install, 'install', true),
        action(view.last_checked_at ? u.checkAgain : u.checkNow, 'check', false),
      ));
      break;

    case 'installing':
      body.push(el('h2', {}, u.installingTitle(view.downloaded_version ?? '')));
      body.push(el('p', { class: 'muted' }, u.installingSubtitle));
      break;

    case 'failed': {
      if (view.failure === 'stop') {
        body.push(el('h2', {}, u.failureStopTitle), el('p', { class: 'muted' }, u.failureStopBody));
        body.push(el('div', { class: 'actions' }, action(u.install, 'install', true)));
      } else if (view.failure === 'install') {
        const retained = view.downloaded_version && view.next_start;
        body.push(el('h2', {}, u.failureInstallTitle), el('p', { class: 'muted' }, retained ? u.failureInstallBody : u.failureInstallMissing));
        body.push(el('div', { class: 'actions' }, retained ? action(u.install, 'install', true) : action(u.checkAgain, 'check', true)));
      } else if (view.failure === 'download') {
        body.push(el('h2', {}, u.failureDownloadTitle));
        let bodyMsg = u.failureDownloadBody;
        if (view.downloaded_version) {
          bodyMsg += ' ' + u.failureDownloadRetained(view.downloaded_version);
        }
        body.push(el('p', { class: 'muted' }, bodyMsg));
        const actions = [action(u.retryDownload, 'download', true)];
        if (view.downloaded_version && view.next_start) {
          actions.push(action(u.install, 'install', false));
        }
        body.push(el('div', { class: 'actions' }, ...actions));
      } else {
        body.push(el('h2', {}, u.failureCheckTitle));
        let bodyMsg = u.failureCheckBody;
        if (view.downloaded_version) {
          bodyMsg += ' ' + u.failureCheckRemains;
        }
        body.push(el('p', { class: 'muted' }, bodyMsg));
        const actions = [action(u.retryCheck, 'check', true)];
        if (view.downloaded_version && view.next_start) {
          actions.push(action(u.install, 'install', false));
        }
        body.push(el('div', { class: 'actions' }, ...actions));
      }
      break;
    }

    default: // idle
      if (view.downloaded_version && view.next_start) {
        body.push(el('h2', {}, u.readyTitle(view.downloaded_version)));
        body.push(el('div', { class: 'actions' },
          action(u.install, 'install', true),
          action(view.last_checked_at ? u.checkAgain : u.checkNow, 'check', false)
        ));
      } else {
        body.push(el('div', { class: 'actions' }, action(view.last_checked_at ? u.checkAgain : u.checkNow, 'check')));
      }
      break;
  }

  const prefs = (changes) => call('update', { action: 'prefs',
    auto_check: view.auto_check, auto_download: view.auto_download, interval: view.interval, ...changes })
    .then(next => { state.update = next; render(); });
  const autoCheck = el('input', { type: 'checkbox', checked: view.auto_check });
  autoCheck.addEventListener('change', () => prefs({ auto_check: autoCheck.checked }));
  const autoDownload = el('input', { type: 'checkbox', checked: view.auto_download });
  autoDownload.addEventListener('change', () => prefs({ auto_download: autoDownload.checked }));
  const interval = el('select', { 'aria-label': u.howOften },
    ['day', 'week', 'month'].map(value => el('option', { value, selected: view.interval === value }, u[value])));
  interval.addEventListener('change', () => prefs({ interval: interval.value }));
  body.push(
    el('h2', { class: 'section' }, u.automatic),
    el('label', { class: 'switch' }, autoCheck, u.autoCheck),
    el('label', { class: 'switch' }, autoDownload, u.autoDownload),
    el('div', { class: 'row pick' }, el('span', {}, u.howOften), interval),
    el('p', { class: 'faint' }, u.privacy),
  );
  return body;
}

listeners.update.push(({ view }) => {
  state.update = view;
  if (state.pane === 'updates') render();
});

listeners.shown.push(async () => {
  if (state.init?.setup_sequence_enabled !== true && firstRunWait.timer !== null) {
    if (!firstRunWait.opened) await openFirstRunPage();
    await checkFirstRunDone();
    return;
  }
  if (!$('main').hidden) {
    await refreshStatus();
    if (display() === 'running') await loadJournalFacts();
    if (state.init?.setup_sequence_enabled !== true) await recheckModels();
  }
});

async function recheckModels() {
  if (state.downloadingModels) return;
  try {
    const ready = await call('modelsReady');
    setModelsMissing(ready !== true);
    render();
  } catch { /* retain the last required-asset reading */ }
}


function render(force = false) {
  if ($('main').hidden) return;
  // An open pairing link is only redrawn by its own steps, so a status poll
  // doesn't move the owner's focus or the code they are scanning.
  if (!force && state.pane === 'devices' && pairing.phase) return;
  renderSidebar();
  const pane = $('pane');
  const focused = document.activeElement;
  const keepFocus = focused && pane.contains(focused) && focused.tagName === 'INPUT';
  if (keepFocus) return; // don't pull a field out from under the owner's typing
  if (paneFocus?.pane !== state.pane) paneFocus = null;
  if (focused && pane.contains(focused)) {
    paneFocus = { pane: state.pane, tag: focused.tagName, text: focused.textContent.trim() };
  } else if (focused && focused !== document.body) {
    paneFocus = null;
  }
  if (display() === 'other-account') {
    pane.replaceChildren(
      el('h2', {}, document.createTextNode(COPY.otherAccountTitle)),
      el('p', {}, document.createTextNode(COPY.otherAccountBody))
    );
    return;
  }
  pane.replaceChildren(...[panes[state.pane]()].flat().filter(Boolean));
  if (paneFocus) {
    const candidate = [...pane.querySelectorAll(paneFocus.tag.toLowerCase())].find(
      el => el.textContent.trim() === paneFocus.text && !el.disabled
    );
    if (candidate) candidate.focus();
    if (candidate || !state.flow?.busy) paneFocus = null;
  }
  // The pairing panel replaces the list in place, so the button the owner
  // pressed is gone: start them at the panel's heading.
  if (pairing.phase && state.pane === 'devices' && !pane.contains(document.activeElement)) {
    pane.querySelector('.pairing h2')?.focus();
  }
}

// --- launch ---------------------------------------------------------------

let booted = false;

async function continueStartup() {
  try {
    await refreshStatus(true);
  } catch (error) {
    $('loading').replaceChildren(
      el('p', { class: 'error' }, document.createTextNode(error.message || "couldn't read journal status")),
      el('button', {
        type: 'button',
        on: { click: () => continueStartup() },
      }, document.createTextNode(COPY.tryAgain))
    );
    return;
  }

  if (typeof state.status?.service?.installed !== 'boolean') {
    $('loading').replaceChildren(
      el('p', { class: 'error' }, document.createTextNode(state.message || "couldn't read journal status")),
      el('button', {
        type: 'button',
        on: { click: () => continueStartup() },
      }, document.createTextNode(COPY.tryAgain))
    );
    return;
  }

  // Opening the app starts the journal, as opening the Mac app does. A
  // sign-in start leaves that to the journal's own task, and an update puts
  // it back only if the owner had it running.
  if (state.init.launch === 'plain' && !['running', 'starting'].includes(display()) && state.status?.set_up) {
    await serviceAction('start');
  }

  if (state.init?.setup_sequence_enabled !== true) {
    if (!state.status?.set_up) {
      beginFirstRun(state.status?.service?.journal);
      return;
    }
    if (display() === 'running' && !await firstRunDone()) {
      waitForFirstRun();
      return;
    }
    await landLegacyHome();
    await recheckModels();
    return;
  }

  if (!booted) {
    booted = true;
    await call('flow', { intent: 'boot', task_installed: state.status.service.installed });
  }

  startPolling();

  if (state.status?.service?.installed === true && typeof state.status?.service?.starts_at_sign_in === 'boolean') {
    await syncAppRunEntry(state.status.service.starts_at_sign_in);
  }
  if (display() === 'running') {
    await loadJournalFacts();
  }
}

async function main() {
  $('admin-terminal').addEventListener('click', () => call('adminTerminal').catch(error => {
    state.message = error.message;
    render();
  }));
  $('quit').addEventListener('click', async () => {
    const running = ['running', 'starting'].includes(display()) || state.status?.service?.running;
    $('overlay-text').textContent = COPY.quitting;
    $('overlay').hidden = !running;
    $('main').hidden = running;
    await call('quit', { stop: Boolean(running) }).catch(() => {});
  });

  state.init = await call('init');
  // The locked mark the app last drew, until the running journal says.
  if (validMark(state.init?.mark)) state.mark = state.init.mark;
  await continueStartup();
}

main().catch(error => {
  $('loading').replaceChildren(el('p', { class: 'error' }, error.message));
});
