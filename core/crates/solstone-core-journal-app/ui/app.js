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
  addDevice: 'add a device',
  unnamedDevice: 'unnamed device',
  devicesInBrowser: 'adding and removing devices happens in your journal, in your browser.',
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
      if (message.ok) entry.resolve(message.value); else entry.reject(new Error(message.error));
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
      el('ul', { class: 'steps' }, firstRun.steps.filter(item => COPY.steps[item.step]).map(item =>
        el('li', { class: item.state }, COPY.steps[item.step]))),
      firstRun.error ? el('p', { class: 'error' }, firstRun.error) : null,
      firstRun.error ? el('div', { class: 'actions' },
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
      firstRun.error ? el('p', { class: 'error' }, firstRun.error) : null,
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
      firstRun.error ? el('p', { class: 'error' }, firstRun.error) : null,
      firstRun.error ? el('div', { class: 'actions' },
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
        el('button', { type: 'button', class: 'link', on: { click: () =>
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
        const text = about;
        try {
          if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(text);
          else {
            const area = el('textarea');
            area.value = text;
            document.body.append(area); area.select();
            try {
              if (!document.execCommand('copy')) throw new Error('clipboard unavailable');
            } finally { area.remove(); }
          }
          button.textContent = 'copied';
        } catch { state.message = "couldn't copy. select the text and copy it."; render(); }
      } } }, 'copy'),
    ];
  },

  devices() {
    const body = [el('h1', {}, COPY.devicesTitle)];
    if (display() !== 'running') {
      body.push(el('h2', {}, COPY.devicesNotRunningTitle), el('p', { class: 'muted' }, COPY.devicesNotRunningBody));
      return body;
    }
    if (state.devicesError) {
      body.push(el('p', { class: 'error' }, state.devicesError),
        el('button', { type: 'button', on: { click: loadDevices } }, COPY.tryAgain));
      return body;
    }
    if (state.devices == null) {
      body.push(el('p', { class: 'muted' }, COPY.devicesLoading));
      return body;
    }
    if (state.devices.length === 0) {
      body.push(el('h2', {}, COPY.devicesEmptyTitle), el('p', { class: 'muted' }, COPY.devicesEmptyBody));
    } else {
      body.push(el('h2', {}, COPY.yourDevices), el('ul', { class: 'devices' },
        state.devices.map(device => el('li', {},
          device.display_label || device.device_label || COPY.unnamedDevice))));
    }
    body.push(
      el('p', { class: 'faint' }, COPY.devicesInBrowser),
      el('div', { class: 'actions' },
        el('button', { type: 'button', class: 'primary', on: { click: () => openPage('devices') } }, COPY.addDevice)));
    return body;
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

async function loadDevices() {
  state.devicesError = null;
  try {
    const response = await call('convey', { method: 'GET', path: '/app/network/api/devices', port: port() });
    state.devices = Array.isArray(response?.devices) ? response.devices : [];
  } catch (error) {
    state.devices = null;
    state.devicesError = error.message;
  }
  render();
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
  }
});

function render() {
  if ($('main').hidden) return;
  renderSidebar();
  const pane = $('pane');
  const focused = document.activeElement;
  const keepFocus = focused && pane.contains(focused) && focused.tagName === 'INPUT';
  if (keepFocus) return; // don't pull a field out from under the owner's typing
  pane.replaceChildren(...[panes[state.pane]()].flat().filter(Boolean));
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
}

main().catch(error => {
  $('loading').replaceChildren(el('p', { class: 'error' }, error.message));
});
