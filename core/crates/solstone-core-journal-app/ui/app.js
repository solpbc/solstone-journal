// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The journal app's window. Every word the owner reads is in COPY; the Mac
// journal app's words are kept wherever they say the same thing here.

import { renderMarkCard, markIconImages, sameMark, validMark } from './mark.js';

const COPY = {
  panes: {
    home: 'home',
    journal: 'name & location',
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
  },
  health: 'health',
  healthy: 'healthy',
  unknown: 'unknown',
  name: 'name',
  save: 'save',
  saved: 'saved',
  location: 'location',
  showInExplorer: 'show in File Explorer',
  copyAbout: 'copy',
  copiedAbout: 'copied',
  copyAboutFailed: "couldn't copy. select the text and copy it.",
  diskUsed: 'disk used',
  nameCanBeSavedLater: 'name can be saved later.',
  modelsMissing: "your journal couldn't download its sound model, so it can't label sounds in your audio yet.",
  modelsRetry: 'download it now',
  modelsFailed: "that didn't download either. check that this PC is online, then try again. if it keeps failing, reinstall the journal app.",
  modelsDownloading: 'downloading…',
  devicesTitle: 'devices',
  yourDevices: 'your devices',
  devicesLoading: 'loading devices…',
  devicesEmptyTitle: 'no devices yet',
  devicesEmptyBody: 'add a device and it joins your journal.',
  devicesNotRunningTitle: "your journal isn't running",
  devicesNotRunningBody: 'start the journal, then try again.',
  devicesLoadFailed: "couldn't load your devices",
  addDevice: 'add a device',
  unnamedDevice: 'unnamed device',
  peerJournals: 'peer journals',
  unnamedJournal: 'unnamed journal',
  neverConnected: 'never connected',
  lastSeenJustNow: 'last seen just now',
  lastSeenUnknown: 'last seen unknown',
  lastSeen: relative => `last seen ${relative}`,
  paired: date => `paired ${date}`,
  remove: 'remove',
  removeTitle: name => `remove ${name}?`,
  removeBody: name => `${name} loses access to your journal. you can add it again later.`,
  removeFailed: "couldn't remove this device",
  cancel: 'cancel',
  close: 'close',
  pairThisPc: 'pair the solstone app on this PC',
  pairingOpening: 'opening pairing…',
  pairingInstructions: 'scan this code or open the link on the device you want to add.',
  pairingThisPc: "this link works only for the solstone app on this PC. it's copied, so paste it into the solstone app to pair.",
  pairingThisPcCopy: 'this link works only for the solstone app on this PC. copy it, then paste it into the solstone app to pair.',
  pairingCode: 'pairing code',
  copyLink: 'copy link',
  copied: 'copied ✓',
  countdown: seconds => `expires in ${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`,
  linkExpiredTitle: 'link expired',
  linkExpiredBody: 'open a fresh link to add a device.',
  openFreshLink: 'open a fresh link',
  pairingFailedTitle: "couldn't open pairing",
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
  nameLocationTitle: "let's create your journal on this PC",
  chooseLocation: 'choose',
  continueButton: 'continue',
  journalFound: 'journal found',
  setupTitle: 'setting up your journal',
  setupSubtitle: 'this can take a minute.',
  setupStopped: "setup stopped before it finished.",
  finishFailed: "couldn't finish setting up your journal.",
  markFailed: "your journal couldn't do that just now.",
  whatHappened: 'what happened',
  steps: {
    doctor: 'checking this PC',
    journal: 'preparing your journal',
    install_models: 'downloading models',
    service: 'starting your journal',
    brain: 'setting up the model',
  },
  markTitle: 'your journal mark',
  markRevealSubtitle: 'this is your journal\'s mark. your journal is always private, only yours.',
  markRevealExplainer: "lock it in and this mark is your journal's, for good. it can't be changed later. try as many as you like first.",
  lockButton: 'lock it in',
  tryAnotherButton: 'try another',
  tryAnotherLoading: 'trying…',
  finishingTitle: 'finishing',
  finishingLoading: 'finishing…',
  finishedWithNotes: 'finished with notes',
  adoptLandingLine: 'nothing moved. your journal was always here. now it has a name.',
  // updates (the Mac app's update words)
  updates: {
    header: version => `journal ${version}`,
    checkNow: 'check now',
    checkAgain: 'check again',
    checking: 'checking…',
    neverChecked: 'never checked for updates',
    justNow: 'just now',
    upToDate: relative => `last checked ${relative} · journal is up to date`,
    lastChecked: relative => `last checked ${relative}`,
    availableTitle: version => `version ${version} is available`,
    availableSubtitle: version => `journal ${version} is ready to download.`,
    download: 'download',
    downloadingTitle: version => `downloading ${version}`,
    readyTitle: version => `ready to install ${version}`,
    readySubtitle: 'the update is downloaded and ready when you are.',
    install: 'install',
    installingTitle: version => `installing ${version}`,
    installingSubtitle: 'journal is handing off to the installer.',
    errorTitle: 'update check failed',
    errorMessage: "we couldn't check right now.",
    retry: 'retry',
    unavailableTitle: 'updates unavailable',
    unavailableSubtitle: "this build can't check for updates on its own. download the latest from solstone.app.",
    automatic: 'automatic updates',
    autoCheck: 'check for updates automatically',
    autoDownload: 'download updates in the background',
    howOften: 'how often',
    day: 'every day',
    week: 'every week',
    month: 'every month',
    privacy: 'no usage data is ever sent. update checks fetch the version list, then download the update itself in the background.',
  },
};

// --- the channel to the app ----------------------------------------------

const waiting = new Map();
let nextId = 1;

function call(cmd, args = {}) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    waiting.set(id, { resolve, reject });
    window.chrome.webview.postMessage(JSON.stringify({ id, cmd, args }));
  });
}

const listeners = { setup: [], update: [], shown: [] };

window.journalApp = {
  receive(message) {
    if (message.type === 'reply') {
      const entry = waiting.get(message.id);
      if (!entry) return;
      waiting.delete(message.id);
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
  name: '',
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
  modelsMissing: (() => { try { return localStorage.getItem('modelsMissing') === '1'; } catch { return false; } })(),
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
}

// --- status --------------------------------------------------------------

async function refreshStatus() {
  try {
    state.status = await call('status');
  } catch (error) {
    state.message = error.message;
  }
  render();
  return state.status;
}

async function serviceAction(action) {
  state.pending = action === 'stop' ? 'stopping' : 'starting';
  state.message = null;
  render();
  try {
    state.status = await call(action);
  } catch (error) {
    state.message = error.message;
    await refreshStatus();
  }
  state.pending = null;
  render();
  if (display() === 'running') await loadJournalFacts();
}

async function loadJournalFacts() {
  try {
    const identity = await call('convey', { method: 'GET', path: '/app/link/api/identity', port: port() });
    if (identity?.committed && validMark(identity.mark)) {
      state.mark = identity.mark;
      await applyIcon(state.mark);
    }
  } catch { /* the card shows the no-mark-yet treatment */ }
  try {
    const config = await call('convey', { method: 'GET', path: '/app/settings/api/config', port: port() });
    state.name = config?.journal?.name ?? '';
  } catch { /* the name field stays as it was */ }
  render();
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

// --- first run -----------------------------------------------------------

const firstRun = {
  name: '',
  location: '',
  holdsJournal: false,
  steps: [],
  current: null,
  error: null,
  mark: null,
  busy: false,
  adopted: false,
};

function renderFirstRun(step) {
  const root = $('firstrun');
  root.replaceChildren();
  const card = el('div', { class: 'firstrun-card' });
  root.append(card);
  if (step === 'name-location') {
    const nameInput = el('input', { type: 'text', value: firstRun.name, 'aria-label': COPY.name });
    nameInput.addEventListener('input', () => { firstRun.name = nameInput.value; });
    const locationInput = el('input', { type: 'text', value: firstRun.location, 'aria-label': COPY.location });
    locationInput.addEventListener('change', async () => {
      firstRun.location = locationInput.value;
      firstRun.holdsJournal = await call('holdsJournal', { path: firstRun.location });
      renderFirstRun('name-location');
    });
    put(card,
      el('h1', {}, COPY.nameLocationTitle),
      el('div', { class: 'field' }, el('h2', {}, COPY.name), nameInput),
      el('div', { class: 'field' },
        el('h2', {}, COPY.location),
        el('div', { class: 'row' }, locationInput,
          el('button', { type: 'button', on: { click: async () => {
            const picked = await call('pickFolder', { start: firstRun.location });
            if (picked) {
              firstRun.location = picked.path;
              firstRun.holdsJournal = picked.holds_journal;
              renderFirstRun('name-location');
            }
          } } }, COPY.chooseLocation)),
        firstRun.holdsJournal ? el('p', { class: 'faint' }, COPY.journalFound) : null),
      firstRun.error ? el('p', { class: 'error' }, firstRun.error) : null,
      el('div', { class: 'actions' },
        el('button', { type: 'button', class: 'primary', on: { click: runSetup } }, COPY.continueButton)),
    );
    nameInput.focus();
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
  } else if (step === 'mark') {
    const markBox = el('div');
    renderMarkCard(markBox, firstRun.mark, null);
    put(card,
      el('h1', {}, COPY.markTitle),
      el('p', { class: 'muted' }, COPY.markRevealSubtitle),
      markBox,
      el('p', { class: 'faint' }, COPY.markRevealExplainer),
      firstRun.error ? failure(COPY.markFailed, firstRun.error) : null,
      el('div', { class: 'actions centered' },
        el('button', { type: 'button', disabled: firstRun.busy, on: { click: tryAnotherMark } },
          firstRun.busy === 'trying' ? COPY.tryAnotherLoading : COPY.tryAnotherButton),
        el('button', { type: 'button', class: 'primary', disabled: firstRun.busy, on: { click: lockMark } },
          COPY.lockButton)),
    );
  } else if (step === 'finishing') {
    put(card,
      el('h1', {}, COPY.finishingTitle),
      el('p', { class: 'muted' }, COPY.finishingLoading),
      firstRun.error ? failure(COPY.finishFailed, firstRun.error) : null,
      firstRun.error ? el('div', { class: 'actions centered' },
        el('button', { type: 'button', on: { click: finishFirstRun } }, COPY.tryAgain)) : null,
    );
  }
  show('firstrun');
}

function beginFirstRun(journalPath) {
  firstRun.name = state.init.starting_name;
  firstRun.location = journalPath || state.init.default_location;
  firstRun.error = null;
  call('holdsJournal', { path: firstRun.location }).then(holds => {
    firstRun.holdsJournal = holds;
    renderFirstRun('name-location');
  });
  renderFirstRun('name-location');
}

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
  renderFirstRun('setup');
});

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
    await landHome();
    return;
  }
  try {
    const response = await call('convey', { method: 'GET', path: '/init/mark', port: port() });
    firstRun.mark = response.mark;
    if (response.locked) {
      await finishFirstRun();
    } else {
      renderFirstRun('mark');
    }
  } catch (error) {
    firstRun.error = error.message;
    renderFirstRun('setup');
  }
}

async function tryAnotherMark() {
  firstRun.busy = 'trying';
  firstRun.error = null;
  renderFirstRun('mark');
  try {
    const response = await call('convey', { method: 'POST', path: '/init/mark/regenerate', port: port() });
    firstRun.mark = response.mark;
  } catch (error) {
    firstRun.error = error.message;
  }
  firstRun.busy = false;
  renderFirstRun('mark');
}

async function lockMark() {
  firstRun.busy = 'locking';
  firstRun.error = null;
  renderFirstRun('mark');
  try {
    const response = await call('convey', { method: 'POST', path: '/init/mark/lock', port: port() });
    firstRun.mark = response.mark;
  } catch (error) {
    firstRun.error = error.message;
    firstRun.busy = false;
    renderFirstRun('mark');
    return;
  }
  firstRun.busy = false;
  await finishFirstRun();
}

async function finishFirstRun() {
  firstRun.error = null;
  renderFirstRun('finishing');
  try {
    if (await call('initProbe', { port: port() }) !== 'complete') {
      await call('convey', { method: 'POST', path: '/init/finalize', body: {}, port: port() });
    }
  } catch (error) {
    firstRun.error = error.message;
    renderFirstRun('finishing');
    return;
  }
  await landHome();
}

async function landHome() {
  // Name the journal what the owner typed, unless it already had a name.
  const name = firstRun.name.trim();
  if (name) {
    try {
      const config = await call('convey', { method: 'GET', path: '/app/settings/api/config', port: port() });
      if (!config?.journal?.name) {
        await call('convey', { method: 'PUT', path: '/app/settings/api/config', port: port(),
          body: { section: 'journal', data: { name } } });
      }
    } catch {
      state.message = COPY.nameCanBeSavedLater;
    }
  }
  if (firstRun.adopted) state.message = COPY.adoptLandingLine;
  state.pane = 'home';
  await refreshStatus();
  await call('syncSignInLaunch', { on: state.status?.service?.starts_at_sign_in ?? true }).catch(() => {});
  show('main');
  startPolling();
  await loadJournalFacts();
}

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
  return state.message ? el('p', { class: 'muted' }, state.message) : null;
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

const panes = {
  home() {
    const markBox = el('div');
    renderMarkCard(markBox, state.mark, COPY.confirmedLine);
    const key = display();
    let offer = null;
    if (key === 'running') {
      offer = el('button', { type: 'button', class: 'primary', on: { click: () => openPage('journal') } }, COPY.openJournal);
    } else if (key === 'stopped' || key === 'not-running') {
      offer = el('button', { type: 'button', class: 'primary', on: { click: () => serviceAction('start') } }, COPY.start);
    } else {
      offer = el('button', { type: 'button', on: { click: () => openPane('run') } }, COPY.runStateLink);
    }
    return [
      el('h1', {}, COPY.homeTitle),
      markBox,
      el('p', { class: 'name-line' }, state.name || state.init.starting_name),
      statusLine(),
      message(),
      state.modelsMissing ? el('div', { class: 'notice' },
        el('p', {}, COPY.modelsMissing),
        el('button', { type: 'button', disabled: state.downloadingModels, on: { click: downloadModels } },
          state.downloadingModels ? COPY.modelsDownloading : COPY.modelsRetry)) : null,
      el('p', { class: 'muted' }, COPY.homeLine),
      el('div', { class: 'actions' }, offer),
    ];
  },

  journal() {
    const input = el('input', { type: 'text', value: state.name, 'aria-label': COPY.name });
    const save = el('button', { type: 'button', on: { click: async () => {
      save.disabled = true;
      try {
        const response = await call('convey', { method: 'PUT', path: '/app/settings/api/config', port: port(),
          body: { section: 'journal', data: { name: input.value.trim() } } });
        state.name = response?.config?.journal?.name ?? input.value.trim();
        state.message = COPY.saved;
      } catch (error) {
        state.message = error.message;
      }
      render();
    } } }, COPY.save);
    input.addEventListener('keydown', event => { if (event.key === 'Enter') save.click(); });
    const journalPath = state.status?.service?.journal ?? '';
    return [
      el('h1', {}, COPY.panes.journal),
      el('div', { class: 'field' }, el('h2', {}, COPY.name), el('div', { class: 'row' }, input, save)),
      message(),
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

// Kept with the window's own storage, so the notice outlives a restart of the app.
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
  } catch {
    state.message = COPY.modelsFailed;
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
  const name = [device.display_label, device.device_label, device.observer_handle]
    .map(value => String(value ?? '').trim()).find(Boolean);
  return name || (isPeerJournal(device) ? COPY.unnamedJournal : COPY.unnamedDevice);
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
      el('button', { type: 'button', on: { click: () => openPairing('this-pc') } }, COPY.pairThisPc)),
    networkSection());
  return body;
}

// --- a pairing link ---------------------------------------------------------

// One link at a time. `phase` is null while no link is open; `generation`
// retires the answers to a link the owner has moved past.
const pairing = { generation: 0, kind: null, phase: null, link: null, nonce: null, deadline: 0, copied: false };
let pairingTimer = null;

function stopPairingTimer() {
  if (pairingTimer !== null) clearInterval(pairingTimer);
  pairingTimer = null;
}

function closePairing() {
  stopPairingTimer();
  pairing.generation += 1;
  Object.assign(pairing, { kind: null, phase: null, link: null, nonce: null, copied: false });
}

function remainingSeconds() {
  return Math.max(0, Math.ceil((pairing.deadline - Date.now()) / 1000));
}

async function openPairing(kind) {
  stopPairingTimer();
  const generation = ++pairing.generation;
  Object.assign(pairing, { kind, phase: 'opening', link: null, nonce: null, copied: false });
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
    deadline: Date.now() + material.expires_in * 1000 });
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
      // journal's mark before it sends anything.
      if (status?.used) {
        closePairing();
        await loadDevices();
      }
    } catch { /* the next tick asks again */ }
  }, 1000);
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
  renderMarkCard(markBox, state.mark, COPY.confirmedLine);
  const closeButton = el('button', { type: 'button', on: { click: () => { closePairing(); loadDevices(); } } }, COPY.close);
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
  if (view.phase === 'unavailable') {
    body.push(el('h2', {}, u.unavailableTitle), el('p', { class: 'muted' }, u.unavailableSubtitle));
    return body;
  }
  const action = (label, act, primary) => el('button', {
    type: 'button', class: primary ? 'primary' : null,
    on: { click: () => call('update', { action: act }).catch(() => {}) },
  }, label);
  const checked = view.last_checked_at
    ? (view.up_to_date ? u.upToDate(relative(view.last_checked_at)) : u.lastChecked(relative(view.last_checked_at)))
    : u.neverChecked;
  body.push(el('h2', {}, u.header(view.current ?? state.init.app_version)), el('p', { class: 'muted' }, checked));
  const v = view.version;
  switch (view.phase) {
    case 'checking':
      body.push(el('p', {}, u.checking));
      break;
    case 'available':
      body.push(el('h2', {}, u.availableTitle(v)), el('p', { class: 'muted' }, u.availableSubtitle(v)),
        el('div', { class: 'actions' }, action(u.download, 'download', true)));
      break;
    case 'downloading':
      body.push(el('h2', {}, u.downloadingTitle(v)), el('progress', { max: 100, value: view.percent ?? 0 }));
      break;
    case 'ready':
      body.push(el('h2', {}, u.readyTitle(v)), el('p', { class: 'muted' }, u.readySubtitle),
        el('div', { class: 'actions' }, action(u.install, 'install', true)));
      break;
    case 'installing':
      body.push(el('h2', {}, u.installingTitle(v)), el('p', { class: 'muted' }, u.installingSubtitle));
      break;
    case 'failed':
      body.push(el('h2', {}, u.errorTitle), el('p', { class: 'muted' }, u.errorMessage),
        el('div', { class: 'actions' }, action(u.retry, v ? 'download' : 'check')));
      break;
    default:
      body.push(el('div', { class: 'actions' }, action(view.last_checked_at ? u.checkAgain : u.checkNow, 'check')));
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
  if (!$('main').hidden) {
    await refreshStatus();
    if (display() === 'running') await loadJournalFacts();
    await recheckModels();
  }
});

// The notice stays only while the models are still missing: a repair from a
// terminal clears it the next time the window opens.
async function recheckModels() {
  if (!state.modelsMissing || state.downloadingModels) return;
  if (await call('modelsReady').catch(() => false)) {
    setModelsMissing(false);
    render();
  }
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
  pane.replaceChildren(...[panes[state.pane]()].flat().filter(Boolean));
  // The pairing panel replaces the list in place, so the button the owner
  // pressed is gone: start them at the panel's heading.
  if (pairing.phase && state.pane === 'devices' && !pane.contains(document.activeElement)) {
    pane.querySelector('.pairing h2')?.focus();
  }
}

// --- launch ---------------------------------------------------------------

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
  if (validMark(state.init.mark)) state.mark = state.init.mark;
  const status = await refreshStatus();
  if (!status?.set_up) {
    beginFirstRun(status?.service?.journal);
    return;
  }
  show('main');
  render();
  startPolling();
  // Opening the app starts the journal, as opening the Mac app does. A
  // sign-in start leaves that to the journal's own task, and an update puts
  // it back only if the owner had it running.
  if (state.init.launch === 'plain' && !['running', 'starting'].includes(display())) {
    await serviceAction('start');
  }
  await call('syncSignInLaunch', { on: state.status?.service?.starts_at_sign_in ?? true }).catch(() => {});
  if (display() === 'running') {
    // A journal set up from the terminal may not have met its mark yet.
    const probe = await call('initProbe', { port: port() }).catch(() => 'complete');
    if (probe === 'incomplete') {
      firstRun.name = state.name || state.init.starting_name;
      await routeAfterSetup();
      return;
    }
    await loadJournalFacts();
  }
  await recheckModels();
}

main().catch(error => {
  $('loading').replaceChildren(el('p', { class: 'error' }, error.message));
});
