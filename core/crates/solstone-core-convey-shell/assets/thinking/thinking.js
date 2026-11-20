// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Thinking, on the view contract: the script only registers, and the view
// starts each time it is mounted. Everything it adds outside its markup goes
// through `ctx` and ends with it: its two page listeners, its timers and the
// sleeps its polls wait on, its history entries and its reads. A read the
// view still has out when it goes never answers, so a poll stops at its next
// read or sleep. The next mount starts from what the journal says, as a full
// load does, so an install, a turn-on or a sign-in still running picks up
// there.
window.SolViews.register('thinking', { mount: (ctx) => {
  // The view's only address. Its API lives under it.
  const THINKING_PATH = '/app/thinking/';
  // The view's own markup. Once it has gone, a write that answers late draws
  // into this markup, out of the page, never into a later mount's.
  const viewRoot = document.getElementById('providers');
  const NO_MARKUP = {querySelector: () => null, querySelectorAll: () => []};
  // The shell's focus helpers, in its boot part.
  const SolFocus = window.SolFocus;
  let markupRoot = null;
  let readyReported = false;

  // Readiness, once per mount.
  function reportThinkingReady(readyState) {
    if (readyReported || ctx.disposed) return;
    readyReported = true;
    ctx.ready(readyState);
  }

  function markup() {
    return markupRoot || document;
  }

  // An entry of thinking's own, on its own path. Nothing once it has gone:
  // the address belongs to whatever came next.
  function writeEntry(kind, hash) {
    if (ctx.disposed) return;
    ctx.history[kind](`${THINKING_PATH}${ctx.route.search || ''}${hash}`);
  }

  function disposeThinking() {
    const modal = $('thinkingRunsPromptModal');
    markupRoot = viewRoot || NO_MARKUP;
    // The move places focus, so the dialog's opener doesn't get it first.
    try {
      if (modal?.open) modal.close();
    } catch (_err) {}
    window.ConveyModalLayer?.reconcile?.({restore: false});
  }
  ctx.onDispose(disposeThinking);

  // A full load of /app/thinking is answered by a redirect to /app/thinking/.
  // In the page there is none, so the view takes that address itself.
  if (window.location.pathname === '/app/thinking') {
    ctx.history.replace(`${THINKING_PATH}${window.location.search}${window.location.hash}`);
  }

  // Thoughts per hour: its own section (attachPace, below).
  const PACE_HASH = '#per-hour';
  let pace = null;

  const state = {
    providers: {},
    keys: {},
    access: null,
    savedLane: null,
    localCapability: null,
    hostCapabilities: null,
    localModels: [],
    localAvailability: null,
    install: null,
    installPollGeneration: 0,
    runtimePollGeneration: 0,
    confidentialPollGeneration: 0,
    localConfirmGeneration: 0,
    brainFollowGeneration: 0,
    selectedByoProvider: '',
    byoMode: 'pick',
    byoSelectedModel: '',
    byoModelResolutionTargets: [],
    pendingSwitchTarget: '',
    // The last refusal a local-setup action met. The card re-renders on every
    // poll tick, so the refusal lives here rather than only in the DOM.
    localSetupError: '',
    localSetupTone: '',
    runsNavigationGeneration: 0,
    runsRouteKey: '',
    runsLastHash: '',
    runsRequestGenerations: {day: 0, run: 0, prompt: 0, output: 0},
    runsInFlight: {day: null, run: null, prompt: null, output: null},
    runsCache: {
      day: new Map(),
      run: new Map(),
      prompt: new Map(),
      output: new Map(),
    },
    runsFailuresOnly: false,
    runsGroupKey: '',
    runsGroupOpen: new Map(),
    runsGroupShown: new Map(),
    runsFacet: '',
    runsFacetTitles: new Map(),
    runsFacetExplicit: false,
    // The last unfiltered run count for a day, {day, count}. The server
    // narrows the day payload to the picked facet, so a filtered render has no
    // way to count the day it is filtering (G2-B02).
    runsDayTotal: null,
    runsSelectedUseId: '',
    runsDetail: null,
    runsModalFocus: null,
    chatgpt: {
      screen: 'card',
      status: null,
      models: [],
      modelsLoading: false,
      selectedModel: '',
      attempt: null,
      pollToken: 0,
      pollTimer: null,
      pollInFlight: null,
      refreshGeneration: 0,
      notice: null,
      pendingNotice: null,
      liveStatus: 'pending_status',
      lastRefreshAt: 0,
      isSigningIn: false,
      isCancelling: false,
      isSigningOut: false,
      isSavingModel: false,
      activeLaunch: null,
    },
  };
  let copy = {};
  const confidentialAudio = {known: true, busy: false, choice: false, epoch: 0, readSequence: 0};
  const CHATGPT_ATTEMPT_STORAGE_KEY = 'solstone.thinking.chatgptAttempt';
  let chatGptStartGateOwner = null;

  function releaseChatGptGate(owner) {
    if (chatGptStartGateOwner === owner) {
      chatGptStartGateOwner = null;
    }
  }

  function launchCurrent(launch) {
    return Boolean(launch && state.chatgpt.activeLaunch === launch && !launch.cancelled);
  }

  function closeChatGptPopup(launch) {
    const popup = launch?.popup;
    if (launch) launch.popup = null;
    try { if (popup && !popup.closed) popup.close(); } catch (_err) {}
  }
  const confidentialTerminalPhases = new Set(['not_verified', 'repair_needed', 'early_access', 'journal_limit']);
  const installInFlightStates = new Set(['resolving', 'downloading', 'verifying', 'installing']);
  const installTerminalStates = new Set(['idle', 'installed', 'failed', 'unavailable']);
  const localServerUnhealthyReasons = new Set(['local_server_unhealthy', 'server_unhealthy']);
  const localSetupMissingReasons = new Set([
    'local_model_missing',
    'model_missing',
    'binary_missing',
    'runtime_missing',
    'artifact-not-ready',
    'manifest-missing',
    'manifest_missing',
    'manifest_pin_mismatch',
  ]);
  const pollIntervalMs = 1500;
  // The turn-on can wait 15 minutes, then 60 minutes after the portal says a
  // subscription is needed. The page waits a minute past that.
  const confidentialPollMaxMs = 76 * 60 * 1000;
  const views = new Set(['main', 'byo-setup', 'confidential-setup', 'local-setup', 'lane-switch']);
  // A day of ordinary processing is well over a thousand runs. Group them by
  // talent and page each group; a light day still renders in full.
  const thinkingRunsPageSize = 50;
  const thinkingRunsExpandAllBelow = 50;
  // Owner names for the talent ids whose own authored title is not one. These
  // win over the title: `entities:entity_observer` is titled "Entity Observer",
  // and "observer" is retired vocabulary, while "record" is a word the app
  // never aims at the owner's material — and talent prompts are not this app's
  // to edit. Everything else takes the talent's authored title from the day
  // payload (lowercased house style, see talentTitles); only a talent that
  // reports no title at all falls back to the humanized id, and the exact id
  // stays in the group's disclosure either way (X-04).
  const talentLabels = {
    'entities:detection': window.JournalI18n.render('journal.window.thinking.window.state.text.entity.detection'),
    'entities:entity_observer': window.JournalI18n.render('journal.window.thinking.window.state.text.entity.facts'),
    'entities:possible_names': window.JournalI18n.render('journal.window.thinking.window.state.text.possible.names'),
    screen: window.JournalI18n.render('journal.window.thinking.window.state.text.screen.descriptions'),
  };
  // Filled from each day payload's `talents` map; empty until one arrives.
  let talentTitles = {};
  const providerEnv = {
    anthropic: 'ANTHROPIC_API_KEY',
    google: 'GOOGLE_API_KEY',
    openai: 'OPENAI_API_KEY',
  };
  // X-03: the confidential lane is a provider like any other as far as a run
  // record is concerned, and with no entry here a confidential run rendered
  // its raw slug ("spp"). It is named the way the lane labels name it.
  const fallbackProviderLabels = {
    anthropic: 'Claude',
    google: 'Gemini',
    openai: 'GPT',
    local: window.JournalI18n.render('journal.window.thinking.provider.local'),
    spp: window.JournalI18n.render('journal.window.shared.date.format.lane.spp'),
  };
  let providerLabels = fallbackProviderLabels;
  // Mirrors stats' MODEL_LABELS (solstone-core-stats-web/assets/static/token-card.js):
  // a local model can self-report a raw wire id that reads fine in a log but
  // not as a fact on a table row. Keeping the same resolved name here means
  // the same runs read the same way in both apps (G2-43); the exact raw id
  // stays in this view's own "exact ids" disclosure, untouched by this.
  const LOCAL_MODEL_LABELS = {
    'local/qwen3.5-4b': window.JournalI18n.render('journal.window.thinking.window.explanation.qwen.b.local'),
  };
  const providerTerms = {
    anthropic: 'https://www.anthropic.com/legal/commercial-terms',
    google: 'https://ai.google.dev/gemini-api/terms',
    openai: 'https://openai.com/policies/row-terms-of-use',
  };
  const googleModelResolutionTargetsField = 'google_model_resolution_targets';

  function $(id) {
    if (markupRoot) return markupRoot.querySelector(`#${id}`);
    return document.getElementById(id);
  }

  // Every draw writes only what differs from what the page already shows, so
  // a poll whose answer changed nothing leaves the page as it is: no node is
  // replaced, and a screen reader doesn't read the focused control again.
  function writeText(el, text) {
    if (!el) return;
    const next = String(text || '');
    // Equal text over a link or other markup is still a change.
    const nodes = el.childNodes;
    const same = el.textContent === next && (!nodes
      || (nodes.length === 0 ? next === '' : nodes.length === 1 && nodes[0].nodeType === 3));
    if (!same) el.textContent = next;
  }

  function hasAttr(el, name) {
    return typeof el.hasAttribute === 'function' ? el.hasAttribute(name) : el.getAttribute?.(name) != null;
  }

  // An attribute set to its own value still counts as a change to the page.
  // A link's own properties are written as properties, as before.
  const LINK_PROPERTIES = new Set(['href', 'target', 'rel']);
  function writeAttr(el, name, value) {
    if (!el) return;
    if (value === null || value === undefined || value === false) {
      if (hasAttr(el, name)) el.removeAttribute(name);
      return;
    }
    const next = value === true ? '' : String(value);
    if (el.getAttribute?.(name) === next) return;
    if (LINK_PROPERTIES.has(name)) el[name] = next;
    else el.setAttribute(name, next);
  }

  function externalLink(el, href) {
    writeAttr(el, 'href', href);
    writeAttr(el, 'target', '_blank');
    writeAttr(el, 'rel', 'noopener noreferrer');
  }

  function writeHidden(el, hidden) {
    const held = heldByPress(el);
    if (held) {
      held.hidden = !!hidden;
      return;
    }
    if (el && el.hidden !== !!hidden) el.hidden = !!hidden;
  }

  function writeDisabled(el, disabled) {
    const held = heldByPress(el);
    if (held) {
      held.disabled = !!disabled;
      return;
    }
    if (el && el.disabled !== !!disabled) el.disabled = !!disabled;
  }

  // A part built from several values is built again only when they change,
  // or when something else has written into it since it was built.
  const drawnParts = new WeakMap();
  function drawPart(el, values, build) {
    if (!el) return;
    const key = JSON.stringify(values);
    const drawn = drawnParts.get(el);
    const nodes = Array.from(el.childNodes || el.children || []);
    if (drawn && drawn.key === key && drawn.nodes.length === nodes.length
      && drawn.nodes.every((node, i) => node === nodes[i])) return;
    build();
    drawnParts.set(el, {key, nodes: Array.from(el.childNodes || el.children || [])});
  }

  // ---- focus that stays where the owner is
  //
  // Thinking draws by showing, hiding and turning off its own controls, so a
  // press or a poll can take away the control that has focus. Each draw notes
  // the focused control first, and if the draw took it away, focus goes to the
  // first of: where the press that was out says, the nearest control left in
  // the same part of the page, and that part's heading. Focus anywhere else,
  // or nowhere, is left alone.

  let drawDepth = 0;
  const presses = [];
  const FOCUS_CONTROLS = 'button, a[href], input, select, textarea, summary';

  function heldByPress(el) {
    return el ? presses.find((entry) => entry.busy && entry.control === el) : undefined;
  }

  function partOf(el) {
    return el?.closest?.('[data-view], [data-thinking-section]') || null;
  }

  function shownView() {
    return Array.from(markup().querySelectorAll('#providers [data-view]')).find((view) => !view.hidden)?.dataset.view || 'main';
  }

  function partHeading(view) {
    return $({
      main: 'thinkingHeading',
      'local-setup': 'localSetupHeading',
      'confidential-setup': 'confidentialSetupHeading',
      'byo-setup': 'byoSetupTitle',
      'lane-switch': 'switchHeading',
    }[view] || 'thinkingHeading');
  }

  // Back on the lanes from a setup, the lane's own control is where the owner was.
  function laneControl(lane) {
    return lane ? $(`${lane}LaneStatus`) : null;
  }

  function laneOfView(view) {
    if (view === 'lane-switch') return state.pendingSwitchTarget || '';
    return {'local-setup': 'local', 'confidential-setup': 'confidential', 'byo-setup': 'byo'}[view] || '';
  }

  // The control's own row first (next, then previous), then the rest of the part.
  function nearestControls(part, from) {
    const near = (root) => {
      const all = Array.from(root.querySelectorAll(FOCUS_CONTROLS))
        .filter((el) => el.getAttribute('tabindex') !== '-1' && el.type !== 'hidden');
      const at = all.indexOf(from);
      return at < 0 ? all : [...all.slice(at + 1), ...all.slice(0, at).reverse()];
    };
    const row = from?.parentElement && from.parentElement !== part ? near(from.parentElement) : [];
    return [...row, ...near(part)];
  }

  function focusTargets(held, finished) {
    const targets = [];
    const part = partOf(held);
    const own = [finished, ...presses].find((entry) => entry
      && (entry.control === held || (part && partOf(entry.control) === part)));
    if (own?.landing) targets.push(...own.landing());
    if (part && SolFocus.isShown(part)) {
      targets.push(...nearestControls(part, held));
    } else if (part) {
      targets.push(laneControl(laneOfView(part.dataset.view)));
    }
    targets.push(partHeading(shownView()), $('thinkingHeading'));
    return targets.filter(Boolean);
  }

  function settleFocus(held, finished) {
    if (!held || ctx.disposed) return;
    if (SolFocus.isShown(held) && !held.disabled) return;
    // A browser can still report a control it has just turned off as focused.
    if (document.activeElement === held) held.blur?.();
    // Focus the owner has since moved to something still shown is left there.
    SolFocus.refocus(document, true, focusTargets(held, finished));
  }

  // Every draw goes through here; only the outermost one settles focus.
  function drawn(draw, finished = null) {
    if (drawDepth > 0) return draw();
    const active = document.activeElement;
    const held = active && active !== document.body && viewRoot?.contains(active) ? active : null;
    drawDepth += 1;
    try {
      return draw();
    } finally {
      drawDepth -= 1;
      settleFocus(held, finished);
    }
  }

  // A press whose write is out keeps its control: busy, still drawn, and deaf
  // to a second press, never `disabled`. A draw that would hide or turn it off
  // meanwhile is kept for the end, so focus moves once, when the press has
  // finished: to `landing()` if the control has gone, as each press says.
  async function pressing(control, landing, work, {busy = true} = {}) {
    if (!control || SolFocus.isBusy(control) || ctx.disposed) return undefined;
    const entry = {control, landing, busy, hidden: null, disabled: null};
    presses.push(entry);
    if (busy) SolFocus.setBusy(control, true);
    try {
      return await work();
    } finally {
      presses.splice(presses.indexOf(entry), 1);
      // Once thinking has gone, its markup is out of the page: touch nothing.
      if (!ctx.disposed) {
        drawn(() => {
          if (busy) SolFocus.setBusy(control, false);
          if (entry.hidden !== null) writeHidden(control, entry.hidden);
          if (entry.disabled !== null) writeDisabled(control, entry.disabled);
        }, entry);
      }
    }
  }

  function setText(id, message) {
    writeText($(id), message);
  }

  function writeMessage(el, message, tone = '') {
    if (!el) return;
    writeText(el, message);
    if (tone) {
      if (el.getAttribute?.('data-tone') !== tone || el.dataset?.tone !== tone) el.dataset.tone = tone;
    } else if (hasAttr(el, 'data-tone') || el.dataset?.tone !== undefined) {
      el.removeAttribute('data-tone');
    }
  }

  function setMessage(id, message, tone = '') {
    writeMessage($(id), message, tone);
  }

  function showLocalSetupError(message, tone = 'error') {
    state.localSetupError = message || '';
    state.localSetupTone = state.localSetupError ? tone : '';
    setMessage('localSetupMessage', state.localSetupError, state.localSetupTone);
  }

  function clearLocalSetupError() {
    state.localSetupError = '';
    state.localSetupTone = '';
  }

  // What local setup says when a request is turned down or its state can't be
  // read. The server's reason is engineering text ("installer admission timed
  // out"); apiJson has already put it, with the status and request id, in the
  // diagnostic console. "check again" is the page's own button: it re-reads the
  // true state, so a refusal that won't clear on its own doesn't loop the owner.
  const localSetupRefusals = {
    check: window.JournalI18n.render('journal.window.thinking.window.explanation.couldn.t.check.local.setup.check'),
    start: window.JournalI18n.render('journal.window.thinking.window.explanation.local.setup.didn.t.start.check'),
    busy: window.JournalI18n.render('journal.window.thinking.window.explanation.local.setup.is.already.running.check'),
    cancel: window.JournalI18n.render('journal.window.thinking.window.explanation.local.setup.couldn.t.be.cancelled'),
    // Turning local on and clearing a URL are also asked from views with no
    // "check again", and pressing the same button again is harmless.
    activate: window.JournalI18n.render('journal.window.thinking.window.explanation.local.thinking.couldn.t.be.turned'),
    clear: window.JournalI18n.render('journal.window.thinking.window.explanation.your.own.endpoint.couldn.t.be'),
  };

  // A refused request, rethrown in owner words. The setup redirect passes
  // through untouched: it is already taking the page to /init.
  function localSetupRefusal(err, kind) {
    if (err?.cause === 'setup_required') return err;
    // Turning local on or clearing a URL can be refused for a state the owner
    // has to change first ("turn off confidential processing first, …"). That
    // answer is written for the owner and says what to do, so it stays.
    // A computer that can't run local thinking is told so; checking again won't help.
    const says = ((kind === 'activate' || kind === 'clear')
      && err?.reasonCode === 'invalid_operation_for_state' && err?.message)
      || (err?.reasonCode === 'host_ineligible' && err?.message)
      // The change landed; saying it didn't would be false.
      || (err?.reasonCode === 'settings_saved_unlogged' && err?.message);
    const refusal = new Error(says || localSetupRefusals[kind]);
    // Setup already running is not a failure, so it doesn't read as one.
    refusal.localSetupTone = kind === 'busy' ? '' : 'error';
    return refusal;
  }

  // Every failure that reaches the local-setup line comes through here, so the
  // owner reads only words written for them, never a server's or browser's.
  function showLocalSetupFailure(err) {
    if (err?.cause === 'setup_required') return;
    if (typeof err?.localSetupTone === 'string') {
      showLocalSetupError(err.message, err.localSetupTone);
      return;
    }
    // A server answer is already in the console; a failed fetch or a page fault isn't.
    if (err?.status === undefined) {
      window.logError?.(typeof err?.cause === 'object' && err.cause ? err.cause : err, { context: 'thinking-local-setup' });
    }
    showLocalSetupError(localSetupRefusals.check);
  }

  function localCapabilityBlocked() {
    return state.localCapability?.overall === 'blocked';
  }

  function localCapabilityRefusedUnknown() {
    return state.localCapability?.overall === 'unknown'
      && state.localCapability?.local_admitted !== true;
  }

  function setElementData(el, key, attr, value) {
    if (!el) return;
    if (typeof el.getAttribute === 'function' && el.getAttribute(attr) === String(value)
      && (!el.dataset || el.dataset[key] === String(value))) return;
    if (el.dataset) el.dataset[key] = value;
    if (typeof el.setAttribute === 'function') el.setAttribute(attr, value);
  }

  function removeElementData(el, key, attr) {
    if (!el) return;
    if (el.dataset) delete el.dataset[key];
    if (typeof el.removeAttribute === 'function') el.removeAttribute(attr);
  }

  function setLocalReason(el, text) {
    if (!el) return;
    writeText(el, text);
    setElementData(el, 'localReason', 'data-local-reason', '');
    // classList.remove writes the attribute even when neither class is there.
    ['error', 'error-message'].forEach((name) => el.classList?.toggle(name, false));
  }

  function clearLocalReason(el) {
    if (!el) return;
    removeElementData(el, 'localReason', 'data-local-reason');
  }

  function setLink(id, url, text) {
    const el = $(id);
    if (!el) return;
    writeHidden(el, !url);
    writeAttr(el, 'href', url || '');
    writeText(el, url ? text : '');
  }

  function setHidden(id, hidden) {
    writeHidden($(id), hidden);
  }

  function setButtonState(id, visible, disabled) {
    const button = $(id);
    if (!button) return;
    writeHidden(button, !visible);
    writeDisabled(button, disabled);
  }

  function setButtonText(id, text) {
    writeText($(id), text);
  }

  function installIsInFlight(status) {
    return installInFlightStates.has(status?.install_state || '');
  }

  function installIsTerminal(status) {
    return installTerminalStates.has(status?.install_state || '');
  }

  function formatInstallBytes(received, total) {
    if (received === null || received === undefined || total === null || total === undefined) return '';
    const gb = 1e9;
    return window.JournalI18n.render('journal.window.thinking.local.download.quantity', [
      ['received', (Number(received) / gb).toFixed(1)],
      ['total', (Number(total) / gb).toFixed(1)],
    ]);
  }

  function installCopyForStatus(status, text) {
    const phase = status?.install_state || '';
    const phaseLabel = text?.phases?.[phase] || phase;
    if (installInFlightStates.has(phase)) {
      const bytesLine = formatInstallBytes(status.progress_bytes_received, status.progress_bytes_total);
      return {
        pill: text?.pill_inflight || '',
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: phaseLabel,
        message: bytesLine || phaseLabel,
        notice: text?.notice_inflight || '',
        activate: false,
        bootstrap: false,
        bootstrapLabel: text?.install || '',
        cancel: true,
        cancelLabel: text?.cancel || window.JournalI18n.render('journal.window.thinking.button.label.cancel'),
        tone: '',
      };
    }
    if (phase === 'failed') {
      const code = typeof status?.error_code === 'string' ? status.error_code.replace(/_/g, '-') : '';
      const stopped = code === 'install-cancelled' || code === 'install-interrupted'
        || status.install_error === 'install_cancelled' || status.install_error === 'install_interrupted';
      if (stopped) {
        const isCancelled = code === 'install-cancelled' || status.install_error === 'install_cancelled';
        const verdict = isCancelled ? text?.cancelled_verdict : text?.interrupted_verdict;
        return {
          pill: verdict || '',
          title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
          sub: verdict || '',
          message: '',
          notice: text?.stopped_reason || '',
          activate: false,
          bootstrap: true,
          bootstrapLabel: text?.retry || '',
          cancel: true,
          cancelLabel: text?.cancel || window.JournalI18n.render('journal.window.thinking.button.label.cancel'),
          tone: 'bad',
        };
      }
      let classKey = 'unidentified_failed';
      if (code === 'sha256-mismatch' || code === 'download-digest-mismatch' || code === 'download-size-mismatch') {
        classKey = 'integrity_failed';
      } else if (code.startsWith('download-') || code === 'fetch-mint-failed' || code === 'model-download-failed') {
        classKey = 'download_failed';
      } else if (code === 'package-unavailable') {
        classKey = 'package_failed';
      } else if (code === 'package-invalid') {
        classKey = 'package_invalid';
      } else if (code === 'unsupported-platform' || code === 'unsupported-model') {
        classKey = 'unsupported_failed';
      } else if (code === 'disk-space-insufficient') {
        classKey = 'disk_failed';
      } else if (code === 'memory-insufficient') {
        classKey = 'memory_failed';
      } else if (code === 'gpu-memory-insufficient') {
        classKey = 'gpu_memory_insufficient';
      } else if (code === 'gpu-memory-unknown' || code === 'gpu-probe-failed') {
        classKey = 'gpu_memory_unknown';
      } else if (code === 'gpu-unavailable') {
        classKey = 'gpu_unavailable';
      }
      return {
        pill: text?.pill_failed || '',
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: text?.failed_verdict || '',
        message: '',
        notice: text?.[classKey] || text?.failed_reason || '',
        activate: false,
        bootstrap: true,
        bootstrapLabel: text?.retry || '',
        cancel: true,
        cancelLabel: text?.cancel || window.JournalI18n.render('journal.window.thinking.button.label.cancel'),
        tone: 'bad',
      };
    }
    return null;
  }

  function localRuntimeCopy(runtime, active, text) {
    if (!runtime) return null;
    const states = text?.states || {};
    const view = (key, options = {}) => {
      const value = states[key] || {};
      return {
        pill: value.pill || '',
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: value.verdict || '',
        message: '',
        notice: value.reason || '',
        activate: false,
        bootstrap: false,
        retryRuntime: !!options.retryRuntime,
        retryRuntimeLabel: text?.retry || '',
        tone: options.tone || '',
      };
    };

    if (runtime.status === 'corrupt' || runtime.phase === 'state-corrupt') {
      return view('corrupt', {tone: 'bad'});
    }
    if (active && runtime.phase === 'ready-proof-unavailable') {
      return view('ready_proof_unavailable');
    }
    if (runtime.status === 'unavailable' || runtime.phase === 'state-unavailable') {
      return view('unavailable', {tone: 'bad'});
    }
    if (runtime.status === 'stale' || runtime.phase === 'state-stale') {
      return view('stale', {tone: 'bad'});
    }
    if (!active) {
      return runtime.phase === 'cleanup-failed'
        ? view('cleanup_failed', {tone: 'bad'})
        : null;
    }

    if (runtime.reason_code === 'gpu-allocation-failed') {
      return view('gpu_allocation_failed', {
        retryRuntime: runtime.can_retry === true || runtime.phase === 'failed',
        tone: 'bad',
      });
    }

    if (runtime.phase === 'ready') return view('ready');
    if (runtime.phase === 'starting' || runtime.phase === 'warming') {
      return view('starting');
    }
    if (runtime.phase === 'backoff') return view('recovering');
    if (runtime.phase === 'retry-requested') return view('retrying');
    if (runtime.phase === 'host-blocked') {
      if (runtime.reason_code === 'package-unavailable' || runtime.reason_code === 'package-invalid') {
        const key = runtime.reason_code === 'package-invalid' ? 'package_invalid' : 'package_failed';
        return {...view('failed', {tone: 'bad'}), notice: copy.local_install?.[key] || states.failed?.reason || ''};
      }
      if (runtime.reason_code === 'platform-unsupported' || runtime.reason_code === 'gpu-unavailable') {
        return view('unsupported', {tone: 'bad'});
      }
      if (runtime.reason_code === 'gpu-memory-insufficient') {
        return view('gpu_memory_insufficient', {tone: 'bad'});
      }
      if (runtime.reason_code === 'gpu-probe-failed') {
        return view('gpu_probe_failed', {tone: 'bad'});
      }
      return view('waiting');
    }
    if (runtime.phase === 'failed') {
      return view('failed', {
        retryRuntime: runtime.can_retry === true,
        tone: 'bad',
      });
    }
    if (runtime.phase === 'stop-deferred' || runtime.phase === 'stopping') {
      return view('changing');
    }
    if (runtime.phase === 'cleanup-failed') {
      return view('cleanup_failed', {tone: 'bad'});
    }
    if (runtime.phase === 'artifact-not-ready') return null;
    // Disposition: a stopped runtime carries no diagnosis of its own; defer to the
    // readiness chain, which knows whether local is blocked and why.
    if (runtime.phase === 'stopped') return null;
    return view('checking');
  }

  async function pollLocalInstallUntilTerminal({
    fetchStatus,
    sleepFn,
    applyStatus,
    isCurrent,
    intervalMs,
    initialStatus = null,
  }) {
    if (installTerminalStates.has(initialStatus?.install_state || '')) return initialStatus;
    if (installInFlightStates.has(initialStatus?.install_state || '')) {
      if (!isCurrent()) return null;
      await sleepFn(intervalMs);
    }
    while (isCurrent()) {
      const status = await fetchStatus();
      applyStatus(status);
      if (installTerminalStates.has(status?.install_state || '')) return status;
      await sleepFn(intervalMs);
    }
    return null;
  }

  async function pollLocalRuntimeUntilStable({
    fetchStatus,
    sleepFn,
    applyStatus,
    isCurrent,
    intervalMs,
    initialStatus = null,
  }) {
    let status = initialStatus;
    while (isCurrent() && status?.poll === true) {
      await sleepFn(intervalMs);
      if (!isCurrent()) return null;
      status = await fetchStatus();
      applyStatus(status);
    }
    return status;
  }

  function handleInstallPollError({
    generation,
    currentGeneration,
    clearInstallStatus: _clearInstallStatus,
    stopPoll,
    showError,
    error,
  }) {
    if (generation !== currentGeneration()) return false;
    stopPoll();
    showError(error?.message || '');
    return true;
  }

  function confidentialOperationIsTerminal(operation) {
    return !operation || confidentialTerminalPhases.has(operation.phase || '');
  }

  function confidentialEnableNeedsRecheck(activeLane) {
    return activeLane?.confidential_enabled === true
      && activeLane?.confidential_provenance_configured === true
      // The successful operation can expire while the consent browser is open.
      && (!activeLane.confidential_operation || activeLane.confidential_operation.phase === 'not_verified');
  }

  function confidentialOperationRender(operation, text) {
    const phase = operation?.phase || '';
    const states = text?.operation_states || {};
    if (phase === 'starting' || phase === 'waiting') {
      return {message: states[phase] || '', tone: ''};
    }
    if (phase === 'subscribing') return {message: states.subscribing || '', tone: ''};
    if (phase === 'journal_limit') return {message: states.journal_limit || '', tone: ''};
    if (phase === 'early_access') {
      return {message: states.early_access || '', tone: ''};
    }
    if (phase === 'not_verified') {
      return {message: '', tone: ''};
    }
    if (phase === 'repair_needed') {
      return {message: operation?.guidance || states.repair_needed || '', tone: 'error'};
    }
    return {message: operation?.guidance || '', tone: ''};
  }

  function confidentialSetupMetaLine(attestation, lastCheckedLabel = '') {
    const stateName = attestation?.state || '';
    const blocked = stateName === 'failed' || stateName === 'stale' || stateName === 'unreachable';
    if (!blocked || !lastCheckedLabel) return '';
    return window.JournalI18n.render('journal.window.thinking.runs.visible.text.last.confirmed.value1', [['value1', String(lastCheckedLabel)]]);
  }

  function confidentialNoticeLine(operation, text) {
    const message =
      (operation?.phase || '') === 'early_access' ? text?.operation_states?.early_access || '' : '';
    return {text: message, hidden: !message};
  }

  function confidentialSetupOperationLines(operation, text, attestationMessage = '') {
    const rendered = confidentialOperationRender(operation, text);
    const notice = confidentialNoticeLine(operation, text);
    if (!notice.hidden) {
      return {state: attestationMessage, operation: '', operationTone: '', notice};
    }
    // The operation line beside the button carries this; the status card doesn't repeat it.
    return {
      state: attestationMessage,
      operation: rendered.message || '',
      operationTone: rendered.tone,
      notice,
    };
  }

  // A journal that can't run the hardware check never offers confidential
  // processing: the page says so where turning it on would be, and a lane
  // turned on before can still be turned off.
  function confidentialOffered(attestation) {
    return (attestation?.state || 'off') !== 'not_on_platform';
  }

  function confidentialAudioSetting(activeLane) {
    return activeLane?.confidential_audio !== false;
  }

  function confidentialEgressLine(activeLane, beats) {
    return confidentialAudioSetting(activeLane)
      ? beats?.egress_audio_on || ''
      : beats?.egress_audio_off || '';
  }

  function confidentialAudioRender(activeLane, attestation, text) {
    const on = confidentialAudioSetting(activeLane);
    const audio = text?.audio || {};
    return {
      hidden: !confidentialOffered(attestation) || (attestation?.state || 'off') === 'off',
      on,
      label: audio.label || '',
      description: on ? audio.on || '' : audio.off || '',
      note: audio.note || '',
    };
  }

  function confidentialAudioDeferralLine(activeLane, attestation, text) {
    const stateName = attestation?.state || 'off';
    if (!confidentialAudioSetting(activeLane)) return '';
    if (
      stateName !== 'verifying' &&
      stateName !== 'failed' &&
      stateName !== 'stale' &&
      stateName !== 'unreachable'
    ) {
      return '';
    }
    return text?.audio?.deferral || '';
  }

  function confidentialAttestationRender(attestation, text, checkedLabel = '') {
    const stateName = attestation?.state || 'off';
    const states = text?.attestation_states || {};
    if (stateName === 'verified') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.copy.active'),
        tone: 'hot',
        message: formatCopy(states.verified || '', {checked: checkedLabel}),
        recheck: false,
      };
    }
    if (stateName === 'inactive') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.copy.available'),
        tone: '',
        message: states.inactive || '',
        recheck: false,
      };
    }
    if (stateName === 'failed' || stateName === 'stale' || stateName === 'unreachable') {
      return {
        pill: stateName === 'unreachable' ? window.JournalI18n.render('journal.window.thinking.confidential.status.unreachable') : window.JournalI18n.render('journal.window.thinking.window.state.text.not.ready'),
        tone: 'bad',
        message: states[stateName] || '',
        recheck: state.providers?.brain?.identity?.lane === 'spp',
      };
    }
    if (stateName === 'verifying') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.copy.checking'),
        tone: '',
        message: states.verifying || '',
        recheck: false,
      };
    }
    if (stateName === 'not_on_platform') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.window.state.text.not.yet'),
        tone: '',
        message: states.not_on_platform || '',
        recheck: false,
      };
    }
    return {
      pill: window.JournalI18n.render('journal.window.thinking.status.off'),
      tone: '',
      message: '',
      recheck: false,
    };
  }

  function clearConfidentialInProgressOperation(activeLane) {
    const operation = activeLane?.confidential_operation;
    if (!operation || confidentialOperationIsTerminal(operation)) return false;
    activeLane.confidential_operation = null;
    return true;
  }

  function confidentialGlanceForAttestation(attestation, text, checkedLabel = '') {
    const glance = text?.glance || {};
    const stateName = attestation?.state || 'off';
    if (stateName === 'off') {
      return {label: '', value: '', detail: ''};
    }
    if (stateName === 'verified') {
      const row = glance.confidential_verified || {};
      return {
        label: row.label || glance.lane_label || '',
        value: row.value || '',
        detail: formatCopy(row.detail || '', {checked: checkedLabel}),
      };
    }
    if (stateName === 'inactive') {
      const row = glance.confidential_available || {};
      return {
        label: row.label || '',
        value: row.value || '',
        detail: row.detail || '',
      };
    }
    if (stateName === 'verifying') {
      const row = glance.confidential_checking || {};
      return {
        label: row.label || '',
        value: row.value || '',
        detail: row.detail || '',
      };
    }
    const row = glance.confidential_blocked || {};
    const message = confidentialAttestationRender(attestation, text?.confidential, checkedLabel).message;
    return {
      label: row.label || '',
      value: row.value || '',
      detail: formatCopy(row.detail || '', {message}),
    };
  }

  async function pollConfidentialUntilTerminal({
    fetchStatus,
    sleepFn,
    applyStatus,
    isCurrent,
    intervalMs,
    maxElapsedMs,
    initialStatus = null,
    nowFn = Date.now,
  }) {
    const started = nowFn();
    if (initialStatus) {
      applyStatus(initialStatus);
      if (confidentialOperationIsTerminal(initialStatus.active_lane?.confidential_operation)) return initialStatus;
    }
    while (isCurrent() && nowFn() - started < maxElapsedMs) {
      const status = await fetchStatus();
      applyStatus(status);
      if (confidentialOperationIsTerminal(status?.active_lane?.confidential_operation)) return status;
      await sleepFn(intervalMs);
    }
    if (!isCurrent()) return null;
    // Out of time: leave whatever the journal last said on screen rather than
    // clearing it, so the owner still sees how the turn-on ended.
    const status = await fetchStatus();
    applyStatus(status);
    return status;
  }

  function handleConfidentialPollError({
    generation,
    currentGeneration,
    clearOperation,
    stopPoll,
    showError,
    error,
  }) {
    if (generation !== currentGeneration()) return false;
    clearOperation();
    stopPoll();
    showError(error?.message || '');
    return true;
  }

  function formatCopy(template, values = {}) {
    return String(template || '').replace(/\{(\w+)\}/g, (_, key) => values[key] ?? '');
  }

  function byoReasonCopy(reasonCode, context, text, provider, model = '') {
    if (provider === 'chatgpt' || provider === (copy.provider_labels?.chatgpt || '') || context === 'chatgpt') {
      const chatgptText = text?.chatgpt || copy.byo_setup?.chatgpt || {};
      if (reasonCode === 'chatgpt_sign_in_required') return chatgptText.signed_out || '';
      if (reasonCode === 'chatgpt_not_eligible') return chatgptText.not_eligible || '';
      if (reasonCode === 'model_not_found') return chatgptText.model_not_found || '';
      if (reasonCode === 'model_missing') return chatgptText.model_missing || '';
      return chatgptText.save_refused || '';
    }
    if (context === 'probe' && reasonCode === 'model_not_found') {
      return formatCopy(text?.custom_not_found || '', {provider, model});
    }
    const reasonKey = {
      provider_key_invalid: 'reason_rejected',
      provider_quota_exceeded: 'reason_quota',
      network_unreachable: 'reason_network',
      brain_refresh_timeout: 'reason_network',
    }[reasonCode || ''] || 'reason_unknown';
    return formatCopy(text?.[reasonKey] || text?.reason_unknown || '', {provider, model});
  }

  function byoModelStepAllowed(provider, validation) {
    return validation?.valid === true;
  }

  function byoEntryMode(provider, validation) {
    if (provider === 'local') return 'endpoint';
    if (provider === 'chatgpt') return 'chatgpt';
    if (byoModelStepAllowed(provider, validation)) return 'model';
    return 'paste';
  }

  function byoKeyInputEmpty(value) {
    return String(value || '').trim() === '';
  }

  // The owner brings the model id with their key; there is no built-in catalog
  // to fall back to, because provider catalogs change faster than releases.
  function preselectByoModel(provider, providersPayload) {
    const remembered = String(providersPayload?.byo_models?.[provider] || '').trim();
    if (remembered) return remembered;
    const active = providersPayload?.active || {};
    const activeModel = String(active.model || '').trim();
    return active.provider === provider ? activeModel : '';
  }

  async function runByoKeyCheckFlow({
    apiFn,
    applyKeys,
    provider,
    providerName,
    envVar,
    value,
    text,
    providersPayload,
    setMode,
    selectModel,
    resetDraft,
    renderFn,
    showStatus,
  }) {
    if (byoKeyInputEmpty(value)) {
      showStatus(text?.key_hint || '', '');
      return {status: 'empty'};
    }
    showStatus(formatCopy(text?.checking_key || '', {provider: providerName}), '');
    const check = await apiFn('api/keys/check', {
      method: 'POST',
      body: JSON.stringify({env_var: envVar, value}),
    });
    if (check?.valid !== true) {
      resetDraft();
      setMode('paste');
      renderFn();
      const reason = byoReasonCopy(check?.reason_code, 'key', text, providerName);
      showStatus(formatCopy(text?.key_failed || '', {provider: providerName, reason}), window.JournalI18n.render('journal.window.thinking.status.text.error'));
      return {status: 'invalid', validation: check};
    }
    const result = await apiFn('api/keys', {
      method: 'PUT',
      body: JSON.stringify({env_var: envVar, value}),
    });
    applyKeys(result);
    const validation = result?.key_validation?.[provider] || result?.validation || {};
    if (validation.valid !== true) {
      resetDraft();
      setMode('paste');
      renderFn();
      const reason = byoReasonCopy(validation.reason_code, 'key', text, providerName);
      showStatus(formatCopy(text?.key_failed || '', {provider: providerName, reason}), window.JournalI18n.render('journal.window.thinking.status.text.error'));
      return {status: 'invalid', validation};
    }
    resetDraft();
    if (byoModelStepAllowed(provider, validation)) {
      selectModel(preselectByoModel(provider, providersPayload));
      setMode('model');
      renderFn();
      return {status: 'model', validation};
    }
    setMode('paste');
    renderFn();
    return {status: 'checked', validation};
  }

  async function runByoModelSaveFlow({
    apiFn,
    applyProviders,
    provider,
    providerName,
    model,
    googleModelResolutionTargets = [],
    text,
    setMode,
    renderFn,
    showStatus,
  }) {
    showStatus(formatCopy(text?.model_saving || '', {model}), '');
    const probe = await apiFn('api/validate-model', {
      method: 'POST',
      body: JSON.stringify({provider, model}),
    });
    if (probe?.valid !== true) {
      if (probe?.reason_code === 'key_missing') {
        setMode('paste');
        renderFn();
        const reason = byoReasonCopy(probe.reason_code, 'key', text, providerName, model);
        showStatus(formatCopy(text?.key_failed || '', {provider: providerName, reason}), window.JournalI18n.render('journal.window.thinking.status.text.error'));
        return {status: 'key_missing', probe};
      }
      const reason = byoReasonCopy(probe?.reason_code, 'probe', text, providerName, model);
      const message = probe?.reason_code === 'model_not_found'
        ? reason
        : formatCopy(text?.probe_failed_save || '', {provider: providerName, model, reason});
      showStatus(message, window.JournalI18n.render('journal.window.thinking.status.text.error'));
      return {status: 'probe_failed', probe};
    }
    try {
      const body = {lane: 'byo', provider, model};
      if (Array.isArray(googleModelResolutionTargets) && googleModelResolutionTargets.length > 0) {
        body[googleModelResolutionTargetsField] = googleModelResolutionTargets;
      }
      const providers = await apiFn('api/providers', {
        method: 'PUT',
        body: JSON.stringify(body),
      });
      applyProviders(providers);
      renderFn();
      if (providers?.active_lane?.lane === 'confidential') {
        showStatus(
          formatCopy(text?.model_saved_restore || '', {label: model}),
          window.JournalI18n.render('journal.window.thinking.status.text.ok'),
        );
        return {status: 'restored', providers};
      }
      return {status: 'saved', providers};
    } catch (err) {
      renderFn();
      showStatus(err?.message || '', window.JournalI18n.render('journal.window.thinking.status.text.error'));
      return {status: 'save_failed', error: err};
    }
  }

  function laneCopy(id) {
    return (copy.lanes || []).find((lane) => lane.id === id) || {};
  }

  function laneDisplayLabel(lane) {
    if (lane.id === 'byo') return lane.label || window.JournalI18n.render('journal.window.thinking.lane.label.byo.short');
    return (lane.label || lane.id || '').toLowerCase();
  }

  function activeLaneLabel(kind) {
    return copy.active_lane_labels?.[kind] || kind || '';
  }

  function applyCopy(payload) {
    copy = payload || {};
    providerLabels = copy.provider_labels || fallbackProviderLabels;
    setText('thinkingHeading', copy.heading || window.JournalI18n.render('journal.window.thinking.copy.thinking'));
  }

  function renderInitialLoading() {
    const loading = $('thinking-loading');
    if (!loading) return;
    loading.innerHTML = window.SurfaceState.loading({ text: window.JournalI18n.render('journal.window.thinking.text.loading.thinking.settings') });
    loading.style.display = '';
    const app = $('thinkingApp');
    if (app) app.hidden = true;
  }

  function revealThinkingApp() {
    const loading = $('thinking-loading');
    if (loading) loading.style.display = 'none';
    const app = $('thinkingApp');
    if (app) app.hidden = false;
  }

  function renderInitialError(err) {
    window.logError?.(err, { context: 'thinking-state' });
    const loading = $('thinking-loading');
    if (!loading) return;
    loading.innerHTML = window.SurfaceState.error({
      heading: window.JournalI18n.render('journal.window.thinking.heading.couldn.t.load.thinking.settings'),
      desc: window.CONVEY_COPY.RELOAD_HINT,
      serverMessage: window.SurfaceState.serverMessageFrom(err),
      detail: err,
      retry: true,
    });
    loading.querySelector('.surface-state-retry')?.addEventListener('click', () => {
      init();
    });
  }

  let accessUnknownRereadMs = 1000;
  let accessUnknownRereadScheduled = false;

  function scheduleAccessUnknownReread() {
    if (accessUnknownRereadScheduled) return;
    if (state.access?.state !== 'unknown') return;
    accessUnknownRereadScheduled = true;
    ctx.setTimeout(async () => {
      try {
        const payload = await ctx.read('/app/thinking/api/state');
        state.providers = payload.providers || {};
        state.keys = payload.keys || {};
        state.access = payload.access || null;
        state.savedLane = payload.saved_lane ?? null;
        state.localCapability = payload.local_capability || null;
        state.hostCapabilities = payload.host_capabilities || null;
        applyCopy(payload.copy || {});
        renderAll();
      } catch (_err) {}
    }, accessUnknownRereadMs);
  }

  async function loadInitialState() {
    renderInitialLoading();
    try {
      const payload = await ctx.read('/app/thinking/api/state');
      state.providers = payload.providers || {};
      state.keys = payload.keys || {};
      state.access = payload.access || null;
      state.savedLane = payload.saved_lane ?? null;
      state.localCapability = payload.local_capability || null;
        state.hostCapabilities = payload.host_capabilities || null;
      applyCopy(payload.copy || {});
      scheduleAccessUnknownReread();
      revealThinkingApp();
      return true;
    } catch (err) {
      renderInitialError(err);
      reportThinkingReady('error');
      return false;
    }
  }

  async function api(path, options = {}) {
    // Every address is absolute, so a read still answers for this view's API
    // whatever the page's address is by then.
    const url = String(path).startsWith('/') ? path : `${THINKING_PATH}${path}`;
    // A write already out when the view goes still finishes. One its code
    // would send next, after that one answers (a key's save after its check),
    // is never sent, as it never was once the page had gone.
    const method = String(options.method || 'GET').toUpperCase();
    if (ctx.disposed && method !== 'GET') return window.SolStore.never();
    let payload;
    try {
      payload = await ctx.read(url, {
        ...options,
        noAuthRedirect: true,
        headers: {
          ...(options.body ? {'Content-Type': 'application/json'} : {}),
          ...(options.headers || {}),
        },
      });
    } catch (err) {
      const refused = requestError(err, err?.payload);
      // The change landed even though the answer is an error. Re-read and render
      // what is now set before the caller writes the sentence, so a re-render
      // doesn't wipe it.
      if (refused.reasonCode === 'settings_saved_unlogged') {
        await Promise.all([refreshProviders(), refreshKeys()].map((read) => read.catch(() => {})));
      }
      throw refused;
    }
    // A 200 can still say the action didn't happen (the brain check's
    // `check_not_started`); that `error` is a code, not words for the owner.
    if (payload?.error) {
      throw requestError({}, payload);
    }
    return payload;
  }

  // A refusal's `error` is the sentence written for the owner. Its `detail` is
  // diagnostics: apiJson has already put it, with the status and request id, in
  // the diagnostic console, so it stays on the error and never reaches the page.
  // Anything that isn't a refusal (a failed fetch, an answer that isn't JSON, a
  // bare code in a 200) reads as the shell's generic line.
  function requestError(err, payload) {
    if (err?.cause === 'setup_required') return err;
    const says = err?.status >= 400 && typeof payload?.error === 'string'
      ? payload.error.trim()
      : '';
    const wrapped = new Error(
      says || window.CONVEY_COPY?.CONSOLE_SUMMARY_REQUEST_FAILED || window.JournalI18n.render('journal.window.thinking.window.explanation.couldn.t.finish.that.request'),
    );
    wrapped.status = err?.status;
    wrapped.reasonCode = err?.reasonCode ?? payload?.reason_code ?? payload?.error ?? null;
    wrapped.detail = payload?.detail ?? null;
    // No status means no server answered: keep the original for the console.
    if (typeof err?.message === 'string' && err.status === undefined) wrapped.cause = err;
    return wrapped;
  }

  function sleep(ms) {
    return new Promise((resolve) => ctx.setTimeout(resolve, ms));
  }

  function showView(name, options = {}) {
    return drawn(() => showViewNow(name, options));
  }

  function showViewNow(name, options = {}) {
    let target = views.has(name) ? name : 'main';
    if (target === 'lane-switch' && !state.pendingSwitchTarget) {
      target = 'main';
    }
    const nextHash = `#${target}`;
    // A hashless load already is the setup route: leave the address alone rather
    // than writing a route token the owner never asked for.
    const keepHashless = options.keepHashless && target === 'main' && !window.location.hash;
    if (!keepHashless && window.location.hash !== nextHash) {
      writeEntry(options.replace ? 'replace' : 'push', nextHash);
    }
    if (target !== 'local-setup') {
      clearLocalSetupError();
      stopInstallPoll();
      stopRuntimePoll();
    } else if (!bundledLocalUnavailable() && state.localModels.length > 0) {
      refreshInstallStatus({autoResume: true}).catch((err) => {
        showLocalSetupFailure(err);
      });
      refreshLocalRuntime({autoResume: true}).catch((err) => {
        showLocalSetupFailure(err);
      });
    }
    if (target !== 'byo-setup') {
      stopChatGptPoll();
    } else if (
      state.chatgpt.screen === 'pending' &&
      state.chatgpt.attempt?.id &&
      document.visibilityState !== 'hidden'
    ) {
      if (state.chatgpt.pollInFlight == null && !state.chatgpt.pollTimer) {
        pollChatGptAttempt(state.chatgpt.attempt.id, state.chatgpt.pollToken).catch(() => {});
      }
    }
    let previous = '';
    markup().querySelectorAll('#providers [data-view]').forEach((section) => {
      if (!previous && !section.hidden) previous = section.dataset.view;
      writeHidden(section, section.dataset.view !== target);
    });
    if (target !== 'main') {
      stopLocalConfirmation();
      stopBrainFollow();
    } else if (previous && previous !== 'main') {
      // A setup page can change what the lane cards should say, and the main view
      // renders from the providers it last fetched.
      refreshProviders()
        .then(() => followLocalConfirmation())
        .catch((err) => setMessage('thinkingActiveDetail', err.message, 'error'));
    }
  }

  function viewFromHash() {
    const hash = window.location.hash.replace(/^#/, '');
    return views.has(hash) ? hash : 'main';
  }

  function encodeThinkingSegment(value) {
    return encodeURIComponent(value);
  }

  function decodeThinkingSegment(value) {
    try {
      return decodeURIComponent(value);
    } catch (_) {
      return null;
    }
  }

  function todayThinkingDay() {
    if (window.JournalClock) return window.JournalClock.today();
    const now = new Date();
    return `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
  }

  function thinkingRunsRoute({kind = 'runs', day = '', talent = '', useId = '', facet = '', facetExplicit = false}) {
    const resolvedFacet = facetExplicit && facet ? facet : '';
    const resolvedFacetExplicit = Boolean(resolvedFacet);
    const key = kind === 'run-id'
      ? `run:${useId}`
      : `runs:${day}:${talent}:${useId}`;
    return {kind, day, talent, useId, facet: resolvedFacet, facetExplicit: resolvedFacetExplicit, key};
  }

  function currentThinkingRunsRoute(route) {
    return thinkingRunsRoute({
      ...route,
      facet: state.runsFacet,
      facetExplicit: state.runsFacetExplicit,
    });
  }

  function thinkingRunsHash(route) {
    const suffix = route.facetExplicit && route.facet
      ? `?facet=${encodeURIComponent(route.facet)}`
      : '';
    if (route.kind === 'run-id') {
      return `#runs/run/${encodeThinkingSegment(route.useId)}${suffix}`;
    }
    const parts = ['#runs', route.day];
    if (route.talent) parts.push(encodeThinkingSegment(route.talent));
    if (route.useId) parts.push(encodeThinkingSegment(route.useId));
    return `${parts.join('/')}${suffix}`;
  }

  function parseThinkingHash() {
    const rawHash = window.location.hash.replace(/^#/, '');
    const [hash, query = ''] = rawHash.split('?', 2);
    if (!hash.startsWith('runs')) return null;
    const params = new URLSearchParams(query);
    const facet = params.get('facet') || '';
    const facetExplicit = Boolean(facet);
    const parts = hash.split('/');
    if (parts[0] !== 'runs') return null;
    if (parts.length === 1) return {kind: 'runs-root', key: 'runs-root'};
    if (parts[1] === 'run') {
      if (parts.length !== 3 || !parts[2]) return {kind: 'runs-invalid', key: 'runs-invalid'};
      const useId = decodeThinkingSegment(parts[2]);
      return useId
        ? thinkingRunsRoute({kind: 'run-id', useId, facet, facetExplicit})
        : {kind: 'runs-invalid', key: 'runs-invalid'};
    }
    if (parts.length < 2 || parts.length > 4 || !/^\d{8}$/.test(parts[1])) {
      return {kind: 'runs-invalid', key: 'runs-invalid'};
    }
    const decoded = parts.slice(2).map(decodeThinkingSegment);
    if (decoded.some((part) => !part)) return {kind: 'runs-invalid', key: 'runs-invalid'};
    const [talent = '', useId = ''] = decoded;
    if (useId && !talent) return {kind: 'runs-invalid', key: 'runs-invalid'};
    return thinkingRunsRoute({
      kind: 'runs',
      day: parts[1],
      talent,
      useId,
      facet,
      facetExplicit,
    });
  }

  function replaceThinkingHash(hash) {
    if (window.location.hash !== hash) {
      writeEntry('replace', hash);
    }
  }

  function thinkingSelectedRunId(route) {
    return (route?.kind === 'runs' || route?.kind === 'run-id') ? route.useId || '' : '';
  }

  function setThinkingRoute(route) {
    if (route?.kind === 'runs' || route?.kind === 'run-id') {
      setRunsFacet(route.facet, route.facetExplicit);
      state.runsLastHash = thinkingRunsHash(route);
    }
    const selectedUseId = thinkingSelectedRunId(route);
    if (state.runsSelectedUseId !== selectedUseId) {
      state.runsSelectedUseId = selectedUseId;
      closeThinkingPrompt();
      clearThinkingRunRenderState();
    }
    if (state.runsRouteKey !== route.key) {
      state.runsRouteKey = route.key;
      state.runsNavigationGeneration += 1;
    }
  }

  function thinkingCacheKey(kind, values = {}) {
    if (kind === 'day') return `day:${values.day}:facet:${values.facet || ''}`;
    if (kind === 'run') return `run:${values.useId}`;
    if (kind === 'prompt') return `prompt:${values.talent}`;
    if (kind === 'output') return `output:${values.day}:${values.file}`;
    return kind;
  }

  function beginThinkingRequest(kind) {
    const requestGeneration = (state.runsRequestGenerations[kind] || 0) + 1;
    state.runsRequestGenerations[kind] = requestGeneration;
    const token = {
      kind,
      navigationGeneration: state.runsNavigationGeneration,
      requestGeneration,
      hash: window.location.hash,
      selectionKey: currentRunsSelectionKey(),
    };
    state.runsInFlight[kind] = token;
    return token;
  }

  function isCurrentThinkingRequest(token) {
    return token.navigationGeneration === state.runsNavigationGeneration
      && token.requestGeneration === state.runsRequestGenerations[token.kind]
      && token.hash === window.location.hash
      && token.selectionKey === currentRunsSelectionKey();
  }

  function readThinkingCache(kind, key) {
    return state.runsCache[kind].get(key) || null;
  }

  function evictThinkingCache(kind, key) {
    state.runsCache[kind].delete(key);
  }

  function writeThinkingCache(kind, key, value, token) {
    if (!isCurrentThinkingRequest(token)) return false;
    state.runsCache[kind].set(key, value);
    return true;
  }

  async function loadThinkingRequest(kind, key, load, renderReady, renderFailed, shouldCache = () => true) {
    const token = beginThinkingRequest(kind);
    try {
      const value = await load();
      if (!isCurrentThinkingRequest(token)) return;
      if (!shouldCache(value)) {
        renderReady(value);
        return;
      }
      if (!writeThinkingCache(kind, key, value, token) || !isCurrentThinkingRequest(token)) return;
      renderReady(value);
    } catch (err) {
      if (isCurrentThinkingRequest(token)) renderFailed(err);
    } finally {
      if (isCurrentThinkingRequest(token)) state.runsInFlight[kind] = null;
    }
  }

  function setThinkingStatus(id, message) {
    const status = $(id);
    if (!status) return;
    status.textContent = message || '';
    status.hidden = !message;
  }

  function runContextFromRecord(route, record) {
    const day = record?.day;
    const talent = record?.name;
    const useId = record?.id || route.useId;
    if (!/^\d{8}$/.test(day || '') || !talent || !useId) return route;
    const contextual = thinkingRunsRoute({
      kind: 'runs', day, talent, useId,
      facet: route.facet,
      facetExplicit: route.facetExplicit,
    });
    if (route.day !== day || route.talent !== talent || route.useId !== useId || route.kind === 'run-id') {
      replaceThinkingHash(thinkingRunsHash(contextual));
    }
    return contextual;
  }

  function thinkingPanelHeading(tabId) {
    if (tabId === 'thinkingRunsTab') return $('thinkingRunsHeading');
    if (tabId === 'thinkingPaceTab') return $('thinkingPaceHeading');
    return $('thinkingHeading');
  }

  function activateThinkingTab(tabId, origin, tablistId, headingId = '') {
    const tablist = $(tablistId);
    const tab = $(tabId);
    if (!tab) return;
    const tabs = tablist?.querySelectorAll('[role="tab"]') || [];
    tabs.forEach((candidate) => {
      const selected = candidate === tab;
      candidate.setAttribute('aria-selected', String(selected));
      candidate.tabIndex = selected ? 0 : -1;
    });
    if (origin === 'pointer' || origin === 'keyboard') {
      tab.focus();
      return;
    }
    // Only an owner-initiated section change moves focus. A plain load ('reload')
    // must not draw a focus ring on a heading nobody asked for.
    if (origin === 'history' && !tablist?.contains(document.activeElement)) {
      (headingId ? $(headingId) : thinkingPanelHeading(tabId))?.focus({preventScroll: true});
    }
  }

  function activateThinkingSectionTab(tabId, origin) {
    activateThinkingTab(tabId, origin, 'thinkingSectionTabs');
  }

  function showThinkingSection(section, route, origin) {
    setThinkingRoute(route);
    markup().querySelectorAll('#providers [data-view]').forEach((view) => {
      view.hidden = true;
    });
    markup().querySelectorAll('[data-thinking-section]').forEach((panel) => {
      panel.hidden = panel.dataset.thinkingSection !== section;
    });
    // The intro speaks for setup, so it goes with it.
    writeHidden($('thinkingIntro'), true);
    activateThinkingSectionTab(section === 'pace' ? 'thinkingPaceTab' : 'thinkingRunsTab', origin);
    if (section === 'pace') pace?.show().catch(() => {});
    if (section === 'runs') {
      if (route.kind === 'runs') loadThinkingRuns(route);
      if (route.useId) loadThinkingRun(route);
    }
  }

  function navigateThinkingSection(section, origin) {
    const hash = section === 'setup'
      ? '#main'
      : section === 'pace' ? PACE_HASH : (state.runsLastHash || '#runs');
    if (window.location.hash !== hash) writeEntry('push', hash);
    routeThinkingHash(origin);
  }

  function showThinkingSetup(origin) {
    setThinkingRoute({key: 'setup'});
    markup().querySelectorAll('[data-thinking-section]').forEach((panel) => {
      panel.hidden = true;
    });
    writeHidden($('thinkingIntro'), false);
    showView(viewFromHash(), {replace: true, keepHashless: true});
    activateThinkingSectionTab('thinkingSetupTab', origin);
  }

  function routeThinkingHash(origin = 'history') {
    const hash = window.location.hash;
    if (!hash) {
      showThinkingSetup(origin);
      return;
    }
    if (hash === PACE_HASH) {
      showThinkingSection('pace', {key: 'pace'}, origin);
      return;
    }
    const route = parseThinkingHash();
    if (route?.kind === 'runs-root') {
      const canonical = thinkingRunsRoute({kind: 'runs', day: todayThinkingDay()});
      replaceThinkingHash(thinkingRunsHash(canonical));
      showThinkingSection('runs', canonical, origin);
      return;
    }
    if (route?.kind === 'runs-invalid') {
      const canonical = thinkingRunsRoute({kind: 'runs', day: todayThinkingDay()});
      replaceThinkingHash(thinkingRunsHash(canonical));
      showThinkingSection('runs', canonical, origin);
      setThinkingStatus('thinkingRunsStatus', window.JournalI18n.render('journal.window.thinking.window.explanation.that.run.isn.t.available'));
      return;
    }
    if (route?.kind === 'runs' || route?.kind === 'run-id') {
      replaceThinkingHash(thinkingRunsHash(route));
      showThinkingSection('runs', route, origin);
      setThinkingStatus('thinkingRunsStatus', '');
      return;
    }
    showThinkingSetup(origin);
  }

  function bindThinkingTablist(tablistId, activate) {
    const thinkingTabs = Array.from($(tablistId)?.querySelectorAll('[role="tab"]') || []);
    thinkingTabs.forEach((tab) => {
      tab.addEventListener('click', () => {
        if (!tab.hidden) activate(tab, 'pointer');
      });
      tab.addEventListener('keydown', (event) => {
        const visibleTabs = thinkingTabs.filter((candidate) => !candidate.hidden);
        const currentIndex = visibleTabs.indexOf(tab);
        if (currentIndex === -1) return;
        let nextIndex = currentIndex;
        if (event.key === 'ArrowLeft') nextIndex = (currentIndex + visibleTabs.length - 1) % visibleTabs.length;
        else if (event.key === 'ArrowRight') nextIndex = (currentIndex + 1) % visibleTabs.length;
        else if (event.key === 'Home') nextIndex = 0;
        else if (event.key === 'End') nextIndex = visibleTabs.length - 1;
        else return;
        event.preventDefault();
        activate(visibleTabs[nextIndex], 'keyboard');
      });
    });
  }

  function bindThinkingSectionTabs() {
    const thinkingSectionForTab = (tab) => ({
      thinkingSetupTab: 'setup',
      thinkingRunsTab: 'runs',
      thinkingPaceTab: 'pace',
    })[tab.id];
    bindThinkingTablist('thinkingSectionTabs', (tab, origin) => {
      navigateThinkingSection(thinkingSectionForTab(tab), origin);
    });
  }

  function runsDayInputValue(day) {
    return `${day.slice(0, 4)}-${day.slice(4, 6)}-${day.slice(6, 8)}`;
  }

  function runsDayFromInput(value) {
    const day = String(value || '').replace(/-/g, '');
    return /^\d{8}$/.test(day) ? day : '';
  }

  function shiftThinkingDay(day, amount) {
    const value = new Date(Number(day.slice(0, 4)), Number(day.slice(4, 6)) - 1, Number(day.slice(6, 8)) + amount);
    return `${value.getFullYear()}${String(value.getMonth() + 1).padStart(2, '0')}${String(value.getDate()).padStart(2, '0')}`;
  }

  function currentRunsSelectionKey() {
    return `${state.runsRouteKey}:facet:${state.runsFacet}`;
  }

  function setRunsFacet(value, explicit) {
    const next = value || '';
    if (state.runsFacet !== next || state.runsFacetExplicit !== explicit) {
      state.runsFacet = next;
      state.runsFacetExplicit = explicit;
      state.runsNavigationGeneration += 1;
    }
  }

  function runFacetLabel(run) {
    return state.runsFacetTitles.get(run.facet) || run.facet || '';
  }

  function thinkingRunFacts(run) {
    return [
      ['ran', window.JournalFormat.timestamp(run.start)],
      ['model', runModelLabel(run)],
      ['provider', runProviderLabel(run)],
      ['runtime', runRuntimeLabel(run)],
      ['status', run.failed ? 'failed' : (run.status || window.JournalI18n.render('journal.window.thinking.runs.status.unknown')).replaceAll('_', ' ')],
      [window.JournalI18n.render('journal.window.thinking.window.state.text.thinking.events'), run.thinking_count],
      [window.JournalI18n.render('journal.window.thinking.window.state.text.tool.calls'), run.tool_count],
      ['facet', runFacetLabel(run)],
    ].filter(([, value]) => value !== null && value !== undefined && value !== '');
  }

  function appendThinkingRunFacts(host, run) {
    thinkingRunFacts(run).forEach(([label, value]) => {
      const item = document.createElement('span');
      item.textContent = `${label}: ${value}`;
      host.appendChild(item);
    });
  }

  function normalizedThinkingRuns(payload) {
    return Array.isArray(payload?.uses) ? payload.uses.map((run) => ({
      id: run.id,
      name: run.name,
      start: run.start,
      status: run.status,
      failed: run.failed === true,
      model: run.model,
      provider: run.provider,
      runtime_seconds: run.runtime_seconds,
      thinking_count: run.thinking_count,
      tool_count: run.tool_count,
      facet: run.facet,
      output_file: run.output_file,
    })).filter((run) => run.id && run.name) : [];
  }

  function renderThinkingRunsSummary(runs) {
    const summary = $('thinkingRunsSummary');
    if (!summary) return;
    summary.replaceChildren();
    const total = document.createElement('span');
    total.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.value1.run.value2', [['value1', String(runs.length)], ['value2', String(runs.length !== 1 ? 's' : '')]]);
    summary.appendChild(total);
    const failed = runs.filter((run) => run.failed).length;
    if (failed) {
      const item = document.createElement('span');
      item.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.value1.failed', [['value1', String(failed)]]);
      summary.appendChild(item);
    }
  }

  function thinkingRunControl(run) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'thinking-runs-run-control';
    // G2-38: thinking_count and tool_count are the only two things a run log
    // can ever hold. When both are zero, the log will be empty every time —
    // dim the control so that's visible before the click, not after.
    // B14: the reads layer emits null for either count whenever the source
    // entry lacks it, and null > 0 is false. An absent count is unknown, not
    // zero, so it gets neither the dimming nor the claim that nothing was
    // recorded.
    const counted = run.thinking_count != null && run.tool_count != null;
    if (counted && !(run.thinking_count > 0) && !(run.tool_count > 0)) {
      button.classList.add('thinking-runs-run-control-empty');
      button.title = window.JournalI18n.render('journal.window.thinking.tooltip.this.run.left.no.log');
    }
    button.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.run.log');
    button.addEventListener('click', () => navigateThinkingRun(run));
    return button;
  }

  function runStatusLabel(run) {
    return run.failed ? window.JournalI18n.render('journal.window.thinking.runs.status.failed') : (run.status || window.JournalI18n.render('journal.window.thinking.runs.status.unknown')).replaceAll('_', ' ');
  }

  // "duration unavailable" is what the app says about a fact it failed to get.
  // A run that started two minutes ago and has not finished has no runtime to
  // get yet, and that row read as a fault beside a healthy status (G2-B12).
  // Only the in-flight case moves; a finished run that recorded no runtime
  // keeps the sentinel.
  function runRuntimeLabel(run) {
    const missing = run.runtime_seconds === null || run.runtime_seconds === undefined;
    if (missing && !run.failed && run.status === 'running') return window.JournalI18n.render('journal.window.thinking.window.state.text.still.running');
    return window.JournalFormat.duration(run.runtime_seconds);
  }

  // Column order for the runs table. `hideable` columns can be lifted out
  // of the table (into the group summary sentence, or dropped outright when
  // every row in the group is blank) when a whole group agrees on them —
  // a ten-column table where three columns repeat one value on every row
  // and two are always empty answers "is my journal thinking?" with mostly
  // noise (G2-46). `ran` and the run-log control are never hidden.
  const thinkingRunColumns = [
    {key: 'status', label: window.JournalI18n.render('journal.window.thinking.runs.column.status'), hideable: true, value: runStatusLabel},
    {key: 'model', label: window.JournalI18n.render('journal.window.thinking.runs.column.model'), hideable: true, value: runModelLabel},
    {key: 'provider', label: window.JournalI18n.render('journal.window.thinking.runs.column.provider'), hideable: true, value: runProviderLabel},
    {key: 'runtime', label: window.JournalI18n.render('journal.window.thinking.runs.column.runtime'), hideable: false, value: runRuntimeLabel},
    {key: 'thinking_count', label: window.JournalI18n.render('journal.window.thinking.label.thinking.events'), hideable: false, value: (run) => run.thinking_count},
    {key: 'tool_count', label: window.JournalI18n.render('journal.window.thinking.label.tool.calls'), hideable: false, value: (run) => run.tool_count},
    {key: 'facet', label: window.JournalI18n.render('journal.window.thinking.runs.column.facet'), hideable: true, value: runFacetLabel},
    {key: 'output', label: window.JournalI18n.render('journal.window.thinking.runs.column.output'), hideable: true, value: (run) => (run.output_file ? window.JournalI18n.render('journal.window.thinking.runs.column.output') : '')},
  ];

  // Two reasons a hideable column earns its way out of the table, and they are
  // not interchangeable (G2-46). A column that is empty on every run in the
  // group carries nothing, so it can always go — a model column blank down all
  // 120 rows is the noise this was written to remove, whether or not the note
  // is shown. A column that repeats one *real* value is only redundant while
  // something else is saying that value, so it goes only when it is in
  // `spokenFor`; anything the note does not say stays in the table. Both are
  // read from the full group rather than the shown page, so the table shape
  // doesn't change as more rows page in.
  function thinkingRunHiddenColumns(group, spokenFor = new Set()) {
    const hidden = new Set();
    thinkingRunColumns.forEach((column) => {
      if (!column.hideable) return;
      if (group.every((run) => !column.value(run))) {
        hidden.add(column.key);
        return;
      }
      if (!spokenFor.has(column.key)) return;
      const values = new Set(group.map((run) => String(column.value(run) ?? '')));
      if (values.size === 1) hidden.add(column.key);
    });
    return hidden;
  }

  // "52 pulse runs completed on Qwen 3.5 4B (local), 20 sec each" — the
  // sentence a reader would otherwise have to derive by scanning 52 identical
  // rows, shown once when the group actually is that uniform (G2-46).
  function thinkingRunGroupNote(label, group) {
    if (!group.length) return null;
    const statuses = new Set(group.map(runStatusLabel));
    const models = new Set(group.map(runModelLabel));
    const providers = new Set(group.map(runProviderLabel));
    // Each clause is a claim about every run in the group, so each earns its
    // place on its own columns' uniformity. They used to earn it together --
    // one differing status suppressed the whole note -- and a group with a run
    // still in flight is exactly a group whose statuses differ, so today's day
    // kept a table repeating one model and one provider down every row with
    // nothing standing in for them (prior G2-46 partial).
    //
    // The provider gate guards the *model* clause, not a provider clause -- do
    // not drop it on the grounds that the sentence never says the provider.
    // runModelLabel resolves LOCAL_MODEL_LABELS off a `${provider}/${model}`
    // key, so "Qwen 3.5 4B (local)" makes a lane claim that rides on the
    // provider; across two providers that resolve to one label, the lane would
    // be false for some of the runs.
    const statusText = statuses.size === 1 ? ` ${[...statuses][0]}` : '';
    const uniformModel = models.size === 1 && providers.size === 1;
    // Every clause here is a claim about every run in the group, not about the
    // group in aggregate — that is what the uniformity guards above buy the
    // status and the model, and the runtime has to earn it the same way. As an
    // average it never did, and failed twice over: an unrecorded runtime is
    // null, and Number(null) is 0 that passes Number.isFinite, so unknown
    // runtimes averaged in as zero and told an owner watching for signs of
    // life that the thinking took "about 0 sec" while the runtime column
    // beside it read "duration unavailable" for those same runs; and a group
    // of 2 sec and 38 sec averaged to "about 20 sec each", true of neither
    // run. So this is not an average at all: it is the runtime column's own
    // value, said once, and only when every run in the group reads the same.
    const eachSuffix = group.length === 1 ? '' : window.JournalI18n.render('journal.window.thinking.window.state.text.each');
    const runtimes = new Set(group.map(runRuntimeLabel));
    const sharedRuntime = runtimes.size === 1 ? [...runtimes][0] : null;
    // No hedge: this is not an average any more, it is the runtime column's
    // own string repeated, so "about" would be apologising for a figure that
    // is exact -- and formatDuration is to the second above a minute, which
    // made "about 5 min 3 sec each" read as generated.
    // "still running" joins "duration unavailable" as a value that is a state,
    // not a duration; neither belongs in a sentence about how long the runs
    // took (G2-B12).
    const measuredRuntime = group.every(run => run.runtime_seconds != null && Number.isFinite(Number(run.runtime_seconds)));
    const durationText = sharedRuntime && measuredRuntime
      ? `, ${sharedRuntime}${eachSuffix}`
      : '';
    // A run can simply not record a model, and runModelLabel hands the absent
    // id straight back — which the sentence then read out as "completed on
    // undefined." Say nothing rather than name a lane the runs did not record;
    // the provider column stays in the table either way, so nothing is lost by
    // the silence.
    const modelLabel = uniformModel ? String([...models][0] ?? '').trim() : '';
    const modelText = modelLabel ? window.JournalI18n.render('journal.window.thinking.runs.visible.text.on.value1', [['value1', String(modelLabel)]]) : '';
    // No "all". It is a quantifier over the population, and the population
    // here is a filtered view -- `runs` is narrowed to failures when the
    // failed-runs-only box is checked, and the day payload is narrowed to one
    // facet when a facet is picked. A talent that ran 52 times and failed 3
    // then said "all 3 pulse runs failed", which reads as a dead lane; worse,
    // that filter forces statuses.size === 1, so the view that breaks the word
    // is the view that most reliably shows it. A bare count is true under
    // every filter, and the table below carries the completeness "all" was
    // reaching for. (The empty state at the top of renderThinkingRunsDay
    // already counts matchingRuns rather than runs for exactly this reason.)
    const runCount = `${group.length} ${label} run${group.length === 1 ? '' : 's'}`;
    // The note reports what it said, so the columns lifted out of the table
    // cannot drift from the facts the sentence actually states. A static list
    // did drift: a whitespace-only model id is truthy in the column (so it is
    // not dropped as empty) but trims to nothing for the sentence, and the
    // column went while nothing stood in for it. Never the provider, either --
    // the note does not name it, and the model label only carries the lane for
    // the single id in LOCAL_MODEL_LABELS, so a BYO model falling through
    // runModelLabel as its bare wire id would take the one column saying
    // whether the thinking stayed on the owner's device with it. `facet` and
    // `output` are never here for the same reason: the note never says them,
    // so they only leave by being empty on every row.
    const spokenFor = new Set();
    if (statusText) spokenFor.add('status');
    if (modelText) spokenFor.add('model');
    // The runtime column is never lifted out of the table, so a runtime clause
    // on its own would only repeat what every row already shows. The note earns
    // its line by standing in for a column; the runtime rides along when it can.
    if (!spokenFor.size) return null;
    return {text: `${runCount}${statusText}${modelText}${durationText}.`, spokenFor};
  }

  function renderThinkingRunList(host, runs, hiddenColumns = new Set()) {
    const visibleColumns = thinkingRunColumns.filter((column) => !hiddenColumns.has(column.key));
    const table = document.createElement('table');
    table.className = 'thinking-runs-table';
    const head = document.createElement('thead');
    const headRow = document.createElement('tr');
    // G2-45: this column holds the run-log button, not the prompt — the
    // word "prompt" already names a different tab inside the run detail.
    for (const label of ['ran', ...visibleColumns.map((column) => column.label), window.JournalI18n.render('journal.window.thinking.window.state.text.run.log')]) {
      const cell = document.createElement('th');
      cell.scope = 'col';
      cell.textContent = label;
      if (label === 'ran' && window.JournalClock?.placeNote) {
        window.JournalClock.placeNote(cell, { inside: true });
      }
      headRow.appendChild(cell);
    }
    head.appendChild(headRow);
    table.appendChild(head);
    const body = document.createElement('tbody');
    runs.forEach((run) => {
      const row = document.createElement('tr');
      if (run.failed) row.className = 'thinking-run-failed';
      const values = [window.JournalFormat.timestamp(run.start), ...visibleColumns.map((column) => column.value(run))];
      for (const value of values) {
        const cell = document.createElement('td');
        if (value !== null && value !== undefined && value !== '') cell.textContent = value;
        row.appendChild(cell);
      }
      const runLog = document.createElement('td');
      runLog.appendChild(thinkingRunControl(run));
      row.appendChild(runLog);
      body.appendChild(row);
    });
    table.appendChild(body);
    host.appendChild(table);
    const cards = document.createElement('div');
    cards.className = 'thinking-runs-cards';
    runs.forEach((run) => {
      const card = document.createElement('article');
      card.className = 'thinking-runs-card';
      const heading = document.createElement('h4');
      heading.textContent = talentLabel(run.name);
      card.appendChild(heading);
      appendThinkingRunFacts(card, run);
      card.appendChild(thinkingRunControl(run));
      cards.appendChild(card);
    });
    host.appendChild(cards);
  }

  function renderThinkingRunsDay(payload, route) {
    rememberTalentTitles(payload);
    const dayRuns = normalizedThinkingRuns(payload);
    const matchingRuns = dayRuns.filter(run => !route.talent || run.name === route.talent);
    const runs = state.runsFailuresOnly ? matchingRuns.filter(run => run.failed) : matchingRuns;
    const facetFiltered = Boolean(state.runsFacetExplicit && state.runsFacet);
    // An unfiltered read of a day is the only thing that knows how many runs
    // the day had; remember it so a later filtered read of the same day can
    // say what the filter did rather than what the day did (G2-B02).
    if (!facetFiltered) state.runsDayTotal = {day: route.day, count: dayRuns.length};
    updateThinkingRunsDayControls(route);
    const returnedFacets = Array.isArray(payload?.facets)
      ? payload.facets
      : Object.entries(payload?.facets && typeof payload.facets === 'object' ? payload.facets : {})
        .map(([name, metadata]) => ({
          ...(metadata && typeof metadata === 'object' ? metadata : {}),
          name,
        }));
    returnedFacets.forEach((item) => {
      const name = item?.name ?? item;
      state.runsFacetTitles.set(name, item?.title || name);
    });
    if (state.runsDetail) {
      const facts = $('thinkingRunsDetailFacts');
      if (facts) {
        facts.replaceChildren();
        appendThinkingRunFacts(facts, state.runsDetail);
      }
    }
    const facet = $('thinkingRunsFacet');
    if (facet) {
      facet.replaceChildren();
      const all = document.createElement('option');
      all.value = '';
      all.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.all');
      facet.appendChild(all);
      returnedFacets.forEach((item) => {
        const option = document.createElement('option');
        option.value = item.name || item;
        option.textContent = item.title || item.name || item;
        facet.appendChild(option);
      });
      facet.value = state.runsFacet;
    }
    const host = $('thinkingRunsContent');
    if (!host) return;
    host.replaceChildren();
    $('thinkingRunsDetail').hidden = !route.useId;
    if (!route.useId) $('thinkingRunsNoOutput').hidden = true;
    renderThinkingRunsSummary(matchingRuns);
    const controls = document.createElement('div');
    controls.className = 'thinking-runs-filters';
    if (route.talent) {
      const context = document.createElement('span');
      context.textContent = talentLabel(route.talent);
      const all = document.createElement('a');
      all.href = thinkingRunsHash(thinkingRunsRoute({...route, talent: '', useId: ''}));
      all.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.all.runs.for.this.day');
      controls.append(context, all);
    }
    const label = document.createElement('label');
    const input = document.createElement('input');
    input.type = 'checkbox'; input.checked = state.runsFailuresOnly;
    input.addEventListener('change', () => { state.runsFailuresOnly = input.checked; renderThinkingRunsDay(payload, route); host.querySelector('input[type=checkbox]')?.focus(); });
    label.append(input, document.createTextNode(window.JournalI18n.render('journal.window.thinking.window.state.text.failed.runs.only')));
    controls.append(label); host.append(controls);
    if (!runs.length) {
      const picked = returnedFacets.find((item) => (item?.name ?? item) === state.runsFacet);
      const facetTitle = (picked && (picked.title || picked.name)) || state.runsFacet;
      const dayTotal = state.runsDayTotal && state.runsDayTotal.day === route.day ? state.runsDayTotal.count : null;
      const heading = document.createElement('p');
      heading.textContent = state.runsFailuresOnly
        ? window.JournalI18n.render('journal.window.thinking.visible.text.no.failed.runs.match.this.view')
        : route.talent
          ? window.JournalI18n.render('journal.window.thinking.visible.text.no.runs.of.this.kind.on')
          : facetFiltered
            ? window.JournalI18n.render('journal.window.thinking.visible.text.no.runs.in.this.facet.on')
            : window.JournalI18n.render('journal.window.thinking.visible.text.no.runs.on.this.day');
      const detail = document.createElement('p');
      // G2-33: the failures-only heading is filter-aware, but this line used to
      // fall through to "runs appear here when processing takes place" even on
      // a day where matchingRuns.length runs plainly did happen — just none of
      // them failed. Say what's true for that case instead.
      if (state.runsFailuresOnly && matchingRuns.length) {
        // X-03: the server narrows the day payload to the picked facet, so
        // matchingRuns is the facet's runs, not the day's. Reported as the day
        // it told the owner nothing failed when the runs it did not fetch may
        // well have. Name the filter that produced the number.
        const count = matchingRuns.length;
        const runWord = count === 1 ? 'run' : 'runs';
        detail.textContent = facetFiltered
          ? window.JournalI18n.render('journal.window.thinking.visible.text.all.value1.value2.in.value3.on', [['value1', String(count)], ['value2', String(runWord)], ['value3', String(facetTitle)]])
          : window.JournalI18n.render('journal.window.thinking.visible.text.all.value1.value2.on.this.day', [['value1', String(count)], ['value2', String(runWord)]]);
      } else if (route.talent) {
        detail.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.no.runs.on.this.day.match');
      } else if (facetFiltered) {
        // G2-B02: the facet branch is the sibling G2-33 missed. It fell through
        // to "runs appear here when processing takes place" on a day with 591
        // of them, which answers "is my journal thinking?" with the opposite of
        // the truth. Say what the filter did, and count the day only when an
        // unfiltered read of this same day supplied the number.
        detail.textContent = dayTotal
          ? window.JournalI18n.render('journal.window.thinking.visible.text.value1.value2.on.this.day.none', [['value1', String(dayTotal)], ['value2', String(dayTotal === 1 ? 'run' : 'runs')], ['value3', String(facetTitle)]])
          : window.JournalI18n.render('journal.window.thinking.visible.text.no.run.on.this.day.carries', [['value1', String(facetTitle)]]);
      } else {
        detail.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.runs.appear.here.when.processing.takes');
      }
      host.append(heading, detail);
      // X-03: this escape used to be withheld whenever the failures-only box
      // was ticked, which is the one combination the owner is most likely to
      // be stuck in — both filters on, nothing showing, and only one of the
      // two ways out offered. Both buttons appear when both filters apply.
      if (facetFiltered && !route.talent) {
        const showAll = document.createElement('button');
        showAll.type = 'button';
        showAll.className = 'thinking-runs-control';
        showAll.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.show.all.facets');
        showAll.addEventListener('click', () => {
          const picker = $('thinkingRunsFacet');
          if (picker) picker.value = '';
          applyRunsFacetSelection('');
        });
        host.append(showAll);
      }
      if (state.runsFailuresOnly && matchingRuns.length) {
        const showAll = document.createElement('button');
        showAll.type = 'button';
        showAll.className = 'thinking-runs-control';
        showAll.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.show.all.runs');
        showAll.addEventListener('click', () => {
          state.runsFailuresOnly = false;
          renderThinkingRunsDay(payload, route);
          // B41: the re-render replaces this button's own host, so without
          // this the focus falls back to body. Land on the same control the
          // checkbox handler lands on.
          host.querySelector('input[type=checkbox]')?.focus();
        });
        host.append(showAll);
      }
      return;
    }
    const groups = new Map();
    runs.forEach((run) => groups.set(run.name, [...(groups.get(run.name) || []), run]));
    const expandEveryGroup = Boolean(route.talent) || runs.length <= thinkingRunsExpandAllBelow;
    groups.forEach((group, name) => {
      const section = document.createElement('section');
      section.className = 'thinking-runs-group';
      const heading = document.createElement('h3');
      heading.textContent = talentLabel(name);
      section.appendChild(heading);
      const details = document.createElement('details');
      details.className = 'thinking-runs-group-detail';
      details.open = state.runsGroupOpen.has(name) ? state.runsGroupOpen.get(name) : expandEveryGroup;
      const summary = document.createElement('summary');
      const failedInGroup = group.filter((run) => run.failed).length;
      summary.textContent = failedInGroup
        ? window.JournalI18n.render('journal.window.thinking.visible.text.value1.run.value2.value3.failed', [['value1', String(group.length)], ['value2', String(group.length !== 1 ? 's' : '')], ['value3', String(failedInGroup)]])
        : window.JournalI18n.render('journal.window.thinking.visible.text.value1.run.value2', [['value1', String(group.length)], ['value2', String(group.length !== 1 ? 's' : '')]]);
      details.appendChild(summary);
      details.addEventListener('toggle', () => state.runsGroupOpen.set(name, details.open));
      const exact = document.createElement('p');
      exact.className = 'thinking-runs-group-id';
      exact.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.exact.id.value1', [['value1', String(name)]]);
      details.appendChild(exact);
      // Computed from the whole group, not just the shown page, so the
      // note and the set of hidden columns don't change shape as more rows
      // page in (G2-46).
      const groupNote = thinkingRunGroupNote(talentLabel(name), group);
      if (groupNote) {
        const note = document.createElement('p');
        note.className = 'thinking-runs-group-note';
        note.textContent = groupNote.text;
        details.appendChild(note);
      }
      const hiddenColumns = thinkingRunHiddenColumns(group, groupNote ? groupNote.spokenFor : new Set());
      const shown = state.runsGroupShown.get(name) || thinkingRunsPageSize;
      renderThinkingRunList(details, group.slice(0, shown), hiddenColumns);
      if (group.length > shown) {
        const remaining = group.length - shown;
        const next = Math.min(thinkingRunsPageSize, remaining);
        const more = document.createElement('button');
        more.type = 'button';
        more.className = 'thinking-runs-control';
        more.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.show.value1.more.run.value2.value3', [['value1', String(next)], ['value2', String(next !== 1 ? 's' : '')], ['value3', String(remaining)]]);
        more.addEventListener('click', () => {
          state.runsGroupShown.set(name, shown + thinkingRunsPageSize);
          state.runsGroupOpen.set(name, true);
          renderThinkingRunsDay(payload, route);
        });
        details.appendChild(more);
      }
      section.appendChild(details);
      host.appendChild(section);
    });
  }

  function renderThinkingRunsLoading() {
    const host = $('thinkingRunsContent');
    if (!host) return;
    host.replaceChildren();
    const message = document.createElement('p');
    message.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.loading.runs');
    host.appendChild(message);
  }

  function renderThinkingRunsFailure() {
    const host = $('thinkingRunsContent');
    if (!host) return;
    host.replaceChildren();
    const message = document.createElement('p');
    message.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.couldn.t.load.runs');
    const retry = document.createElement('button');
    retry.type = 'button';
    retry.className = 'thinking-runs-retry';
    retry.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.try.again');
    retry.addEventListener('click', () => loadThinkingRuns(parseThinkingHash(), true));
    host.append(message, retry);
  }

  function renderThinkingUpdatedDays(days) {
    const host = $('thinkingRunsUpdated');
    if (!host) return;
    host.replaceChildren();
    // These are past days whose newest material the day's thinking hasn't
    // covered. The read can't tell a day waiting its turn from one that is held
    // back, so the label states where the day stands and promises no progress.
    const list = Array.isArray(days) ? days : [];
    host.hidden = list.length === 0;
    if (!list.length) return;
    const label = document.createElement('span');
    label.className = 'thinking-runs-updated-label';
    label.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.thinking.hasn.t.caught.up.with');
    host.appendChild(label);
    list.forEach((day) => {
      const link = document.createElement('a');
      link.href = thinkingRunsHash(currentThinkingRunsRoute({kind: 'runs', day, talent: '', useId: ''}));
      link.textContent = window.JournalFormat.day(day);
      host.appendChild(link);
    });
  }

  function loadThinkingUpdatedDays() {
    const token = beginThinkingRequest('day');
    ctx.read('/app/thinking/api/updated-days').then((days) => {
      if (isCurrentThinkingRequest(token)) renderThinkingUpdatedDays(days);
    }).catch(() => {
      if (isCurrentThinkingRequest(token)) setThinkingStatus('thinkingRunsStatus', window.JournalI18n.render('journal.window.thinking.window.explanation.some.run.details.aren.t.available'));
    }).finally(() => {
      if (isCurrentThinkingRequest(token)) state.runsInFlight.day = null;
    });
  }

  function thinkingDayUrl(route) {
    const path = `/app/thinking/api/talents/${route.day}`;
    return state.runsFacetExplicit ? `${path}?facet=${encodeURIComponent(state.runsFacet)}` : path;
  }

  function updateThinkingRunsDayControls(route) {
    const today = todayThinkingDay();
    const date = $('thinkingRunsDate');
    if (date) {
      date.value = runsDayInputValue(route.day);
      date.max = runsDayInputValue(today);
    }
    // Tomorrow has not happened; an empty future day reads as missing processing.
    const next = $('thinkingRunsNext');
    if (next) next.disabled = route.day >= today;
  }

  function resetThinkingRunGroups(key) {
    if (state.runsGroupKey === key) return;
    state.runsGroupKey = key;
    state.runsGroupOpen.clear();
    state.runsGroupShown.clear();
  }

  // X-03: a run that was mid-flight when the day was read is the one fact on
  // this page with a shelf life. Serving it from the day cache meant "still
  // running" outlived the run — the owner came back an hour later and the
  // table still said so. A day that holds one is read again, never cached.
  function thinkingDayHasRunningRun(payload) {
    return normalizedThinkingRuns(payload).some((run) => run.status === 'running');
  }

  function loadThinkingRuns(route, force = false) {
    if (!route || route.kind !== 'runs') return;
    const key = thinkingCacheKey('day', {day: route.day, facet: state.runsFacet});
    updateThinkingRunsDayControls(route);
    resetThinkingRunGroups(key);
    let cached = readThinkingCache('day', key);
    if (cached && thinkingDayHasRunningRun(cached)) {
      evictThinkingCache('day', key);
      cached = null;
    }
    if (cached && !force) {
      renderThinkingRunsDay(cached, route);
      loadThinkingUpdatedDays();
      return;
    }
    renderThinkingRunsLoading();
    loadThinkingRequest(
      'day',
      key,
      () => ctx.read(thinkingDayUrl(route)),
      (payload) => {
        renderThinkingRunsDay(payload, route);
        loadThinkingUpdatedDays();
      },
      renderThinkingRunsFailure,
      (payload) => !thinkingDayHasRunningRun(payload),
    );
  }

  function navigateThinkingRun(run) {
    const route = parseThinkingHash();
    if (!route?.day) return;
    const next = thinkingRunsRoute({
      kind: 'runs', day: route.day, talent: run.name, useId: run.id,
      facet: route.facet,
      facetExplicit: route.facetExplicit,
    });
    writeEntry('push', thinkingRunsHash(next));
    routeThinkingHash('pointer');
  }

  function renderThinkingRunLog(run) {
    const panel = $('thinkingRunsLogPanel');
    if (!panel) return;
    panel.replaceChildren();
    if (run.status === 'running') {
      const progress = document.createElement('p');
      progress.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.this.run.is.still.in.progress');
      const check = document.createElement('p');
      check.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.check.back.soon');
      panel.append(progress, check);
      return;
    }
    // The run-level outcome is run.status; a step with no recorded detail only
    // means "nothing was written for this step", never that the run failed.
    const runFinished = run.failed !== true && run.status === 'completed';
    // G2-38: two adjacent steps with nothing recorded produced two identical
    // "no detail was recorded for this step" lines. Collapse a run of
    // identical empty markers into the one line that already said it.
    let lastEmptyNote = null;
    // B40: the collapsed line has to say how many steps it stands for, or N
    // identical steps read as one.
    let lastEmptyNoteElement = null;
    let lastEmptyNoteCount = 0;
    (Array.isArray(run.events) ? run.events : []).forEach((event) => {
      const item = document.createElement('div');
      const fields = [['thinking', event.thinking], ['tools', event.tools], ['args', event.args], ['result', event.result], ['error', event.error]];
      fields.forEach(([label, value]) => {
        if (value === null || value === undefined || value === '') return;
        const fact = document.createElement('p');
        fact.textContent = `${label}: ${typeof value === 'string' ? value : JSON.stringify(value)}`;
        item.appendChild(fact);
      });
      if (!item.children.length) {
        const noteText = runFinished
          ? window.JournalI18n.render('journal.window.thinking.window.explanation.no.detail.was.recorded.for.this')
          : (event.event === 'tool_start' ? window.JournalI18n.render('journal.window.thinking.window.explanation.tool.call.did.not.complete') : window.JournalI18n.render('journal.window.thinking.window.state.text.did.not.complete'));
        if (noteText === lastEmptyNote) {
          lastEmptyNoteCount += 1;
          if (lastEmptyNoteElement) {
            lastEmptyNoteElement.textContent = `${noteText} (×${lastEmptyNoteCount})`;
          }
          return;
        }
        lastEmptyNote = noteText;
        const note = document.createElement('p');
        note.textContent = noteText;
        item.appendChild(note);
        lastEmptyNoteElement = note;
        lastEmptyNoteCount = 1;
      } else {
        lastEmptyNote = null;
        lastEmptyNoteElement = null;
        lastEmptyNoteCount = 0;
      }
      panel.appendChild(item);
    });
  }

  function renderThinkingRunDetail(run, route) {
    state.runsDetail = run;
    const detail = $('thinkingRunsDetail');
    if (detail) detail.hidden = false;
    setText('thinkingRunsDetailHeading', talentLabel(run.name || route.talent));
    const exact = [run.name || route.talent, run.provider, run.model].filter(Boolean).join(' · ');
    setText('thinkingRunsDetailIdsText', exact);
    setHidden('thinkingRunsDetailIds', !exact);
    const facts = $('thinkingRunsDetailFacts');
    if (facts) {
      facts.replaceChildren();
      appendThinkingRunFacts(facts, run);
    }
    const output = $('thinkingRunsOutputTab');
    if (output) output.hidden = !run.output_file;
    $('thinkingRunsNoOutput').hidden = !!run.output_file;
    renderThinkingRunLog(run);
    activateThinkingRunDetailTab('thinkingRunsLogTab', 'history');
  }

  function clearThinkingRunRenderState() {
    state.runsDetail = null;
    $('thinkingRunsDetail').hidden = true;
    setHidden('thinkingRunsDetailIds', true);
    $('thinkingRunsDetailFacts')?.replaceChildren();
    $('thinkingRunsOutputTab').hidden = true;
    $('thinkingRunsNoOutput').hidden = true;
    $('thinkingRunsOutputPanel')?.replaceChildren();
  }

  function renderThinkingRunFailure() {
    const panel = $('thinkingRunsLogPanel');
    if (!panel) return;
    panel.replaceChildren();
    const message = document.createElement('p');
    message.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.couldn.t.load.that.run');
    panel.appendChild(message);
  }

  function renderThinkingRunPending(route) {
    const detail = $('thinkingRunsDetail');
    if (detail) detail.hidden = false;
    setText('thinkingRunsDetailHeading', talentLabel(route.talent));
    setHidden('thinkingRunsDetailIds', true);
    $('thinkingRunsDetailFacts')?.replaceChildren();
    $('thinkingRunsOutputTab').hidden = true;
    $('thinkingRunsNoOutput').hidden = true;
    renderThinkingRunLog({status: 'running'});
    activateThinkingRunDetailTab('thinkingRunsLogTab', 'history');
  }

  function loadThinkingRun(route) {
    if (!route?.useId) return;
    const selectedDetail = $('thinkingRunsDetail');
    if (selectedDetail) {
      selectedDetail.hidden = false;
      requestAnimationFrame(() => {
        if (selectedDetail.isConnected) selectedDetail.scrollIntoView({block: 'start'});
      });
    }
    const key = thinkingCacheKey('run', {useId: route.useId});
    const cached = readThinkingCache('run', key);
    if (cached) {
      const contextual = runContextFromRecord(route, cached);
      const contextChanged = contextual.key !== route.key;
      setThinkingRoute(contextual);
      renderThinkingRunDetail(cached, contextual);
      if (route.kind === 'run-id' || contextChanged) loadThinkingRuns(contextual);
      return;
    }
    const detail = $('thinkingRunsDetail');
    if (detail) detail.hidden = false;
    setText('thinkingRunsDetailHeading', talentLabel(route.talent));
    setHidden('thinkingRunsDetailIds', true);
    const panel = $('thinkingRunsLogPanel');
    if (panel) panel.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.loading.run.details');
    loadThinkingRequest(
      'run',
      key,
      () => ctx.read(`/app/thinking/api/run/${encodeThinkingSegment(route.useId)}`),
      (run) => {
        if (run?.reason_code === 'talent_run_pending') {
          renderThinkingRunPending(route);
          return;
        }
        const contextual = runContextFromRecord(route, run);
        const contextChanged = contextual.key !== route.key;
        setThinkingRoute(contextual);
        renderThinkingRunDetail(run, contextual);
        if (route.kind === 'run-id' || contextChanged) loadThinkingRuns(contextual);
      },
      renderThinkingRunFailure,
      (run) => run?.reason_code !== 'talent_run_pending',
    );
  }

  function activateThinkingRunDetailTab(tabId, origin) {
    if ($(tabId)?.hidden) return;
    const output = tabId === 'thinkingRunsOutputTab';
    $('thinkingRunsLogPanel').hidden = output;
    $('thinkingRunsOutputPanel').hidden = !output;
    activateThinkingTab(tabId, origin, 'thinkingRunsDetailTabs', 'thinkingRunsDetailHeading');
    if (output) loadThinkingOutput();
  }

  function renderThinkingOutput(panel, payload) {
    panel.replaceChildren();
    const raw = String(payload.content || '');
    if (payload.format === 'md') {
      const documentView = document.createElement('div');
      documentView.className = 'thinking-output-document';
      const body = raw.replace(/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, '');
      documentView.innerHTML = window.AppServices.renderMarkdown(body);
      panel.appendChild(documentView);
      const source = document.createElement('details');
      const label = document.createElement('summary');
      label.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.stored.text');
      const text = document.createElement('pre');
      text.textContent = raw;
      source.append(label, text);
      panel.appendChild(source);
    } else {
      const text = document.createElement('pre');
      text.textContent = raw;
      panel.appendChild(text);
    }
  }

  function loadThinkingOutput() {
    const run = state.runsDetail;
    if (!run?.output_file || !run.day) return;
    const key = thinkingCacheKey('output', {day: run.day, file: run.output_file});
    const cached = readThinkingCache('output', key);
    const panel = $('thinkingRunsOutputPanel');
    if (cached) {
      renderThinkingOutput(panel, cached);
      return;
    }
    panel.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.loading.output');
    const encoded = String(run.output_file).split('/').map(encodeThinkingSegment).join('/');
    loadThinkingRequest(
      'output', key,
      () => ctx.read(`/app/thinking/api/output/${run.day}/${encoded}`),
      (payload) => renderThinkingOutput(panel, payload),
      () => { panel.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.couldn.t.load.that.output'); },
    );
  }

  function openThinkingPrompt() {
    const run = state.runsDetail;
    if (!run?.name) return;
    const modal = $('thinkingRunsPromptModal');
    state.runsModalFocus = document.activeElement;
    modal.showModal();
    const content = $('thinkingRunsPromptContent');
    $('thinkingRunsRequestContent').textContent = run.prompt || window.JournalI18n.render('journal.window.thinking.visible.text.no.request.text.was.saved.for');
    content.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.loading.current.template');
    const key = thinkingCacheKey('prompt', {talent: run.name});
    const cached = readThinkingCache('prompt', key);
    if (cached) {
      content.textContent = cached.full_prompt || '';
      return;
    }
    loadThinkingRequest(
      'prompt', key,
      () => ctx.read(`/app/thinking/api/preview/${encodeThinkingSegment(run.name)}`),
      (payload) => { content.textContent = payload.full_prompt || ''; },
      () => { content.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.couldn.t.load.the.current.template'); },
    );
  }

  // One path for changing the facet, so the reset in the empty state and the
  // picker itself cannot drift apart (G2-B02).
  function applyRunsFacetSelection(value) {
    setRunsFacet(value, Boolean(value));
    const route = parseThinkingHash();
    if (route?.kind !== 'runs') return;
    const next = currentThinkingRunsRoute(route);
    writeEntry('push', thinkingRunsHash(next));
    routeThinkingHash('pointer');
  }

  function closeThinkingPrompt() {
    $('thinkingRunsPromptModal').close();
    // The modal layer sees a closed <dialog> only at its next change. Until
    // then it still holds the dialog active and pulls any focus back into it,
    // which, closed, leaves focus on the page.
    window.ConveyModalLayer?.reconcile?.({restore: false});
    state.runsModalFocus?.focus();
    state.runsModalFocus = null;
  }

  function bindThinkingRuns() {
    $('thinkingRunsPromptModal')?.addEventListener('cancel', (event) => {
      event.preventDefault();
      closeThinkingPrompt();
    });
    bindThinkingTablist('thinkingRunsDetailTabs', (tab, origin) => {
      activateThinkingRunDetailTab(tab.id, origin);
    });
    $('thinkingRunsPrevious')?.addEventListener('click', () => navigateThinkingRunsDay(-1));
    $('thinkingRunsNext')?.addEventListener('click', () => navigateThinkingRunsDay(1));
    $('thinkingRunsDate')?.addEventListener('change', (event) => navigateThinkingRunsDay(0, runsDayFromInput(event.target.value)));
    $('thinkingRunsFacet')?.addEventListener('change', (event) => applyRunsFacetSelection(event.target.value));
    $('thinkingRunsPrompt')?.addEventListener('click', openThinkingPrompt);
    $('thinkingRunsPromptClose')?.addEventListener('click', closeThinkingPrompt);
  }

  function navigateThinkingRunsDay(amount, requestedDay = '') {
    const route = parseThinkingHash();
    const today = todayThinkingDay();
    const requested = requestedDay || shiftThinkingDay(route?.day || today, amount);
    const day = requested > today ? today : requested;
    const next = currentThinkingRunsRoute({kind: 'runs', day, talent: '', useId: ''});
    writeEntry('push', thinkingRunsHash(next));
    routeThinkingHash('pointer');
  }

  // X-03: every map below is a plain object literal, and a run record supplies
  // the key. A model id or provider slug of `constructor` (or `toString`, or
  // `__proto__`) reached Object.prototype and rendered a function's source as
  // a fact on the table. Read own properties only.
  function mapValue(map, key) {
    if (!map || typeof map !== 'object' || typeof key !== 'string') return undefined;
    return Object.hasOwn(map, key) ? map[key] : undefined;
  }

  function providerLabel(provider) {
    return mapValue(providerLabels, provider) || provider || window.JournalI18n.render('journal.window.thinking.field.label.provider');
  }

  // Prior partial G2-05: the runs table and cards still showed the raw
  // provider-prefixed model id (`local/qwen3.5-4b`) and raw provider slug as
  // top-level facts. Readable names go in the fact/column; the exact ids stay
  // in the run detail view's existing "exact ids" disclosure, which reads
  // run.provider/run.model directly and is untouched by this.
  function runModelLabel(run) {
    const modelId = String(run.model || '').trim();
    if (!modelId) return run.model;
    const provider = String(run.provider || '').trim();
    const prefix = `${provider}/`;
    // The local lane's bare wire id ("qwen3.5-4b") would otherwise disagree
    // with stats' "Qwen 3.5 4B (local)" for the same model (G2-43).
    const qualifiedId = prefix.length > 1 && modelId.startsWith(prefix) ? modelId : `${prefix}${modelId}`;
    const knownLabel = mapValue(LOCAL_MODEL_LABELS, qualifiedId.toLowerCase());
    if (knownLabel) return knownLabel;
    return prefix.length > 1 && modelId.startsWith(prefix) ? modelId.slice(prefix.length) : modelId;
  }

  // The runs table has a model column beside this one, so the provider column
  // names the provider and the model column names the model -- "Claude" in
  // both said one thing twice. This is the same set the shell's
  // JournalFormat.processingLane already uses, and stats' token-card.js now
  // agrees. providerLabel() is left alone: it serves the BYO chooser, where
  // the owner really does hold a "Claude key". The local lane is not a brand
  // and stays lowercase (X-05, G2-43, S-5).
  const RUN_PROVIDER_LABELS = {
    anthropic: 'Anthropic',
    google: 'Google',
    openai: 'OpenAI',
  };

  function runProviderLabel(run) {
    if (!run.provider) return run.provider;
    const key = String(run.provider).toLowerCase();
    if (key === 'local') return 'local';
    return mapValue(RUN_PROVIDER_LABELS, key) || providerLabel(run.provider);
  }

  // X-03: the register is sentence case, and lowercasing the whole title also
  // flattened the third-party names inside it — "Gemini Summary" arrived as
  // "gemini summary", which is not what Google calls it. A vendor's own name
  // keeps the vendor's casing; every other word lowercases as before. A Map,
  // not an object literal, so a talent titled "constructor" reads as itself.
  const TITLE_BRANDS = ['AI', 'Anthropic', 'Claude', 'Cloudflare', 'Gemini', 'Google', 'GPT', 'OpenAI', 'Qwen'];
  const titleBrandByKey = new Map(TITLE_BRANDS.map((name) => [name.toLowerCase(), name]));

  function talentTitleCase(title) {
    return String(title).replace(/[A-Za-z]+/g, (word) => (
      titleBrandByKey.get(word.toLowerCase()) || word.toLowerCase()
    ));
  }

  // The day payload already carries every talent's authored title; the runs
  // view used to build its headings out of the raw id instead, so retired and
  // agent-internal vocabulary reached the owner as product words (X-04).
  function rememberTalentTitles(payload) {
    const talents = payload && typeof payload.talents === 'object' && payload.talents ? payload.talents : null;
    if (!talents) return;
    const titles = {};
    Object.entries(talents).forEach(([id, metadata]) => {
      const title = String((metadata && metadata.title) || '').trim();
      // The reads layer defaults a missing title to the talent key, so a title
      // equal to the id is "this talent has no title", not a name anyone chose.
      if (title && title !== id) titles[id] = talentTitleCase(title);
    });
    talentTitles = titles;
  }

  function talentLabel(name) {
    const id = String(name || '');
    if (!id) return '';
    return mapValue(talentLabels, id) || mapValue(talentTitles, id) || id.replace(/[_:]+/g, ' ');
  }

  function configuredProviders() {
    return Object.entries(state.keys.api_keys || {})
      .filter(([, configured]) => !!configured)
      .map(([provider]) => provider);
  }

  function localEndpointConfigured() {
    return !!state.providers.local_override?.enabled;
  }

  function byoIsUsable() {
    if (state.providers.active?.provider === 'chatgpt') return true;
    return configuredProviders().length > 0 || localEndpointConfigured();
  }

  function selectedByoProvider() {
    const select = $('byoProvider');
    return state.selectedByoProvider || select?.value || defaultByoProvider();
  }

  function defaultByoProvider() {
    const activeProvider = state.providers.active?.provider || '';
    if (activeProvider === 'chatgpt') return 'chatgpt';
    if (localEndpointConfigured() && activeProvider === 'local') return 'local';
    if (providerEnv[activeProvider]) return activeProvider;
    if (localEndpointConfigured() && configuredProviders().length === 0) return 'local';
    return configuredProviders()[0] || 'anthropic';
  }

  function laneProvider(lane) {
    if (lane === 'local') return 'local';
    return selectedByoProvider();
  }

  function localReadiness() {
    const avail = state.localAvailability;
    const availCode = avail?.reason_code;
    if (avail?.available === false) {
      return {
        status: 'blocked',
        reason: availCode || 'availability_unavailable',
        summary: avail.reason || '',
        detail: '',
      };
    }
    // An unselected provider has no Generate readiness yet. A successful
    // artifact check is enough to offer selection; activation owns runtime start.
    if (avail?.available === true && state.providers.active_lane?.lane !== 'local') {
      return {status: 'ready', reason: 'ready', summary: '', detail: ''};
    }
    const readiness = state.providers.provider_status?.local;
    if (readiness) {
      const ready = !!readiness.generate_ready;
      let issue = Array.isArray(readiness.issues) ? readiness.issues[0] : '';
      if (!ready && availCode && availCode !== 'ready') {
        issue = availCode;
      }
      return {
        status: ready ? 'ready' : 'blocked',
        reason: ready ? 'ready' : (issue || availCode || ''),
        summary: issue || avail?.reason || '',
        detail: '',
      };
    }
    if (avail?.available === true) {
      return {status: 'ready', reason: 'ready', summary: '', detail: ''};
    }
    return {status: '', reason: '', summary: '', detail: ''};
  }

  function localIsReady() {
    if (state.localAvailability?.available === false) return false;
    if (state.localAvailability?.available === true && state.providers.active_lane?.lane !== 'local') return true;
    const readiness = state.providers.provider_status?.local;
    return !!readiness?.generate_ready;
  }

  // Local is the selected lane and installed (the provider reports no issue), but
  // processing has not confirmed it ready yet.
  function localAwaitingConfirmation() {
    return state.providers.active_lane?.lane === 'local'
      && !localIsReady()
      && !localEndpointConfigured()
      && !localReadiness().summary;
  }

  function localUnreadyCopy() {
    if (localAwaitingConfirmation()) return window.JournalI18n.render('journal.window.thinking.window.explanation.installed.waiting.for.processing.to.confirm');
    if (!state.localAvailability) return window.JournalI18n.render('journal.window.thinking.window.explanation.checking.whether.your.journal.s.computer');
    return window.JournalI18n.render('journal.window.thinking.window.explanation.couldn.t.tell.whether.your.journal');
  }

  // What the main view's local card says when local is blocked, by reason. The
  // readiness summary it used to echo is a server phrase ("local runtime is not
  // installed") or, with local selected, a bare issue code. The card's link
  // leads to local setup, which says what to do next.
  const localLaneBlocked = {
    setup: window.JournalI18n.render('journal.window.thinking.window.explanation.not.set.up.on.your.journal'),
    ineligible: window.JournalI18n.render('journal.window.thinking.window.explanation.installed.but.it.won.t.start'),
    check: window.JournalI18n.render('journal.window.thinking.window.explanation.couldn.t.check.local.setup'),
    unhealthy: window.JournalI18n.render('journal.window.thinking.window.explanation.local.thinking.isn.t.ready.yet'),
    other: window.JournalI18n.render('journal.window.thinking.window.explanation.couldn.t.get.local.thinking.ready'),
  };

  function localLaneBlockedKind(reason) {
    if (localSetupMissingReasons.has(reason)) return 'setup';
    if (reason === 'host_ineligible') return 'ineligible';
    if (['local_probe_failed', 'gpu_probe_failed', 'availability_unavailable'].includes(reason)) return 'check';
    if (localServerUnhealthyReasons.has(reason)) return 'unhealthy';
    return 'other';
  }

  function localLaneUnreadyLine(local) {
    // Until availability arrives, readiness for an unselected local lane can
    // carry another lane's issues, so it is still checking.
    const settled = !!state.localAvailability || state.providers.active_lane?.lane === 'local';
    // A blocked state with no reason is installed and waiting for processing
    // to confirm it.
    if (local.status !== 'blocked' || !local.reason || !settled) return localUnreadyCopy();
    return localLaneBlocked[localLaneBlockedKind(local.reason)];
  }

  function localIsGpuBlocked() {
    const reason = localReadiness().reason;
    return reason === 'gpu_unavailable';
  }

  function activeLanePayload() {
    return state.providers.active_lane || {};
  }

  function confidentialProvenancePresent() {
    return !!activeLanePayload().confidential_provenance_configured;
  }

  function byoKindForProvider(provider) {
    if (provider === 'local') return 'endpoint';
    if (provider === 'chatgpt') return 'chatgpt';
    return 'key';
  }

  function activeBrain() {
    const lane = state.providers.active_lane?.lane || 'none';
    const provider = state.providers.active?.provider || '';
    const byoUsable = byoIsUsable();

    if (lane === 'byo' && byoUsable) {
      return {
        kind: 'byo',
        byoKind: byoKindForProvider(provider),
        provider,
        providerLabel: providerLabel(provider),
      };
    }
    if (lane === 'local' && localIsReady()) {
      return {kind: 'local', providerLabel: window.JournalI18n.render('journal.window.thinking.provider.local')};
    }
    if (lane === 'confidential' && confidentialProvenancePresent()) {
      return {kind: 'confidential', providerLabel: window.JournalI18n.render('journal.window.thinking.window.state.text.confidential.processing')};
    }
    return {kind: 'none', providerLabel: ''};
  }

  function laneIsUsable(lane) {
    if (lane === 'byo') return byoIsUsable();
    if (lane === 'confidential') return confidentialProvenancePresent();
    if (lane === 'local') return localIsReady() && !localEndpointConfigured();
    return false;
  }

  function relativeTime(iso) {
    if (!iso) return '';
    const stamp = Date.parse(iso);
    if (Number.isNaN(stamp)) return '';
    const seconds = Math.max(0, Math.floor((Date.now() - stamp) / 1000));
    if (seconds < 60) return window.JournalI18n.render('journal.window.thinking.window.state.text.just.now');
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes} min ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours} hours ago`;
    return shortDate(iso);
  }

  function shortDate(iso) {
    if (!iso) return '';
    if (window.JournalClock?.formatInstant) {
      return window.JournalClock.formatInstant(iso, {month: 'short', day: 'numeric'}).toLowerCase();
    }
    const date = new Date(iso);
    if (Number.isNaN(date.getTime())) return '';
    return date.toLocaleDateString(undefined, {month: 'short', day: 'numeric'}).toLowerCase();
  }

  function buildNotice(key, { tone = '', owner = false, holdCard = false } = {}) {
    if (!key) return null;
    const chatgptText = copy.byo_setup?.chatgpt || {};
    let text = chatgptText[key] || '';
    if (['denied', 'expired', 'unfinished'].includes(key) && chatgptText.keep_failing) {
      text = text ? `${text} ${chatgptText.keep_failing}` : chatgptText.keep_failing;
    }
    return { key, text, tone, owner, holdCard };
  }

  function setChatGptNotice(key, { tone = 'error', owner = false, holdCard = false } = {}) {
    state.chatgpt.notice = buildNotice(key, { tone, owner, holdCard });
  }

  function clearChatGptNotice(force = false) {
    if (!force && state.chatgpt.notice?.owner) return;
    state.chatgpt.notice = null;
  }

  function stopChatGptPoll() {
    state.chatgpt.pollToken = (state.chatgpt.pollToken || 0) + 1;
    if (state.chatgpt.pollTimer) {
      ctx.clearTimeout(state.chatgpt.pollTimer);
      state.chatgpt.pollTimer = null;
    }
  }

  function renderGlance() {
    return drawn(() => renderGlanceNow());
  }

  function renderGlanceNow() {
    const brain = state.providers.brain || {};
    let intro;
    let glanceState = null;
    let savedLaneAttr = null;
    let activeValue = brain.headline || '';

    if (brain.state === 'ready') {
      intro = window.JournalI18n.render('journal.window.thinking.window.explanation.your.model.is.ready.you.can');
    } else if (brain.reason_code === 'thinking_engine_not_chosen') {
      if (state.savedLane === 'byo') {
        glanceState = 'saved';
        savedLaneAttr = 'byo';
        activeValue = window.JournalI18n.render('journal.window.thinking.window.explanation.finish.setting.up.your.own.model');
        intro = window.JournalI18n.render('journal.window.thinking.window.explanation.finish.setting.up.your.own.model.variant2');
      } else if (state.savedLane === 'spp') {
        glanceState = 'saved';
        savedLaneAttr = 'spp';
        activeValue = window.JournalI18n.render('journal.window.thinking.window.explanation.finish.setting.up.confidential.processing');
        intro = window.JournalI18n.render('journal.window.thinking.window.explanation.finish.setting.up.confidential.processing.your');
      } else if (state.savedLane === 'local') {
        if (localCapabilityBlocked()) {
          glanceState = 'cant_run';
          savedLaneAttr = null;
          activeValue = bundledLocalUnavailable()
            ? window.JournalI18n.render('journal.window.thinking.window.explanation.local.thinking.isn.t.available.where')
            : window.JournalI18n.render('journal.window.thinking.window.explanation.your.journal.s.computer.can.t');
          intro = bundledLocalUnavailable()
            ? window.JournalI18n.render('journal.window.thinking.window.explanation.choose.another.thinking.option.your.journal')
            : window.JournalI18n.render('journal.window.thinking.window.explanation.your.journal.s.computer.can.t.variant2');
        } else {
          glanceState = 'saved';
          savedLaneAttr = 'local';
          activeValue = window.JournalI18n.render('journal.window.thinking.window.explanation.finish.setting.up.local.thinking');
          intro = window.JournalI18n.render('journal.window.thinking.window.explanation.finish.setting.up.local.thinking.your');
        }
      } else {
        glanceState = 'not_chosen';
        savedLaneAttr = null;
        activeValue = brain.headline || '';
        intro = window.JournalI18n.render('journal.window.thinking.window.explanation.choose.where.the.thinking.happens.your');
      }
    } else if (brain.progressing && ['local_artifact_not_ready', 'local_runtime_not_ready'].includes(brain.reason_code)) {
      intro = window.JournalI18n.render('journal.window.thinking.window.explanation.local.thinking.is.getting.ready.your');
    } else {
      intro = window.JournalI18n.render('journal.window.thinking.window.explanation.choose.or.check.your.model.in');
    }
    setText('thinkingIntro', intro);
    const glance = $('brainGlance');
    const glanceLabel = $('thinkingActiveLane');
    const identity = brain.identity || {};
    const evidence = brain.evidence || {};
    if (glance) {
      if (glanceState) {
        setElementData(glance, 'glanceState', 'data-glance-state', glanceState);
        if (savedLaneAttr) {
          setElementData(glance, 'savedLane', 'data-saved-lane', savedLaneAttr);
        } else {
          removeElementData(glance, 'savedLane', 'data-saved-lane');
        }
      } else {
        removeElementData(glance, 'glanceState', 'data-glance-state');
        removeElementData(glance, 'savedLane', 'data-saved-lane');
      }
      glance.classList.toggle('none', brain.state === 'unknown' && !brain.reason_code);
    }
    writeHidden(glanceLabel, false);
    setText('thinkingActiveLane', window.JournalI18n.render('journal.window.thinking.status.text.right.now'));
    setText('thinkingActiveValue', activeValue);

    renderBrainAction(brain.action || null);

    const isChatGpt = brain.identity?.provider === 'chatgpt' || (!brain.identity?.provider && state.providers.active?.provider === 'chatgpt');
    const chatgptText = copy.byo_setup?.chatgpt || {};
    const chatgptLabel = copy.provider_labels?.chatgpt || '';

    if (isChatGpt) {
      const reasonCode = brain.reason_code || '';
      const isConfigured = state.providers.provider_status?.chatgpt?.configured === true;

      setHidden('chatgptGlancePlan', true);
      setHidden('chatgptGlanceReminder', true);
      setHidden('chatgptGlanceContinue', true);
      setHidden('chatgptGlanceUsage', true);
      setHidden('chatgptGlanceStartOver', true);
      setHidden('chatgptGlancePickOther', true);

      if (reasonCode === 'chatgpt_usage_limit') {
        setText('thinkingActiveDetail', '');
        setHidden('chatgptGlanceUsage', false);
        setText('chatgptGlanceUsageLane', chatgptLabel);
        setText('chatgptGlanceUsageTitle', chatgptText.usage_limit_title || '');
        setText('chatgptGlanceUsageBody', chatgptText.usage_limit_body || '');
        setText('chatgptGlanceUsageManage', chatgptText.manage_usage_link || '');
        externalLink($('chatgptGlanceUsageManage'), 'https://chatgpt.com/settings/usage');
        setHidden('brainCheckAction', true);
      } else if (reasonCode === 'chatgpt_not_eligible') {
        setText('thinkingActiveDetail', chatgptText.not_eligible || '');
        setHidden('chatgptGlanceStartOver', false);
        setText('chatgptGlanceStartOver', chatgptText.start_over || '');
        setHidden('chatgptGlancePickOther', false);
        setText('chatgptGlancePickOther', chatgptText.pick_other || '');
        setHidden('brainCheckAction', true);
      } else if (reasonCode === 'chatgpt_sign_in_required') {
        setText('thinkingActiveDetail', brain.reason_text || '');
        setHidden('chatgptGlanceContinue', false);
        setText('chatgptGlanceContinueLabel', chatgptText.continue || '');
        setHidden('brainCheckAction', true);
      } else if (!isConfigured) {
        setText('thinkingActiveDetail', chatgptText.signed_out || '');
        setHidden('chatgptGlanceContinue', false);
        setText('chatgptGlanceContinueLabel', chatgptText.continue || '');
        setHidden('brainCheckAction', true);
      } else {
        if (brain.state === 'ready') {
          const confirmed = evidence.age_text ? window.JournalI18n.render('journal.window.thinking.runs.visible.text.last.confirmed.value1.ago', [['value1', String(evidence.age_text)]]) : '';
          setText('thinkingActiveDetail', `${chatgptLabel}${confirmed}`);
        } else {
          setText('thinkingActiveDetail', `${chatgptLabel}: ${brain.reason_text || ''}`);
        }
        setHidden('chatgptGlancePlan', false);
        setText('chatgptGlancePlanLabel', chatgptText.plan || '');
        setText('chatgptGlancePlanManage', chatgptText.manage_usage_link || '');
        externalLink($('chatgptGlancePlanManage'), 'https://chatgpt.com/settings/usage');
        setHidden('chatgptGlanceReminder', false);
        setText('chatgptGlanceReminder', chatgptText.reminder || '');
      }
    } else {
      setHidden('chatgptGlancePlan', true);
      setHidden('chatgptGlanceReminder', true);
      setHidden('chatgptGlanceContinue', true);
      setHidden('chatgptGlanceUsage', true);
      setHidden('chatgptGlanceStartOver', true);
      setHidden('chatgptGlancePickOther', true);

      if (identity.lane && identity.provider && identity.model) {
        if (brain.state === 'ready') {
          const confirmed = evidence.age_text ? window.JournalI18n.render('journal.window.thinking.runs.visible.text.last.confirmed.value1.ago', [['value1', String(evidence.age_text)]]) : '';
          setText('thinkingActiveDetail', `${window.JournalFormat.processingLane(identity.lane)}${confirmed}`);
        } else {
          setText('thinkingActiveDetail', `${window.JournalFormat.processingLane(identity.lane)}: ${brain.reason_text || ''}`);
        }
      } else if (identity.lane || identity.provider || identity.model) {
        setText('thinkingActiveDetail', brain.reason_text || '');
      } else {
        setText('thinkingActiveDetail', '');
      }
    }

    // A headline that names the saved choice already says what's next, so the
    // brain's "nothing chosen yet" must not sit under it.
    if (glanceState === 'saved' || glanceState === 'cant_run') {
      setText('thinkingActiveDetail', '');
    }

    const identityText = [identity.lane, identity.provider, identity.model].filter(Boolean).join(' · ');
    setText('thinkingActiveIdentity', identityText);
    setHidden('thinkingIdentityDetails', !identityText);
  }

  function renderBrainAction(action) {
    const button = $('brainCheckAction');
    if (!button) return;
    // "open thinking" is for other pages; on this page it would go nowhere.
    if (!action?.label || (action.href && action.href.startsWith('/app/thinking'))) {
      writeHidden(button, true);
      button.onclick = null;
      return;
    }
    writeHidden(button, false);
    writeText(button, action.label);
    if (action.href) {
      button.onclick = () => {
        // Another view of the page opens in it where it can.
        if (!window.solRouter?.navigate?.(action.href)) window.location.href = action.href;
      };
      return;
    }
    if (action.refresh) {
      button.onclick = () => pressing(button, null, requestBrainCheck).catch((err) => {
        setMessage('thinkingActiveDetail', err.message, 'error');
      });
      return;
    }
    button.onclick = null;
  }

  async function requestBrainCheck() {
    const response = await api('api/brain/check', {method: 'POST', body: '{}'});
    if (response.brain) state.providers.brain = response.brain;
    renderGlance();
    followBrainCheck({newEvidence: true});
  }

  async function refreshConfidentialReadiness() {
    await requestBrainCheck();
    await refreshProviders();
  }

  function setCardActive(lane, active) {
    const card = $(`lane-${lane}`);
    if (!card) return;
    card.classList.toggle('active', active);
    writeHidden($(`${lane}ActiveTag`), !active);
  }

  function setPill(id, label, tone = '') {
    const pill = $(id);
    if (!pill) return;
    writeText(pill, label);
    pill.classList.toggle('hot', tone === 'hot');
    pill.classList.toggle('bad', tone === 'bad');
  }

  function renderConfidentialHowItWorks() {
    setText('confidentialLaneMore', copy.confidential?.more_label || '');
  }

  function confidentialCheckedLabel(attestation) {
    return relativeTime(attestation?.observed_at || '');
  }

  function renderConfidentialTrust() {
    return drawn(() => renderConfidentialTrustNow());
  }

  function renderConfidentialTrustNow() {
    const beats = copy.confidential?.setup?.trust_beats || {};
    const active = activeLanePayload();
    const attestation = active.confidential_attestation || {state: 'off'};
    const audio = confidentialAudioRender(active, attestation, copy.confidential);
    const audioKnown = confidentialAudio.known && !confidentialAudio.busy;
    // While its write is out the toggle stays drawn, busy, with the choice the
    // owner just made. What it says about where audio goes waits for the answer.
    const audioBusy = confidentialAudio.busy;
    const deferral = audioKnown ? confidentialAudioDeferralLine(active, attestation, copy.confidential) : '';
    setText('confidentialSetupHeading', beats.heading || '');
    setText('confidentialSetupSub', beats.sub || '');
    setText('confidentialTrustHeading', beats.heading || '');
    setText('confidentialTrustSub', beats.sub || '');
    setText('confidentialTrustEgress', audioKnown ? confidentialEgressLine(active, beats) : '');
    setHidden('confidentialAudioRow', audio.hidden || !(audioKnown || audioBusy));
    const audioToggle = $('confidentialAudioToggle');
    if (audioToggle) {
      audioToggle.checked = audioBusy ? confidentialAudio.choice : audioKnown && audio.on;
      writeDisabled(audioToggle, !audioKnown && !audioBusy);
      if (SolFocus.isBusy(audioToggle) !== audioBusy) SolFocus.setBusy(audioToggle, audioBusy);
    }
    setText('confidentialAudioLabel', audio.label);
    setText('confidentialAudioDescription', audioBusy ? '' : audio.description);
    setText('confidentialAudioNote', audio.note);
    setText('confidentialAudioDeferral', deferral);
    setHidden('confidentialAudioDeferral', !deferral);
    setText('confidentialTrustClaims', beats.claims || '');
    setText('confidentialTrustFailClosed', beats.attestation || '');
    setText('confidentialTrustSubstrate', beats.substrate || '');
    setText('confidentialTrustEarlyAccess', copy.confidential?.lane_detail?.early_access || '');
    const showEarlyAccess = confidentialOffered(attestation) && activeBrain().kind !== 'confidential';
    setHidden('confidentialTrustEarlyAccess', !showEarlyAccess);
  }

  function renderConfidentialCard() {
    const active = activeLanePayload();
    const lane = laneCopy('confidential');
    const attestation = active.confidential_attestation || {state: 'off'};
    const operation = active.confidential_operation;
    const confidentialCopy = copy.confidential || {};
    const checked = confidentialCheckedLabel(attestation);
    const rendered = confidentialAttestationRender(attestation, confidentialCopy, checked);
    const operationRendered = confidentialOperationRender(operation, confidentialCopy);
    const operationActive = !!operation && !confidentialOperationIsTerminal(operation);

    setCardActive('confidential', activeBrain().kind === 'confidential');
    const card = $('lane-confidential');
    if (card) card.classList.toggle('greyed', !confidentialOffered(attestation));
    if (!confidentialOffered(attestation)) {
      setPill('confidentialLanePill', rendered.pill, rendered.tone);
      setText('confidentialLaneDescription', rendered.message);
      setText('confidentialLaneStatus', active.confidential_provenance_configured ? window.JournalI18n.render('journal.window.thinking.status.text.manage') : '');
      setHidden('confidentialLaneStatus', !active.confidential_provenance_configured);
      return;
    }
    setHidden('confidentialLaneStatus', false);
    // The service refusing this sign-in outranks a passing hardware check: the card says
    // access ended, and the way back is one tap in (manage).
    const accessEnded = !operationActive
      && activeBrain().kind === 'confidential'
      && (state.access?.state === 'ended' || state.access?.state === 'credential_unknown');
    const accessLine = state.access?.state === 'ended'
      ? confidentialCopy.access_ended || ''
      : confidentialCopy.credential_unknown || '';
    setPill(
      'confidentialLanePill',
      operationActive ? operation.phase || '' : accessEnded ? window.JournalI18n.render('journal.window.thinking.window.state.text.not.active') : rendered.pill,
      operationActive ? '' : accessEnded ? 'bad' : rendered.tone,
    );
    setText(
      'confidentialLaneDescription',
      accessEnded ? accessLine : operationRendered.message || rendered.message || lane.description || '',
    );
    setText(
      'confidentialLaneStatus',
      operationActive
        ? window.JournalI18n.render('journal.window.thinking.status.text.continue.in.browser')
        : attestation.state === 'off'
          ? confidentialCopy.actions?.off || ''
          : rendered.recheck
            ? confidentialCopy.actions?.recheck || ''
            : window.JournalI18n.render('journal.window.thinking.status.text.manage'),
    );
  }

  function renderConfidentialSetup() {
    renderConfidentialTrust();
    const active = activeLanePayload();
    const attestation = active.confidential_attestation || {state: 'off'};
    const operation = active.confidential_operation;
    const configured = !!active.confidential_provenance_configured;
    const confidentialCopy = copy.confidential || {};
    const checked = confidentialCheckedLabel(attestation);
    const rendered = confidentialAttestationRender(attestation, confidentialCopy, checked);
    const lines = confidentialSetupOperationLines(operation, confidentialCopy, rendered.message);
    const operationActive = !!operation && !confidentialOperationIsTerminal(operation);
    const phase = operation?.phase || '';

    setText('confidentialSetupTitle', activeLaneLabel('confidential'));
    setPill(
      'confidentialSetupPill',
      operationActive ? phase : rendered.pill,
      operationActive ? '' : rendered.tone,
    );
    setText('confidentialSetupState', lines.state);
    setText('confidentialSetupMeta', confidentialSetupMetaLine(attestation, checked));

    if (phase === 'subscribing' || phase === 'journal_limit') {
      setLink(
        'confidentialLaneOperationLink',
        operation?.subscribe_url || '',
        confidentialCopy.portal_link || '',
      );
    } else if (activeBrain().kind === 'confidential' && confidentialOperationIsTerminal(operation)) {
      if (state.access?.state === 'ended') {
        lines.operation = confidentialCopy.access_ended || '';
        setLink(
          'confidentialLaneOperationLink',
          state.access.subscribe_url || '',
          confidentialCopy.portal_link || '',
        );
      } else if (state.access?.state === 'credential_unknown') {
        lines.operation = confidentialCopy.credential_unknown || '';
        setLink('confidentialLaneOperationLink', '', '');
      } else {
        setLink('confidentialLaneOperationLink', '', '');
      }
    } else if (phase === 'starting' || phase === 'waiting') {
      setLink(
        'confidentialLaneOperationLink',
        operation?.portal_url || '',
        window.JournalI18n.render('journal.window.thinking.window.state.text.continue.in.browser'),
      );
    } else {
      setLink('confidentialLaneOperationLink', '', '');
    }

    setMessage(
      'confidentialLaneOperation',
      lines.operation,
      lines.operationTone,
    );
    setText('confidentialNotice', lines.notice.text);
    setHidden('confidentialNotice', lines.notice.hidden);
    const offered = confidentialOffered(attestation);
    setButtonState('confidentialEnable', offered && !configured && !operationActive, operationActive);
    setButtonText('confidentialEnable', confidentialCopy.actions?.off || '');
    setButtonState('confidentialDisable', configured, operationActive || !configured);
    setButtonText('confidentialDisable', confidentialCopy.actions?.enabled || '');
    setButtonState('confidentialRecheck', configured && rendered.recheck, operationActive);
    setButtonText('confidentialRecheck', confidentialCopy.actions?.recheck || '');
  }

  function bundledLocalUnavailable() {
    return state.hostCapabilities?.bundled_local_thinking === false;
  }

  function renderMainLanes() {
    return drawn(() => renderMainLanesNow());
  }

  function renderMainLanesNow() {
    const brain = activeBrain();
    const localLane = laneCopy('local');
    const confidentialLane = laneCopy('confidential');
    const byoLane = laneCopy('byo');
    setText('localLaneTitle', laneDisplayLabel(localLane));
    setText('confidentialLaneTitle', laneDisplayLabel(confidentialLane));
    setText('byoLaneTitle', laneDisplayLabel(byoLane));
    const local = localReadiness();
    const localCard = $('lane-local');
    const localActive = brain.kind === 'local';
    const capabilityBlocked = localCapabilityBlocked();
    const capabilityUnknown = localCapabilityRefusedUnknown();
    const gpuBlocked = !capabilityBlocked && localIsGpuBlocked();
    const endpointOverride = localEndpointConfigured();
    const forkHintEl = $('forkHint');
    if (brain.kind !== 'none') {
      setText('forkHint', window.JournalI18n.render('journal.window.thinking.status.text.one.at.a.time.the.one'));
      setElementData(forkHintEl, 'forkHint', 'data-fork-hint', 'active');
    } else if (capabilityBlocked || capabilityUnknown || gpuBlocked) {
      setText('forkHint', window.JournalI18n.render('journal.window.thinking.status.text.pick.one.of.these.or.choose'));
      setElementData(forkHintEl, 'forkHint', 'data-fork-hint', 'no-local');
    } else {
      setText('forkHint', window.JournalI18n.render('journal.window.thinking.status.text.pick.one.use.local.when.it'));
      setElementData(forkHintEl, 'forkHint', 'data-fork-hint', 'local-ready');
    }
    // A computer that can't run local thinking gets a plain card saying so,
    // in place of a local option it could never pick.
    setHidden('localCantRun', bundledLocalUnavailable() || !(gpuBlocked && !localActive));
    setCardActive('local', localActive);
    $('localLaneStatus')?.removeAttribute('data-tone');
    if (localCard) {
      writeHidden(localCard, bundledLocalUnavailable() || (gpuBlocked && !localActive));
      localCard.classList.toggle('greyed', endpointOverride);
      writeAttr(localCard, 'aria-disabled', 'false');
    }
    if (capabilityBlocked) {
      setPill('localLanePill', window.JournalI18n.render('journal.window.thinking.copy.unavailable'));
      const desc = $('localLaneDescription');
      if (desc) {
        setLocalReason(desc, state.localCapability?.detail || '');
      }
      setText('localLaneStatus', '');
      if (localCard) {
        removeElementData(localCard, 'openView', 'data-open-view');
      }
    } else if (capabilityUnknown) {
      const copyUnknown = localCopy();
      setPill('localLanePill', copyUnknown.pill);
      const desc = $('localLaneDescription');
      if (desc) {
        clearLocalReason(desc);
        setText('localLaneDescription', copyUnknown.sub || '');
      }
      setText('localLaneStatus', '');
      if (localCard) {
        removeElementData(localCard, 'openView', 'data-open-view');
      }
    } else {
      const desc = $('localLaneDescription');
      if (desc) {
        clearLocalReason(desc);
      }
      if (localCard) {
        setElementData(localCard, 'openView', 'data-open-view', 'local-setup');
      }
      if (localActive) {
        setPill('localLanePill', window.JournalI18n.render('journal.window.thinking.copy.active'), 'hot');
        setText('localLaneDescription', localLane.description || '');
        setText('localLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.manage'));
      } else if (endpointOverride) {
        setPill('localLanePill', window.JournalI18n.render('journal.window.thinking.local.status.endpoint'));
        const managed = state.providers.active_lane?.lane === 'confidential';
        setText('localLaneDescription', managed
          ? window.JournalI18n.render('journal.window.thinking.status.text.not.in.use.while.confidential.processing')
          : window.JournalI18n.render('journal.window.thinking.status.text.not.in.use.while.your.own'));
        setText('localLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.set.up.the.bundled.model'));
      } else if (gpuBlocked) {
        setPill('localLanePill', window.JournalI18n.render('journal.window.thinking.copy.unavailable'), 'bad');
        const desc = $('localLaneDescription');
        drawPart(desc, ['gpu'], () => {
          desc.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.your.journal.s.computer.can.t');
          const link = document.createElement('a');
          link.className = 'textlink';
          link.href = 'https://support.solstone.app/kb/solstone-memory-and-local-models';
          link.target = '_blank';
          link.rel = 'noopener noreferrer';
          link.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.minimum.requirements');
          desc.appendChild(link);
        });
        setText('localLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.not.available'));
      } else if (local.status === 'ready') {
        setPill('localLanePill', window.JournalI18n.render('journal.window.thinking.status.off'));
        setText('localLaneDescription', localLane.description || '');
        setText('localLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.turn.on.local'));
      } else if (installIsInFlight(state.install)) {
        // Mid-download is setup under way; "not set up" with "set up →" would
        // read as if nothing had started.
        setPill('localLanePill', window.JournalI18n.render('journal.window.thinking.status.off'));
        setText('localLaneDescription', window.JournalI18n.render('journal.window.thinking.status.text.downloading.the.local.model'));
        setText('localLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.details'));
      } else {
        setPill('localLanePill', window.JournalI18n.render('journal.window.thinking.status.off'));
        setText('localLaneDescription', localLaneUnreadyLine(local));
        setText('localLaneStatus', localAwaitingConfirmation() ? window.JournalI18n.render('journal.window.thinking.status.text.details') : window.JournalI18n.render('journal.window.thinking.status.text.set.up'));
      }
    }

    renderConfidentialCard();
    renderConfidentialHowItWorks();

    const configured = configuredProviders();
    const activeByo = brain.kind === 'byo';
    const byoProvider = activeByo ? brain.provider : configured[0] || defaultByoProvider();
    setCardActive('byo', activeByo);
    setPill('byoLanePill', activeByo ? window.JournalI18n.render('journal.window.thinking.status.active') : window.JournalI18n.render('journal.window.thinking.status.off'), activeByo ? 'hot' : '');
    setText('byoLaneDescription', byoLane.description || '');
    if (activeByo) {
      if (brain.byoKind === 'endpoint') {
        setText('byoLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.using.endpoint.manage'));
      } else if (brain.byoKind === 'chatgpt' || brain.provider === 'chatgpt') {
        const isConfigured = state.providers.provider_status?.chatgpt?.configured === true;
        setText('byoLaneStatus', isConfigured ? (copy.byo_setup?.chatgpt?.lane_signed_in || '') : (copy.byo_setup?.chatgpt?.lane_signed_out || ''));
      } else {
        setText('byoLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.using.value1.key.manage', [['value1', String(providerLabel(byoProvider))]]));
      }
    } else if (endpointOverride) {
      setText('byoLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.manage.endpoint'));
    } else if (byoProvider === 'chatgpt') {
      const isConfigured = state.providers.provider_status?.chatgpt?.configured === true;
      setText('byoLaneStatus', isConfigured ? (copy.byo_setup?.chatgpt?.lane_signed_in || '') : (copy.byo_setup?.chatgpt?.lane_signed_out || ''));
    } else if (configured.length > 0) {
      setText('byoLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.manage.value1.key', [['value1', String(providerLabel(byoProvider))]]));
    } else {
      setText('byoLaneStatus', window.JournalI18n.render('journal.window.thinking.status.text.add.a.key.or.endpoint'));
    }
  }

  function setSelectedByoProvider(provider) {
    state.selectedByoProvider = provider || defaultByoProvider();
    const select = $('byoProvider');
    if (select) select.value = state.selectedByoProvider;
  }

  function setByoModelResolutionTargets(targets) {
    state.byoModelResolutionTargets = Array.isArray(targets)
      ? targets.filter((target) => typeof target === 'string')
      : [];
  }

  function clearByoModelResolutionTargets() {
    state.byoModelResolutionTargets = [];
  }

  function hasConfidentialPriorResolutionTarget(targets = state.byoModelResolutionTargets) {
    return Array.isArray(targets) && targets.includes('confidential_prior');
  }

  function restoreOnlyModelResolutionActive(targets = state.byoModelResolutionTargets) {
    return hasConfidentialPriorResolutionTarget(targets)
      && state.providers.active_lane?.lane === 'confidential';
  }

  function resetByoDraft() {
    state.byoSelectedModel = '';
  }

  function changeByoProvider(provider) {
    clearByoModelResolutionTargets();
    setSelectedByoProvider(provider);
    resetByoDraft();
    const keyInput = $('byoKeyInput');
    if (keyInput) keyInput.value = '';
    const modelInput = $('byoModelInput');
    if (modelInput) modelInput.value = '';
    const selected = selectedByoProvider();
    const validation = state.keys.key_validation?.[selected];
    const mode = byoEntryMode(selected, validation);
    if (mode === 'model') state.byoSelectedModel = preselectByoModel(selected, state.providers);
    if (selected === 'chatgpt') {
      state.byoMode = 'chatgpt';
      if (state.chatgpt.status?.signed_in && state.chatgpt.models.length === 0) {
        loadChatGptModels().catch(() => {});
      }
    } else {
      state.byoMode = mode;
    }
    renderByo();
    renderMainLanes();
  }

  function renderByoModelPanel(provider, validation, byoText) {
    const providerName = providerLabel(provider);
    const checked = relativeTime(validation?.timestamp) || relativeTime(new Date().toISOString());

    setText('byoKeyCheckstripText', formatCopy(byoText.key_ok_strip || '', {provider: providerName, when: checked}));
    setButtonText('byoCheckAgain', byoText.check_again || '');
    setText('byoModelHeading', byoText.model_heading || '');
    setText('byoModelSub', formatCopy(byoText.model_sub || '', {provider: providerName}));
    renderConfigurationGuidance();
    setText('byoModelInputLabel', byoText.model_label || '');
    setButtonText('byoDifferentKey', byoText.use_different_key || '');

    const modelInput = $('byoModelInput');
    if (modelInput && document.activeElement !== modelInput) {
      modelInput.value = state.byoSelectedModel;
    }
    const saveCopy = restoreOnlyModelResolutionActive()
      ? byoText.model_save_restore
      : byoText.model_save;
    setButtonText('byoModelSave', saveCopy || '');
    writeDisabled($('byoModelSave'), !String(state.byoSelectedModel || '').trim());
  }

  function renderChatGptCard(chatgptText) {
    const card = $('prov-chatgpt');
    if (!card) return;
    const picked = selectedByoProvider() === 'chatgpt';
    card.classList.toggle('active', picked);
    card.classList.toggle('greyed', false);
    setText('chatgptCardTitle', copy.provider_labels?.chatgpt || '');
    const pill = $('prov-chatgpt-pill');
    if (pill) {
      writeText(pill, picked
        ? 'selected'
        : (state.chatgpt.status?.signed_in ? (chatgptText.pill_signed_in || '') : 'pick'));
      pill.classList.toggle('hot', picked);
    }
    setText('chatgptCardBody', chatgptText.card_body || '');
    setText('chatgptCardReminder', chatgptText.reminder || '');
    writeMessage($('chatgptCardStatus'), state.chatgpt.notice?.text || '', state.chatgpt.notice?.tone || '');
    const signedIn = !!state.chatgpt.status?.signed_in;
    setHidden('chatgptCardManage', !signedIn);
    setButtonText('chatgptCardManage', chatgptText.manage || '');
    setHidden('chatgptCardContinue', signedIn);
    setText('chatgptCardContinueLabel', chatgptText.continue || '');
    const startOverKeys = [
      'denied',
      'expired',
      'unfinished',
      'account_mismatch',
      'registration_refused',
      'storage',
      'plan_usage_not_granted',
    ];
    const showStartOver = !signedIn && startOverKeys.includes(state.chatgpt.notice?.key);
    setHidden('chatgptCardStartOver', !showStartOver);
    setButtonText('chatgptCardStartOver', chatgptText.start_over || '');
  }

  function renderChatGptPending(chatgptText) {
    setText('chatgptPendingHeading', chatgptText.pending_heading || '');
    setText('chatgptPendingSub', chatgptText.pending_sub || '');
    setText('chatgptPendingNotice', state.chatgpt.pendingNotice || '');
    setHidden('chatgptPendingNotice', !state.chatgpt.pendingNotice);
    const liveStatusKey = state.chatgpt.liveStatus || 'pending_status';
    setText('chatgptPendingStatus', chatgptText[liveStatusKey] || '');
    setButtonText('chatgptReopen', chatgptText.reopen || '');
    setButtonText('chatgptCancel', chatgptText.cancel || '');
    setText('chatgptFallbackHeading', chatgptText.fallback_heading || '');
    setText('chatgptFallbackExplanation', chatgptText.fallback_explanation || '');
    setText('chatgptAddressLabel', chatgptText.address_label || '');
    setButtonText('chatgptFinish', chatgptText.finish || '');
  }

  function renderChatGptPanel(chatgptText) {
    const email = state.chatgpt.status?.email;
    const stripTemplate = email ? chatgptText.strip : chatgptText.strip_no_email;
    setText(
      'chatgptStrip',
      formatCopy(stripTemplate || '', { email: email || '' }),
    );
    setButtonText('chatgptSignOut', chatgptText.sign_out || '');
    setText('chatgptModelHeading', chatgptText.model_heading || '');
    setText('chatgptModelSub', chatgptText.model_sub || '');

    const choices = $('chatgptModelChoices');
    drawPart(choices, state.chatgpt.models.map((model) => [model.slug, model.display_name || '']), () => {
      choices.replaceChildren();
      for (const model of state.chatgpt.models) {
        const label = document.createElement('label');
        const input = document.createElement('input');
        input.type = 'radio';
        input.name = 'chatgptModel';
        input.value = model.slug;
        input.checked = state.chatgpt.selectedModel === model.slug;
        input.addEventListener('change', () => {
          state.chatgpt.selectedModel = model.slug;
          renderByo();
        });
        const name = document.createElement('span');
        name.textContent = model.display_name || model.slug;
        label.append(input, name);
        if (model.display_name) {
          const slug = document.createElement('span');
          slug.className = 'chatgpt-model-slug';
          slug.textContent = model.slug;
          label.append(slug);
        }
        choices.appendChild(label);
      }
    });

    // The picked model can change without the list changing.
    choices?.querySelectorAll('input[name="chatgptModel"]').forEach((input) => {
      input.checked = state.chatgpt.selectedModel === input.value;
    });

    writeMessage($('chatgptModelStatus'), state.chatgpt.notice?.text || '', state.chatgpt.notice?.tone || '');

    setHidden('chatgptPlanRow', state.providers?.active?.provider !== 'chatgpt');
    setText('chatgptPanelPlanLabel', chatgptText.plan || '');
    setText('chatgptPanelManageUsage', chatgptText.manage_usage_link || '');
    externalLink($('chatgptPanelManageUsage'), 'https://chatgpt.com/settings/usage');
    setText('chatgptPanelReminder', chatgptText.reminder || '');
    setButtonText('chatgptUseModel', chatgptText.model_save || '');
    writeDisabled($('chatgptUseModel'), !state.chatgpt.selectedModel);
    setButtonText('chatgptPanelStartOver', chatgptText.start_over || '');
    setButtonText('chatgptPickOther', chatgptText.pick_other || '');
    setHidden('chatgptPickOther', state.chatgpt.notice?.key !== 'not_eligible');
  }

  // The GPT card connects to OpenAI only. Its body names that and links any
  // other OpenAI-compatible provider to the endpoint form, where it belongs.
  function renderOpenaiCardDescription(byoText) {
    const desc = $('prov-openai-desc');
    const template = byoText.openai_card || '';
    if (!desc || !template.includes('{endpoint}')) return;
    drawPart(desc, [template, byoText.chooser_endpoint || ''], () => {
      const [before, after] = template.split('{endpoint}');
      const link = document.createElement('button');
      link.type = 'button';
      link.className = 'laneinline';
      link.textContent = byoText.chooser_endpoint || '';
      link.addEventListener('click', () => $('byoModeEndpoint')?.click());
      desc.replaceChildren(document.createTextNode(before), link, document.createTextNode(after));
    });
  }

  function renderByo() {
    return drawn(() => renderByoNow());
  }

  function renderByoNow() {
    if (!state.selectedByoProvider) setSelectedByoProvider(defaultByoProvider());
    const byoText = copy.byo_setup || {};
    const chatgptText = byoText.chatgpt || {};
    const provider = selectedByoProvider();
    const validation = state.keys.key_validation?.[provider];
    const configured = !!state.keys.api_keys?.[provider];
    let mode = state.byoMode;
    if (mode === 'model' && !byoModelStepAllowed(provider, validation)) {
      resetByoDraft();
      mode = 'paste';
      state.byoMode = mode;
    }
    const endpointMode = mode === 'endpoint';
    const pickMode = mode === 'pick';
    const pasteMode = mode === 'paste';
    const modelMode = mode === 'model';
    const chatgptMode = mode === 'chatgpt';

    setText('byoSetupTitle', activeLaneLabel('byo'));
    setText('byoIntro', byoText.intro || '');
    setText('byoModeKey', byoText.chooser_key || '');
    setText('byoModeEndpoint', byoText.chooser_endpoint || '');
    setText('byoPickTitle', byoText.key_heading || '');
    setText('byoPickSub', byoText.key_sub || '');
    setText('byoEndpointTitle', byoText.endpoint_heading || '');
    setText('byoEndpointSub', byoText.endpoint_sub || '');
    setText('byoEndpointHonesty', byoText.endpoint_honesty || '');
    setButtonText('byoSaveKey', byoText.paste_cta || '');
    markup().querySelectorAll('[data-byo-key-link]').forEach((link) => {
      writeText(link, byoText.get_key || '');
    });
    const keyModeButton = $('byoModeKey');
    const endpointModeButton = $('byoModeEndpoint');
    if (keyModeButton) {
      keyModeButton.classList.toggle('primary', !endpointMode);
      writeAttr(keyModeButton, 'aria-pressed', endpointMode ? 'false' : 'true');
    }
    if (endpointModeButton) {
      endpointModeButton.classList.toggle('primary', endpointMode);
      writeAttr(endpointModeButton, 'aria-pressed', endpointMode ? 'true' : 'false');
    }

    const showChatGptPending = chatgptMode && state.chatgpt.screen === 'pending';
    const showChatGptPanel = chatgptMode && state.chatgpt.screen === 'panel';
    const showByoGrid = pickMode || (chatgptMode && state.chatgpt.screen === 'card');

    setHidden('byoPickPanel', !(pickMode || endpointMode));
    setHidden('byoProviderGrid', !showByoGrid);
    setHidden('byoPickTitle', !pickMode);
    setHidden('byoPickSub', !pickMode);
    setHidden('byoEndpointPanel', !endpointMode);
    setHidden('byoPastePanel', !pasteMode);
    setHidden('byoModelPanel', !modelMode);
    setHidden('chatgptPending', !showChatGptPending);
    setHidden('chatgptPanel', !showChatGptPanel);

    setText('byoBackLink', (pasteMode || modelMode || chatgptMode) ? window.JournalI18n.render('journal.window.thinking.status.text.pick.a.different.provider') : window.JournalI18n.render('journal.window.thinking.status.text.thinking'));

    markup().querySelectorAll('[data-provider-card]').forEach((card) => {
      const cardProvider = card.dataset.providerCard;
      if (cardProvider === 'chatgpt') return;
      const picked = cardProvider === provider;
      card.classList.toggle('active', picked);
      card.classList.toggle('greyed', false);
      const pill = $(`prov-${cardProvider}-pill`);
      if (pill) {
        writeText(pill, picked ? 'selected' : (state.keys.api_keys?.[cardProvider] ? 'saved' : 'pick'));
        pill.classList.toggle('hot', picked);
      }
    });

    renderChatGptCard(chatgptText);

    if (showChatGptPending) {
      renderChatGptPending(chatgptText);
    }
    if (showChatGptPanel) {
      renderChatGptPanel(chatgptText);
    }

    setText('prov-google-desc', window.JournalI18n.render('journal.window.thinking.status.text.use.a.google.ai.studio.key'));
    renderOpenaiCardDescription(byoText);
    if (providerEnv[provider]) {
      setText('byoPasteTitle', formatCopy(byoText.paste_title, {provider: providerLabel(provider)}));
      setText('byoKeyLabel', window.JournalI18n.render('journal.window.thinking.status.text.your.value1.key', [['value1', String(providerLabel(provider))]]));
      setText('byoKeyHint', byoText.key_hint || '');
      const terms = $('byoTermsLine');
      drawPart(terms, [provider, providerLabel(provider), byoText.terms || '', byoText.terms_link || ''], () => {
        terms.textContent = `${formatCopy(byoText.terms, {provider: providerLabel(provider)})} `;
        const link = document.createElement('a');
        link.className = 'textlink';
        link.href = providerTerms[provider] || providerTerms.anthropic;
        link.target = '_blank';
        link.rel = 'noopener noreferrer';
        link.textContent = byoText.terms_link || '';
        terms.appendChild(link);
      });
    }

    writeDisabled($('byoSaveKey'), false);
    writeDisabled($('byoClearKey'), !configured);
    if (pasteMode && validation && validation.valid === false) {
      const reason = byoReasonCopy(validation.reason_code, 'key', byoText, providerLabel(provider));
      setMessage(
        'byoKeyStatus',
        formatCopy(byoText.key_failed || '', {provider: providerLabel(provider), reason}),
        'error',
      );
    } else if (pasteMode && validation && validation.valid === true) {
      setMessage(
        'byoKeyStatus',
        formatCopy(byoText.key_ok_strip || '', {
          provider: providerLabel(provider),
          when: relativeTime(validation.timestamp) || relativeTime(new Date().toISOString()),
        }),
        'ok',
      );
    } else {
      setMessage('byoKeyStatus', pasteMode ? byoText.key_hint || '' : '', '');
    }
    setMessage('byoModelStatus', '', '');
    if (modelMode) {
      renderByoModelPanel(provider, validation, byoText);
    }
    renderByoTuning(byoText, modelMode, endpointMode, showChatGptPanel);
  }

  const BYO_THINKING_BUDGETS = [8192, 16384, 32768];

  function byoThinkingBudget() {
    const budget = Number(state.providers?.byo_thinking_budget);
    return BYO_THINKING_BUDGETS.includes(budget) ? budget : 0;
  }

  function thinkingBudgetLabel(budget) {
    return `${budget / 1024}k`;
  }

  // One thinking setting for the lane. It shows once there is a working key or
  // the endpoint form, and it speaks for the choice in view: a key lane can ask
  // the model to think, while an endpoint only gets room for thinking it does.
  function renderByoTuning(byoText, modelMode, endpointMode, showChatGptPanel = false) {
    const tuning = byoText.tuning || {};
    const chatgptText = byoText.chatgpt || {};
    setHidden('byoTuning', !(modelMode || endpointMode || showChatGptPanel));
    const text = (endpointMode ? tuning.endpoint : tuning.key) || {};
    const budget = byoThinkingBudget();
    setText('byoTuningName', tuning.label || '');
    const current = budget
      ? formatCopy(text.state || '', {budget: thinkingBudgetLabel(budget)})
      : tuning.default || '';
    setText('byoTuningState', `· ${current}`);
    const toggle = $('byoThinkingToggle');
    if (toggle) toggle.checked = budget > 0;
    setText('byoThinkingLabel', text.check || '');
    setHidden('byoThinkingBudget', budget === 0);
    setText('byoThinkingBudgetLegend', text.budget || '');
    markup().querySelectorAll('input[name="byoThinkingBudget"]').forEach((input) => {
      input.checked = Number(input.value) === budget;
    });
    markup().querySelectorAll('[data-thinking-budget-label]').forEach((label) => {
      const value = Number(label.dataset.thinkingBudgetLabel);
      writeText(label, formatCopy(tuning.choice || '', {budget: thinkingBudgetLabel(value)}));
    });
    const noteText = showChatGptPanel ? (chatgptText.tuning_note || '') : (text.note || '');
    setText('byoTuningNote', noteText);
  }

  async function setByoThinkingBudget(budget) {
    const tuning = copy.byo_setup?.tuning || {};
    setMessage('byoTuningStatus', '');
    try {
      await api('/app/settings/api/config', {
        method: 'PUT',
        body: JSON.stringify({
          section: 'providers',
          data: {byo_thinking_budget: budget},
        }),
      });
    } catch (err) {
      // The write never landed, so the saved setting is unchanged.
      setMessage('byoTuningStatus', err.message, 'error');
      renderByo();
      return;
    }
    try {
      await refreshProviders();
    } catch (err) {
      setMessage('byoTuningStatus', err.message, 'error');
      return;
    }
    setMessage('byoTuningStatus', tuning.saved || '', 'ok');
  }

  function openGoogleModelResolutionGuidance(guidance) {
    resetByoDraft();
    setByoModelResolutionTargets(guidance?.[googleModelResolutionTargetsField] || []);
    setSelectedByoProvider('google');
    const validation = state.keys.key_validation?.google;
    if (byoModelStepAllowed('google', validation)) {
      state.byoMode = 'model';
      state.byoSelectedModel = preselectByoModel('google', state.providers);
    } else {
      state.byoMode = 'paste';
    }
    renderByo();
    showView('byo-setup');
  }

  function renderConfigurationGuidance() {
    const guidance = state.providers.configuration_guidance;
    const notice = $('byoConfigurationGuidance');
    if (!notice) return;
    if (!guidance) {
      writeText(notice, '');
      setHidden('byoConfigurationGuidance', true);
      return;
    }
    drawPart(notice, [guidance], () => {
      notice.textContent = '';
      const heading = document.createElement('strong');
      heading.textContent = guidance.heading || '';
      const action = guidance.action || {};
      const link = document.createElement('a');
      link.className = 'textlink';
      link.href = action.href || '#byo-setup';
      link.textContent = action.label || '';
      link.addEventListener('click', (event) => {
        event.preventDefault();
        openGoogleModelResolutionGuidance(guidance);
      });
      notice.append(heading, ' ', link);
    });
    setHidden('byoConfigurationGuidance', false);
  }

  function localCopy() {
    if (bundledLocalUnavailable()) {
      return {pill: window.JournalI18n.render('journal.window.thinking.copy.unavailable'), title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.bundled.local.thinking.isn.t.available'),
        message: '', notice: window.JournalI18n.render('journal.window.thinking.notice.choose.another.thinking.option'),
        activate: false, bootstrap: false, cancel: false, tone: ''};
    }
    if (localCapabilityBlocked()) {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.copy.unavailable'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.your.journal.s.computer.can.t.variant2'),
        message: '',
        notice: state.localCapability?.detail || '',
        activate: false,
        bootstrap: false,
        cancel: false,
        tone: '',
      };
    }
    if (localCapabilityRefusedUnknown()) {
      const text = copy.local_recovery?.states?.local_probe_failed || {};
      return {
        pill: text.pill || window.JournalI18n.render('journal.window.thinking.window.state.text.couldn.t.check'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: text.verdict || window.JournalI18n.render('journal.window.thinking.sub.couldn.t.check.local.setup'),
        message: '',
        notice: text.reason || window.JournalI18n.render('journal.window.thinking.notice.try.checking.again.in.a.moment'),
        activate: false,
        bootstrap: false,
        tone: '',
      };
    }
    if (localEndpointConfigured()) {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.local.status.endpoint'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.you.re.pointed.at.your.own'),
        message: '',
        notice: window.JournalI18n.render('journal.window.thinking.notice.you.re.pointed.at.your.own'),
        activate: false,
        bootstrap: false,
        tone: '',
        endpointOverride: true,
      };
    }
    const inspectionFailed = state.install?.install_state === 'unavailable'
      || ['local_probe_failed', 'gpu_probe_failed', 'availability_unavailable'].includes(state.localAvailability?.reason_code);
    if (inspectionFailed) {
      const text = copy.local_recovery?.states?.local_probe_failed || {};
      return {pill: text.pill || window.JournalI18n.render('journal.window.thinking.window.state.text.couldn.t.check'), title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: text.verdict || window.JournalI18n.render('journal.window.thinking.sub.couldn.t.check.local.setup'), message: '',
        notice: text.reason || window.JournalI18n.render('journal.window.thinking.notice.try.checking.again.in.a.moment'),
        activate: false, bootstrap: false, tone: ''};
    }
    const installOverride = installCopyForStatus(state.install, copy.local_install || {});
    const runtimeOverride = localRuntimeCopy(
      state.providers.local_runtime,
      state.providers.active_lane?.lane === 'local',
      copy.local_recovery || {},
    );
    if (installIsInFlight(state.install)) return installOverride;
    if (
      runtimeOverride
      && state.localAvailability?.available !== false
      && ['ready', 'ready-proof-unavailable'].includes(state.providers.local_runtime?.phase)
    ) {
      return runtimeOverride;
    }
    if (installOverride) return installOverride;
    if (runtimeOverride) return runtimeOverride;
    const local = localReadiness();
    const reason = local.reason;
    // Disposition: ready is the only readiness branch that may offer activation.
    if (local.status === 'ready' || reason === 'ready') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.status.off'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.your.journal.s.computer.can.run'),
        message: '',
        notice: copy.glance?.local?.detail || '',
        activate: true,
        bootstrap: false,
        tone: '',
      };
    }
    // Disposition: gpu_unavailable is honest unavailability — do not offer setup
    // when the required hardware is absent.
    if (reason === 'gpu_unavailable') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.copy.unavailable'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.your.journal.s.computer.can.t'),
        message: '',
        notice: window.JournalI18n.render('journal.window.thinking.notice.your.journal.s.computer.doesn.t', [['value1', String(activeLaneLabel('byo'))]]),
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    // Disposition: gpu_probe_failed is retained for readiness outcomes even though
    // provider issues do not emit it.
    if (reason === 'gpu_probe_failed') {
      const copyProbe = copy.local_recovery?.states?.local_probe_failed;
      return {
        pill: copyProbe?.pill || window.JournalI18n.render('journal.window.thinking.window.state.text.couldn.t.check'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: copyProbe?.verdict || window.JournalI18n.render('journal.window.thinking.sub.couldn.t.check.local.setup'),
        message: '',
        notice: copyProbe?.reason || window.JournalI18n.render('journal.window.thinking.notice.couldn.t.check.local.setup.you', [['value1', String(activeLaneLabel('byo'))]]),
        activate: false,
        bootstrap: false,
        tone: '',
      };
    }
    // The readiness branches below leave the message line empty. Their verdict
    // and notice already say what is wrong, and the readiness summary they used
    // to echo is a server phrase or a bare reason code ("binary_missing").
    // The line carries an install's progress or a refused action, nothing else.
    // Disposition: local_model_installing has no producer in this repository —
    // consumer-only vocabulary, kept inert.
    if (reason === 'local_model_installing') {
      return {
        pill: copy.local_install?.pill_inflight || '',
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.setting.up.a.local.model'),
        message: '',
        notice: copy.local_install?.notice_inflight || '',
        activate: false,
        bootstrap: false,
        tone: '',
      };
    }
    // Disposition: local_model_loading has no producer in this repository —
    // consumer-only vocabulary, kept inert.
    if (reason === 'local_model_loading') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.copy.starting'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.starting.the.local.model'),
        message: '',
        notice: window.JournalI18n.render('journal.window.thinking.notice.local.thinking.will.stay.in.your'),
        activate: false,
        bootstrap: false,
        tone: '',
      };
    }
    // Disposition: bundled setup gaps share one install action across local_model_missing,
    // model_missing, binary_missing, and runtime_missing.
    if (localSetupMissingReasons.has(reason)) {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.window.state.text.setup.needed'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.local.setup.is.not.finished.yet'),
        message: '',
        notice: window.JournalI18n.render('journal.window.thinking.notice.finish.local.setup.before.turning.on'),
        activate: false,
        bootstrap: true,
        bootstrapLabel: copy.local_install?.install || '',
        tone: '',
      };
    }
    // Disposition: local_endpoint_unreachable cannot arrive here; the endpoint
    // override returns before readiness is consulted.
    if (reason === 'local_endpoint_unreachable') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.window.state.text.not.ready'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.your.own.endpoint.didn.t.answer'),
        message: '',
        notice: window.JournalI18n.render('journal.window.thinking.notice.check.the.endpoint.in.value1.then', [['value1', String(activeLaneLabel('byo'))]]),
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    // Disposition: local_server_unhealthy and server_unhealthy share the same
    // unhealthy local-service view.
    if (localServerUnhealthyReasons.has(reason)) {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.window.state.text.not.ready'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.local.thinking.isn.t.ready.yet'),
        message: '',
        notice: window.JournalI18n.render('journal.window.thinking.notice.check.again.after.the.local.service'),
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    // Disposition: ram_insufficient is retained for readiness outcomes; provider
    // issues do not emit it.
    if (reason === 'ram_insufficient') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.copy.unavailable'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.your.journal.s.computer.needs.more'),
        message: '',
        notice: window.JournalI18n.render('journal.window.thinking.notice.you.can.still.use.value1', [['value1', String(activeLaneLabel('byo'))]]),
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    // Disposition: unknown blocked readiness stays bad and visible; missing
    // readiness data stays neutral.
    if (local.status === 'blocked') {
      return {
        pill: window.JournalI18n.render('journal.window.thinking.window.state.text.not.ready'),
        title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
        sub: window.JournalI18n.render('journal.window.thinking.sub.couldn.t.get.local.thinking.ready'),
        message: '',
        notice: window.JournalI18n.render('journal.window.thinking.notice.try.again.or.use.value1', [['value1', String(activeLaneLabel('byo'))]]),
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    return {
      pill: window.JournalI18n.render('journal.window.thinking.copy.checking'),
      title: window.JournalI18n.render('journal.window.thinking.section.heading.local'),
      sub: window.JournalI18n.render('journal.window.thinking.sub.checking.local.readiness'),
      message: '',
      notice: '',
      activate: false,
      bootstrap: false,
      tone: '',
    };
  }

  function renderLocal() {
    return drawn(() => renderLocalNow());
  }

  function renderLocalNow() {
    const unavailable = bundledLocalUnavailable();
    const local = localCopy();
    setHidden('localModelField', unavailable);
    setText('localSetupIntro', unavailable ? '' : window.JournalI18n.render('journal.window.thinking.status.text.run.a.thinking.model.right.in'));
    setPill('localSetupPill', local.pill, local.tone);
    setText('localSetupTitle', local.title);
    setText('localSetupSub', local.sub);
    if (state.localSetupError) {
      setMessage('localSetupMessage', state.localSetupError, state.localSetupTone);
    } else {
      setMessage('localSetupMessage', local.message, local.tone === 'bad' ? 'error' : '');
    }
    let notice = local.notice;
    if (state.install && typeof state.install.admission_warning === 'string' && state.install.admission_warning.length > 0) {
      notice = notice ? `${notice} ${state.install.admission_warning}` : state.install.admission_warning;
    }
    const noticeEl = $('localNotice');
    if (localCapabilityBlocked() && !unavailable) {
      setLocalReason(noticeEl, notice);
    } else {
      clearLocalReason(noticeEl);
      setText('localNotice', notice);
    }
    setText(
      'localOverrideNoticeText',
      state.providers?.active_lane?.lane === 'confidential'
        ? window.JournalI18n.render('journal.window.thinking.status.text.turn.off.confidential.processing.first.then')
        : window.JournalI18n.render('journal.window.thinking.status.text.you.re.pointed.at.your.own'),
    );
    setHidden('localOverrideNotice', !local.endpointOverride);
    setButtonState('localBootstrap', local.bootstrap, !local.bootstrap);
    setButtonText('localBootstrap', local.bootstrapLabel || copy.local_install?.install || '');
    setButtonState('localCancel', !!local.cancel, !local.cancel);
    setButtonText('localCancel', local.cancelLabel || copy.local_install?.cancel || window.JournalI18n.render('journal.window.thinking.button.label.cancel'));
    setButtonState('localRuntimeRetry', local.retryRuntime, !local.retryRuntime);
    setButtonText('localRuntimeRetry', local.retryRuntimeLabel || copy.local_recovery?.retry || '');
    setButtonState('localActivate', local.activate, !local.activate);
    setButtonState('localRefresh', !unavailable, false);
    const links = $('localSetupLinks');
    const showLinks = !unavailable && local.tone === 'bad' && state.install?.install_state !== 'failed' && local.reason !== 'gpu_probe_failed';
    drawPart(links, [showLinks, showLinks ? activeLaneLabel('byo') : ''], () => {
      links.textContent = '';
      if (showLinks) {
        const requirements = document.createElement('a');
        requirements.className = 'textlink';
        requirements.href = 'https://support.solstone.app/kb/solstone-memory-and-local-models';
        requirements.target = '_blank';
        requirements.rel = 'noopener noreferrer';
        requirements.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.minimum.requirements');
        const byo = document.createElement('button');
        byo.type = 'button';
        byo.className = 'textlink';
        byo.dataset.openView = 'byo-setup';
        byo.textContent = activeLaneLabel('byo');
        byo.addEventListener('click', () => showView('byo-setup'));
        links.append(requirements, document.createTextNode(window.JournalI18n.render('journal.window.thinking.window.state.text.or.use')), byo);
      }
    });
  }

  // A field shows the journal's saved value when that value changes. Until
  // then, what the owner has typed in it stays, through any redraw.
  const savedEndpointDrawn = new WeakMap();
  function renderEndpointField(el, saved) {
    if (!el) return;
    if (savedEndpointDrawn.has(el) && savedEndpointDrawn.get(el) === saved) return;
    savedEndpointDrawn.set(el, saved);
    if (el.value !== saved) el.value = saved;
  }

  function renderLocalEndpoint() {
    const endpoint = state.providers.local_override || {};
    renderEndpointField($('localEndpointUrl'), endpoint.endpoint_url || '');
    renderEndpointField($('localEndpointModel'), endpoint.served_model_id || '');
  }

  function renderLocalModels() {
    const select = $('localModelSelect');
    if (!select) return;
    select.innerHTML = '';
    for (const model of state.localModels) {
      const option = document.createElement('option');
      option.value = model.name;
      option.textContent = model.label || model.name;
      select.appendChild(option);
    }
  }

  function byoSetupLabel(brain) {
    if (brain.kind !== 'byo') return activeLaneLabel(brain.kind);
    if (brain.byoKind === 'endpoint') return copy.lane_switch?.setup_endpoint || window.JournalI18n.render('journal.window.thinking.local.status.endpoint');
    return brain.providerLabel || activeLaneLabel('byo');
  }

  function byoSavedSetupLabel(provider) {
    if (provider === 'local') return copy.lane_switch?.setup_endpoint || window.JournalI18n.render('journal.window.thinking.window.state.text.your.endpoint');
    return copy.lane_switch?.setup_key || window.JournalI18n.render('journal.window.thinking.window.state.text.a.saved.key');
  }

  function renderLaneSwitch() {
    return drawn(() => renderLaneSwitchNow());
  }

  function renderLaneSwitchNow() {
    const brain = activeBrain();
    const switchCopy = copy.lane_switch || {};
    const target = state.pendingSwitchTarget || '';
    const targetProvider = target === 'byo' ? selectedByoProvider() : laneProvider(target);
    const currentLabel = brain.kind === 'none' ? activeLaneLabel('none') : activeLaneLabel(brain.kind);
    const targetLabel = target === 'byo'
      ? activeLaneLabel('byo')
      : activeLaneLabel(target);
    setText('switchHeading', switchCopy.heading || '');
    setText('switchCurrentNodeLabel', switchCopy.current_label || '');
    setText('switchTargetNodeLabel', switchCopy.target_label || '');
    setText('switchCurrentLabel', currentLabel);
    setText('switchTargetLabel', targetLabel);
    setMessage('switchStatus', '');
    if (target === 'byo') {
      setText(
        'switchNote',
        formatCopy(switchCopy.to_byo_note, {setup: byoSavedSetupLabel(targetProvider)}),
      );
    } else if (target === 'local') {
      setText(
        'switchNote',
        formatCopy(switchCopy.to_local_note, {current: byoSetupLabel(brain)}),
      );
    } else {
      setText('switchNote', window.JournalI18n.render('journal.window.thinking.status.text.you.can.switch.back.anytime'));
    }
    const primary = $('switchConfirmPrimary');
    if (primary) {
      writeAttr(primary, 'data-switch-lane', target);
      writeText(primary, switchCopy.confirm || '');
    }
    writeText($('switchCancel'), formatCopy(switchCopy.cancel, {current: currentLabel}));
  }

  function renderAll() {
    return drawn(() => renderAllNow());
  }

  function renderAllNow() {
    if (ctx.disposed) return;
    renderGlance();
    renderMainLanes();
    renderByo();
    renderLocalEndpoint();
    renderConfidentialSetup();
    renderLocal();
    renderLaneSwitch();
  }

  async function refreshProvidersPayload() {
    const model = $('localModelSelect')?.value;
    const suffix = model ? `?local_model=${encodeURIComponent(model)}` : '';
    return api(`api/providers${suffix}`);
  }

  async function refreshProviders({confirmAudio = false} = {}) {
    const epoch = confidentialAudio.epoch;
    const canApply = !confidentialAudio.busy || confirmAudio;
    const sequence = canApply ? ++confidentialAudio.readSequence : confidentialAudio.readSequence;
    const providers = await refreshProvidersPayload();
    if (!canApply || epoch !== confidentialAudio.epoch || sequence !== confidentialAudio.readSequence) return false;
    state.providers = providers;
    confidentialAudio.known = true;
    renderAll();
    return true;
  }

  function stopLocalConfirmation() {
    state.localConfirmGeneration += 1;
  }

  // Processing confirms a newly ready local model with a real generate
  // probe, which takes tens of seconds. Follow it for up to five minutes so the
  // card turns active without a reload.
  async function followLocalConfirmation() {
    stopLocalConfirmation();
    const generation = state.localConfirmGeneration;
    for (let attempt = 0; attempt < 100 && localAwaitingConfirmation(); attempt += 1) {
      await sleep(pollIntervalMs * 2);
      if (generation !== state.localConfirmGeneration) return;
      await refreshProviders();
    }
  }

  // A save or a check sends processing to check thinking again, and its result
  // lands seconds to tens of seconds later. Re-read what the journal reports
  // until that check is in, so the card shows it without a reload. A check the
  // owner asked for waits for new evidence; a save waits only while the journal
  // says a check is still due. Nothing here decides readiness.
  function brainCheckOutstanding() {
    const brain = state.providers?.brain || {};
    return brain.state === 'checking' || (brain.state === 'unknown' && brain.action?.refresh === true);
  }

  function stopBrainFollow() {
    state.brainFollowGeneration += 1;
  }

  async function followBrainCheck({newEvidence = false} = {}) {
    stopBrainFollow();
    const generation = state.brainFollowGeneration;
    const before = state.providers?.brain?.evidence?.observed_at ?? null;
    if (!newEvidence && !brainCheckOutstanding()) return;
    for (let attempt = 0; attempt < 40; attempt += 1) {
      await sleep(pollIntervalMs * 2);
      if (generation !== state.brainFollowGeneration) return;
      try {
        await refreshProviders();
      } catch (_) {
        continue;
      }
      const observed = state.providers?.brain?.evidence?.observed_at ?? null;
      if (!brainCheckOutstanding() && (!newEvidence || observed !== before)) return;
    }
  }

  async function refreshKeys() {
    state.keys = await api('api/keys');
    renderAll();
  }

  function openConsentTab(operation) {
    const url = operation?.portal_url;
    // A turn-on that answers after the owner left opens no window over the
    // page they went to. The journal goes on with it, and thinking shows it
    // when they come back.
    if (url && !ctx.disposed) {
      window.solstoneOutside.openDelayed(url);
    }
  }

  async function refreshLocalModels() {
    state.localModels = await api('api/local/models');
    renderLocalModels();
  }

  async function refreshLocalAvailability() {
    const model = $('localModelSelect')?.value || '';
    const suffix = model ? `?model=${encodeURIComponent(model)}` : '';
    try {
      state.localAvailability = await api(`api/local/availability${suffix}`);
    } catch (error) {
      state.localAvailability = {available: false, reason_code: 'availability_unavailable'};
      throw error;
    } finally {
      renderAll();
    }
  }

  function selectedLocalModelId() {
    return $('localModelSelect')?.value || state.localModels[0]?.name || '';
  }

  function stopInstallPoll() {
    state.installPollGeneration += 1;
  }

  function stopRuntimePoll() {
    state.runtimePollGeneration += 1;
  }

  function stopConfidentialPoll() {
    state.confidentialPollGeneration += 1;
  }

  function applyConfidentialProviders(payload, generation) {
    if (generation !== undefined && generation !== state.confidentialPollGeneration) return false;
    state.providers = payload || state.providers;
    renderAll();
    return true;
  }

  function startConfidentialPoll(initialStatus = null) {
    stopConfidentialPoll();
    const generation = state.confidentialPollGeneration;
    return pollConfidentialUntilTerminal({
      fetchStatus: refreshProvidersPayload,
      sleepFn: sleep,
      applyStatus: (payload) => applyConfidentialProviders(payload, generation),
      isCurrent: () => generation === state.confidentialPollGeneration,
      intervalMs: pollIntervalMs,
      maxElapsedMs: confidentialPollMaxMs,
      initialStatus,
    })
      .then((payload) => {
        if (generation !== state.confidentialPollGeneration) return;
        if (payload === null) {
          if (clearConfidentialInProgressOperation(state.providers.active_lane)) {
            renderAll();
          }
          return;
        }
        stopConfidentialPoll();
        return payload;
      })
      .catch((err) => {
        handleConfidentialPollError({
          generation,
          currentGeneration: () => state.confidentialPollGeneration,
          clearOperation: () => {
            if (clearConfidentialInProgressOperation(state.providers.active_lane)) {
              renderAll();
            }
          },
          stopPoll: stopConfidentialPoll,
          showError: (message) => setMessage('confidentialLaneOperation', message, 'error'),
          error: err,
        });
      });
  }

  async function fetchInstallStatus(model = selectedLocalModelId()) {
    if (!model) return null;
    return api(`api/local/bootstrap/status?model=${encodeURIComponent(model)}`);
  }

  async function fetchLocalRuntime() {
    return api('api/local/runtime');
  }

  function applyLocalRuntime(status, generation) {
    if (generation !== undefined && generation !== state.runtimePollGeneration) return false;
    const currentRevision = state.providers.local_runtime?.health_revision;
    const nextRevision = status?.health_revision;
    const currentRetryRevision = state.providers.local_runtime?.retry_revision;
    const nextRetryRevision = status?.retry_revision;
    if (
      (
        Number.isInteger(currentRevision)
        && Number.isInteger(nextRevision)
        && nextRevision < currentRevision
      )
      || (
        Number.isInteger(currentRetryRevision)
        && Number.isInteger(nextRetryRevision)
        && nextRetryRevision < currentRetryRevision
      )
    ) {
      return false;
    }
    state.providers.local_runtime = status || null;
    renderAll();
    return true;
  }

  function startRuntimePoll(initialStatus = state.providers.local_runtime) {
    stopRuntimePoll();
    const generation = state.runtimePollGeneration;
    return pollLocalRuntimeUntilStable({
      fetchStatus: fetchLocalRuntime,
      sleepFn: sleep,
      applyStatus: (status) => applyLocalRuntime(status, generation),
      isCurrent: () => generation === state.runtimePollGeneration,
      intervalMs: pollIntervalMs,
      initialStatus,
    })
      .then(() => {
        if (generation === state.runtimePollGeneration) stopRuntimePoll();
      })
      .catch((err) => {
        if (generation !== state.runtimePollGeneration) return;
        markLocalRuntimeStale();
        showLocalSetupFailure(err);
      });
  }

  function markLocalRuntimeStale() {
    stopRuntimePoll();
    state.providers.local_runtime = {
      status: 'stale',
      phase: 'state-stale',
      reason_code: 'refresh-failed',
      health_revision: null,
      desired_fingerprint_sha256: null,
      retry_revision: null,
      retry_pending: false,
      can_retry: false,
      poll: false,
      updated_at: null,
    };
    renderAll();
  }

  async function refreshLocalRuntime({autoResume = false} = {}) {
    let status;
    try {
      status = await fetchLocalRuntime();
    } catch (err) {
      markLocalRuntimeStale();
      throw err;
    }
    applyLocalRuntime(status);
    if (status?.poll === true && autoResume) {
      startRuntimePoll(status);
    } else {
      stopRuntimePoll();
    }
    return status;
  }

  function applyLocalInstallStatus(status, generation) {
    if (generation !== undefined && generation !== state.installPollGeneration) return false;
    state.install = status || null;
    renderAll();
    return true;
  }

  function startInstallPoll(initialStatus = null, {afterCancel = false} = {}) {
    const model = selectedLocalModelId();
    if (!model) return null;
    stopInstallPoll();
    const generation = state.installPollGeneration;
    return pollLocalInstallUntilTerminal({
      fetchStatus: () => fetchInstallStatus(model),
      sleepFn: sleep,
      applyStatus: (status) => applyLocalInstallStatus(status, generation),
      isCurrent: () => generation === state.installPollGeneration,
      intervalMs: pollIntervalMs,
      initialStatus,
    })
      .then((status) => {
        if (generation !== state.installPollGeneration) return;
        if (installIsTerminal(status)) {
          stopInstallPoll();
          if (afterCancel) {
            clearLocalSetupError();
            renderLocal();
          }
          if (status?.install_state === 'installed') {
            Promise.all([
              refreshProviders(),
              refreshLocalAvailability(),
              refreshLocalRuntime({autoResume: true}),
            ]).catch((err) => {
              showLocalSetupFailure(err);
            });
          }
        }
      })
      .catch((err) => {
        handleInstallPollError({
          generation,
          currentGeneration: () => state.installPollGeneration,
          clearInstallStatus: () => applyLocalInstallStatus(null, generation),
          stopPoll: stopInstallPoll,
          showError: () => showLocalSetupFailure(err),
          error: err,
        });
      });
  }

  async function refreshInstallStatus({autoResume = false} = {}) {
    const generation = state.installPollGeneration;
    const status = await fetchInstallStatus();
    if (!applyLocalInstallStatus(status, generation)) return status;
    if (installIsInFlight(status) && autoResume) {
      startInstallPoll(status);
    } else if (installIsTerminal(status)) {
      stopInstallPoll();
    }
    return status;
  }

  async function switchLane(lane) {
    const payload = {lane};
    if (lane === 'byo') {
      payload.provider = laneProvider('byo');
    }
    state.providers = await api('api/providers', {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
    renderAll();
    followBrainCheck();
  }

  async function activateLane(target) {
    const brain = activeBrain();
    if (brain.kind !== 'none' && brain.kind !== target && laneIsUsable(target)) {
      state.pendingSwitchTarget = target;
      renderLaneSwitch();
      showView('lane-switch');
      return;
    }
    try {
      await switchLane(target);
    } catch (err) {
      throw target === 'local' ? localSetupRefusal(err, 'activate') : err;
    }
    if (target === 'local') {
      await refreshLocalRuntime({autoResume: true});
      showView('local-setup');
      return;
    }
    showView('main');
  }

  async function enableConfidential() {
    // The press is the turn-on's write and its first draw; the poll that
    // follows it is not, so focus moves when the turn-on has started.
    const started = await pressing($('confidentialEnable'), () => [$('confidentialLaneOperationLink')], async () => {
      setMessage('confidentialLaneOperation', '');
      let start;
      try {
        start = await api('api/confidential/enable', {method: 'POST'});
      } catch (err) {
        setMessage('confidentialLaneOperation', err.message, 'error');
        // Busy means a turn-on this page didn't know about is open: re-read it so
        // its "continue in browser" link shows beside the line.
        if (err.reasonCode === 'service_busy') refreshProviders().catch(() => {});
        return false;
      }
      if (state.providers.active_lane) {
        state.providers.active_lane.confidential_operation = start?.operation || null;
        renderAll();
      }
      openConsentTab(start?.operation);
      return true;
    });
    if (!started) return;
    const completed = await startConfidentialPoll();
    if (completed && confidentialEnableNeedsRecheck(completed.active_lane)) {
      try {
        await refreshConfidentialReadiness();
      } catch (err) {
        setMessage('confidentialLaneOperation', err.message, 'error');
      }
    }
  }

  async function recheckConfidential() {
    setMessage('confidentialLaneOperation', '');
    await refreshConfidentialReadiness();
  }

  async function disableConfidential() {
    await api('api/confidential/disable', {method: 'POST'});
    await Promise.all([refreshProviders(), refreshKeys(), refreshLocalAvailability()]);
    showView('main');
  }

  async function setConfidentialAudio(enabled) {
    if (confidentialAudio.busy || !confidentialAudio.known) {
      renderConfidentialTrust();
      return;
    }
    setMessage('confidentialLaneOperation', '');
    const active = activeLanePayload();
    const data = {confidential_audio: enabled};
    if (Object.prototype.hasOwnProperty.call(active, 'speech_backend')) {
      data.backend = enabled ? 'confidential'
        : (['parakeet', 'parakeet-cpp'].includes(active.speech_backend) ? active.speech_backend : 'parakeet');
    }
    confidentialAudio.busy = true;
    confidentialAudio.choice = !!enabled;
    confidentialAudio.known = false;
    confidentialAudio.epoch += 1;
    stopConfidentialPoll();
    renderConfidentialTrust();
    try {
      try {
        await api('/app/settings/api/config', {
          method: 'PUT',
          body: JSON.stringify({
            section: 'transcribe',
            data,
          }),
        });
      } catch (err) {
        // An interrupted answer can follow a successful write. Re-read the choice.
        try {
          await refreshProviders({confirmAudio: true});
        } catch (readError) {
          confidentialAudio.known = false;
        }
        setMessage('confidentialLaneOperation', err.message, 'error');
        return;
      }
      try {
        await refreshProviders({confirmAudio: true});
      } catch (err) {
        confidentialAudio.known = false;
        setMessage('confidentialLaneOperation', err.message, 'error');
      }
    } finally {
      confidentialAudio.busy = false;
      renderConfidentialTrust();
    }
  }

  async function saveByoKey() {
    const provider = laneProvider('byo');
    const value = $('byoKeyInput')?.value || '';
    const result = await runByoKeyCheckFlow({
      apiFn: api,
      applyKeys: (keys) => {
        state.keys = keys;
      },
      provider,
      providerName: providerLabel(provider),
      envVar: providerEnv[provider],
      value,
      text: copy.byo_setup || {},
      providersPayload: state.providers,
      setMode: (mode) => {
        state.byoMode = mode;
      },
      selectModel: (model) => {
        state.byoSelectedModel = model;
      },
      resetDraft: resetByoDraft,
      renderFn: renderByo,
      showStatus: (message, tone) => setMessage('byoKeyStatus', message, tone),
    });
    if (result.status !== 'empty' && $('byoKeyInput')) $('byoKeyInput').value = '';
  }

  async function clearByoKey() {
    const provider = laneProvider('byo');
    const result = await api('api/keys', {
      method: 'PUT',
      body: JSON.stringify({env_var: providerEnv[provider], value: ''}),
    });
    state.keys = result;
    if (state.providers.byo_models) delete state.providers.byo_models[provider];
    resetByoDraft();
    state.byoMode = 'paste';
    if ($('byoKeyInput')) $('byoKeyInput').value = '';
    renderAll();
    await refreshProviders();
  }

  async function recheckByoKey() {
    const provider = laneProvider('byo');
    const byoText = copy.byo_setup || {};
    setMessage('byoModelStatus', formatCopy(byoText.checking_key || '', {provider: providerLabel(provider)}), '');
    const result = await api('api/validate-keys', {method: 'POST'});
    state.keys.key_validation = result.key_validation || {};
    const validation = state.keys.key_validation?.[provider] || {};
    if (byoModelStepAllowed(provider, validation)) {
      if (!state.byoSelectedModel) state.byoSelectedModel = preselectByoModel(provider, state.providers);
      state.byoMode = 'model';
      renderByo();
      return;
    }
    resetByoDraft();
    state.byoMode = 'paste';
    renderByo();
    if (validation.valid === false) {
      const reason = byoReasonCopy(validation.reason_code, 'key', byoText, providerLabel(provider));
      setMessage('byoKeyStatus', formatCopy(byoText.key_failed || '', {provider: providerLabel(provider), reason}), 'error');
    }
  }

  async function saveByoModel() {
    const provider = laneProvider('byo');
    const model = String(state.byoSelectedModel || '').trim();
    if (!model) return;
    const googleModelResolutionTargets = state.byoModelResolutionTargets.slice();
    const result = await runByoModelSaveFlow({
      apiFn: api,
      applyProviders: (providers) => {
        state.providers = providers;
      },
      provider,
      providerName: providerLabel(provider),
      model,
      googleModelResolutionTargets,
      text: copy.byo_setup || {},
      setMode: (mode) => {
        state.byoMode = mode;
      },
      renderFn: renderByo,
      showStatus: (message, tone) => setMessage(state.byoMode === 'paste' ? 'byoKeyStatus' : 'byoModelStatus', message, tone),
    });
    if (result.status === 'saved') {
      clearByoModelResolutionTargets();
      showView('main');
      renderAll();
      followBrainCheck();
    } else if (result.status === 'restored') {
      clearByoModelResolutionTargets();
    }
  }

  async function saveLocalEndpoint() {
    const payload = {
      endpoint_url: $('localEndpointUrl')?.value || '',
      served_model_id: $('localEndpointModel')?.value || '',
    };
    const credential = $('localEndpointCredential')?.value;
    if (credential) payload.credential = credential;
    let result;
    let unlogged = '';
    try {
      result = await api('api/local/endpoint', {
        method: 'POST',
        body: JSON.stringify(payload),
      });
    } catch (err) {
      if (err?.reasonCode !== 'settings_saved_unlogged') throw err;
      // The endpoint was saved (api() has re-read it); finish what saving does
      // and stay here so the owner reads what happened.
      unlogged = err.message;
      result = {local_endpoint: state.providers.local_override};
    }
    state.providers.local_override = result.local_endpoint || {};
    if ($('localEndpointCredential')) $('localEndpointCredential').value = '';
    setSelectedByoProvider('local');
    state.byoMode = 'endpoint';
    const model = (result.local_endpoint || {}).served_model_id || '';
    await switchLane('byo');
    await Promise.all([refreshProviders(), refreshLocalAvailability()]);

    // The save is busy as the press; the two clears can't be used while its
    // check is out, and stay where they are (`disabled` would drop focus).
    const clears = [$('localEndpointClear'), $('localEndpointClearFromLocal')];
    clears.forEach((button) => writeAttr(button, 'aria-disabled', 'true'));
    const checkingMessage = formatCopy(copy.byo_setup?.endpoint_image_checking, {model});
    setMessage('localEndpointStatus', checkingMessage);

    try {
      let checkResult;
      try {
        checkResult = await api('api/local/endpoint/check', {method: 'POST'});
      } catch {
        checkResult = {image_check: {status: 'unchecked', reason: 'unavailable'}};
      }

      if (viewFromHash() !== 'byo-setup') {
        if ($('localEndpointStatus')?.textContent === checkingMessage) {
          setMessage('localEndpointStatus', '');
        }
        return;
      }

      const check =
        checkResult && typeof checkResult === 'object' ? checkResult.image_check : null;
      const status = check?.status;
      const schemaLine =
        checkResult?.schema_check?.status === 'not_followed'
          ? formatCopy(copy.byo_setup?.endpoint_schema_not_followed, {model})
          : '';

      if ((status === 'read' || status === 'not_applicable') && !schemaLine) {
        if (unlogged) {
          setMessage('localEndpointStatus', unlogged, 'error');
        } else {
          setMessage('localEndpointStatus', window.JournalI18n.render('journal.window.thinking.window.state.text.endpoint.saved'), 'ok');
          showView('main');
        }
        return;
      }

      let checkLine = '';
      if (status === 'read' || status === 'not_applicable') {
        checkLine = '';
      } else if (status === 'not_read') {
        checkLine = formatCopy(copy.byo_setup?.endpoint_image_not_read, {model});
      } else {
        let reasonKey = 'endpoint_reason_other';
        const reason = status === 'unchecked' ? check?.reason : undefined;
        if (reason === 'unreachable') reasonKey = 'endpoint_reason_unreachable';
        else if (reason === 'timeout') reasonKey = 'endpoint_reason_timeout';
        else if (reason === 'busy') reasonKey = 'endpoint_reason_busy';
        else if (reason === 'key') reasonKey = 'endpoint_reason_key';
        else if (reason === 'rejected') reasonKey = 'endpoint_reason_rejected';
        else if (reason === 'unusable') reasonKey = 'endpoint_reason_unusable';
        else reasonKey = 'endpoint_reason_other';
        const reasonPhrase = copy.byo_setup?.[reasonKey] || '';
        checkLine = formatCopy(copy.byo_setup?.endpoint_image_unchecked, {
          model,
          reason: reasonPhrase,
        });
      }

      const statusText = [unlogged, schemaLine, checkLine].filter(Boolean).join('\n');
      setMessage('localEndpointStatus', statusText, 'error');
      // The line sits at the foot of the form; on a short window it would be cut off.
      $('localEndpointStatus')?.scrollIntoView?.({block: 'nearest'});
    } finally {
      clears.forEach((button) => writeAttr(button, 'aria-disabled', null));
      if ($('localEndpointStatus')?.textContent === checkingMessage) {
        setMessage('localEndpointStatus', '');
      }
    }
  }

  async function clearLocalEndpoint() {
    let result;
    try {
      result = await api('api/local/endpoint', {method: 'DELETE'});
    } catch (err) {
      throw localSetupRefusal(err, 'clear');
    }
    state.providers.local_override = result.local_endpoint || {};
    if (selectedByoProvider() === 'local') {
      setSelectedByoProvider(defaultByoProvider());
      state.byoMode = 'pick';
    }
    setMessage('localEndpointStatus', window.JournalI18n.render('journal.window.thinking.window.state.text.endpoint.cleared'), 'ok');
    await Promise.all([refreshProviders(), refreshLocalAvailability()]);
  }

  function localStartRefusalKind(err) {
    return err?.status === 409 || err?.reasonCode === 'install_busy' ? 'busy' : 'start';
  }

  async function localInstallAfterStart({startFn, fetchStatus, previousStatus}) {
    try {
      return await startFn();
    } catch (err) {
      // A refusal before installer admission is already durable. Read that
      // attempt so its known cause reaches the same page as an admitted failure.
      const latest = await fetchStatus().catch(() => null);
      if (latest?.install_state === 'failed' && Number.isInteger(latest.revision)
        && latest.revision > (Number.isInteger(previousStatus?.revision) ? previousStatus.revision : 0)) return latest;
      throw localSetupRefusal(err, localStartRefusalKind(err));
    }
  }

  let bootstrapPending = false;
  async function startLocalBootstrap() {
    return pressing($('localBootstrap'), () => [$('localCancel')], startLocalBootstrapNow);
  }

  async function startLocalBootstrapNow() {
    if (bootstrapPending) return;
    bootstrapPending = true;
    clearLocalSetupError();
    stopInstallPoll();
    renderLocal();
    try {
      const model = $('localModelSelect')?.value || '';
      const status = await localInstallAfterStart({
        startFn: () => api(`api/local/bootstrap?model=${encodeURIComponent(model)}`, {method: 'POST'}),
        fetchStatus: () => fetchInstallStatus(model),
        previousStatus: state.install,
      });
      if (status?.refused === true) {
        state.localCapability = {
          overall: 'blocked',
          reason_code: status.reason_code || null,
          detail: typeof status.detail === 'string' ? status.detail : '',
        };
        return;
      }
      state.install = status || null;
      renderAll();
      if (installIsInFlight(status)) {
        startInstallPoll(status);
      } else {
        await refreshInstallStatus({autoResume: true});
      }
      await Promise.all([
        refreshProviders(),
        refreshLocalAvailability(),
        refreshLocalRuntime({autoResume: true}),
      ]);
    } finally {
      bootstrapPending = false;
      renderLocal();
    }
  }

  let cancelPending = false;
  async function localInstallAfterCancel({cancelFn, fetchStatus, previousStatus}) {
    try {
      return {status: await cancelFn(), error: null};
    } catch (err) {
      // A response deadline is not a failed cancellation. The retained worker
      // may still be retiring, or its actual terminal status may now be durable.
      let latest = await fetchStatus().catch(() => null);
      if (!latest || (Number.isInteger(latest.revision) && Number.isInteger(previousStatus?.revision)
        && latest.revision < previousStatus.revision)) latest = previousStatus;
      return {status: latest, error: installIsTerminal(latest) ? null : localSetupRefusal(err, 'cancel')};
    }
  }

  async function cancelLocalBootstrap() {
    return pressing($('localCancel'), () => [$('localBootstrap')], cancelLocalBootstrapNow);
  }

  async function cancelLocalBootstrapNow() {
    if (cancelPending) return;
    const attemptId = state.install?.attempt_id || '';
    if (!attemptId) return;
    const model = selectedLocalModelId();
    cancelPending = true;
    clearLocalSetupError();
    stopInstallPoll();
    renderLocal();
    try {
      const url = `api/local/bootstrap/cancel?attempt_id=${encodeURIComponent(attemptId)}`;
      const {status, error} = await localInstallAfterCancel({
        cancelFn: () => api(url, {method: 'POST'}),
        fetchStatus: () => fetchInstallStatus(model),
        previousStatus: state.install,
      });
      state.install = status || null;
      renderAll();
      if (installIsInFlight(status)) {
        startInstallPoll(status, {afterCancel: true});
      }
      if (error) throw error;
    } finally {
      cancelPending = false;
      renderLocal();
    }
  }

  async function retryLocalRuntime() {
    const runtime = state.providers.local_runtime;
    if (!runtime?.can_retry) return;
    clearLocalSetupError();
    try {
      const status = await api('api/local/runtime/retry', {
        method: 'POST',
        body: JSON.stringify({
          health_revision: runtime.health_revision,
          retry_revision: runtime.retry_revision,
          desired_fingerprint_sha256: runtime.desired_fingerprint_sha256,
        }),
      });
      applyLocalRuntime(status);
      startRuntimePoll(status);
    } catch (_err) {
      await refreshLocalRuntime({autoResume: true});
    }
  }

  async function refreshChatGpt() {
    state.chatgpt.refreshGeneration += 1;
    const generation = state.chatgpt.refreshGeneration;

    const [statusResult, providersResult] = await Promise.allSettled([
      api('api/chatgpt/status'),
      api('api/providers'),
    ]);

    if (generation !== state.chatgpt.refreshGeneration) return;

    if (providersResult.status === 'fulfilled') {
      state.providers = providersResult.value;
    }

    if (statusResult.status === 'rejected') {
      const err = statusResult.reason;
      if (err?.reasonCode === 'storage_error') {
        state.chatgpt.screen = 'card';
        setChatGptNotice('storage', { tone: 'error', owner: false, holdCard: false });
        renderAll();
        return;
      }
      if (state.chatgpt.status && !state.chatgpt.notice?.owner) {
        setChatGptNotice('status_failed', { tone: 'error' });
      }
      renderAll();
      return;
    }

    const status = statusResult.value;
    state.chatgpt.status = status;

    if (state.chatgpt.screen === 'pending') {
      renderAll();
      return;
    }

    if (!state.chatgpt.notice?.owner && status?.plan_usage_declined) {
      state.chatgpt.screen = 'card';
      setChatGptNotice('plan_usage_not_granted', { tone: 'error' });
      renderAll();
      return;
    }
    if (!state.chatgpt.notice?.owner && status?.client_refused) {
      state.chatgpt.screen = 'card';
      setChatGptNotice('registration_refused', { tone: 'error' });
      renderAll();
      return;
    }

    if (status?.signed_in) {
      if (state.chatgpt.notice?.holdCard) {
        state.chatgpt.screen = 'card';
      } else if (state.chatgpt.notice?.key === 'superseded') {
        clearChatGptNotice(true);
        state.chatgpt.screen = 'panel';
      } else {
        state.chatgpt.screen = 'panel';
        if (!state.chatgpt.notice?.owner) {
          clearChatGptNotice(true);
        }
      }
    } else {
      const wasPanel = state.chatgpt.screen === 'panel';
      state.chatgpt.screen = 'card';
      if (wasPanel && !state.chatgpt.notice?.owner) {
        setChatGptNotice('signed_out', { tone: 'error' });
      }
    }
    renderAll();
  }

  async function pollChatGptAttempt(attemptId, token) {
    if (
      token !== state.chatgpt.pollToken ||
      !state.chatgpt.attempt?.id ||
      state.chatgpt.attempt.id !== attemptId ||
      state.chatgpt.screen !== 'pending' ||
      viewFromHash() !== 'byo-setup' ||
      document.visibilityState === 'hidden' ||
      state.chatgpt.pollInFlight != null
    ) {
      return;
    }
    state.chatgpt.pollInFlight = token;
    let resp;
    try {
      resp = await api(`api/chatgpt/sign-in/${encodeURIComponent(attemptId)}`);
    } catch (_err) {
      if (token !== state.chatgpt.pollToken || state.chatgpt.attempt?.id !== attemptId) return;
      state.chatgpt.liveStatus = 'poll_failed';
      renderAll();
      if (
        state.chatgpt.screen === 'pending' &&
        viewFromHash() === 'byo-setup' &&
        document.visibilityState !== 'hidden' &&
        token === state.chatgpt.pollToken &&
        state.chatgpt.attempt?.id === attemptId
      ) {
        state.chatgpt.pollTimer = ctx.setTimeout(() => {
          pollChatGptAttempt(attemptId, token).catch(() => {});
        }, 2000);
      }
      return;
    } finally {
      if (state.chatgpt.pollInFlight === token) {
        state.chatgpt.pollInFlight = null;
      }
    }
    if (token !== state.chatgpt.pollToken || state.chatgpt.attempt?.id !== attemptId) return;

    if (resp?.state === 'pending') {
      state.chatgpt.liveStatus = 'pending_status';
      renderAll();
      if (
        state.chatgpt.screen === 'pending' &&
        viewFromHash() === 'byo-setup' &&
        document.visibilityState !== 'hidden' &&
        token === state.chatgpt.pollToken &&
        state.chatgpt.attempt?.id === attemptId
      ) {
        state.chatgpt.pollTimer = ctx.setTimeout(() => {
          pollChatGptAttempt(attemptId, token).catch(() => {});
        }, 2000);
      }
      return;
    }

    stopChatGptPoll();
    window.sessionStorage.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
    state.chatgpt.attempt = null;
    state.chatgpt.pendingNotice = null;
    state.chatgpt.screen = 'card';

    if (resp?.state === 'signed_in') {
      clearChatGptNotice(true);
      await refreshChatGpt();
      if (state.chatgpt.status?.signed_in) {
        state.chatgpt.screen = 'panel';
        loadChatGptModels().catch(() => {});
      }
      renderAll();
      return;
    }

    if (resp?.state === 'expired') {
      setChatGptNotice('expired', { tone: 'error', holdCard: true });
      await refreshChatGpt();
      return;
    }

    if (resp?.state === 'failed') {
      if (resp.reason === 'cancelled') {
        clearChatGptNotice(true);
        state.chatgpt.screen = 'card';
        await refreshChatGpt();
        return;
      }
      if (resp.reason === 'denied') {
        setChatGptNotice('denied', { tone: 'error', holdCard: true });
        await refreshChatGpt();
        return;
      }
      if (
        resp.reason === 'exchange_failed' ||
        resp.reason === 'callback_invalid' ||
        !resp.reason ||
        ![
          'account_mismatch',
          'registration_refused',
          'plan_usage_not_granted',
          'superseded',
          'busy',
          'storage',
        ].includes(resp.reason)
      ) {
        setChatGptNotice('unfinished', { tone: 'error', holdCard: true });
        await refreshChatGpt();
        return;
      }
      const holdCard = resp.reason !== 'superseded';
      setChatGptNotice(resp.reason, { tone: 'error', holdCard });
      await refreshChatGpt();
      return;
    }

    setChatGptNotice('unfinished', { tone: 'error', holdCard: true });
    await refreshChatGpt();
  }

  async function startChatGptSignIn({ startOver = false } = {}) {
    if (chatGptStartGateOwner) return;
    if (state.chatgpt.activeLaunch) {
      state.chatgpt.activeLaunch.cancelled = true;
      closeChatGptPopup(state.chatgpt.activeLaunch);
    }

    clearChatGptNotice(true);
    state.chatgpt.pendingNotice = null;

    const launch = {
      popup: null,
      cancelled: false,
    };
    chatGptStartGateOwner = launch;
    state.chatgpt.activeLaunch = launch;

    const reservation = window.solstoneOutside.reserveBlank();

    if (!reservation || reservation.kind === 'blocked') {
      releaseChatGptGate(launch);
      setChatGptNotice('popup_blocked', { tone: 'error', owner: true });
      renderAll();
      return;
    }

    if (reservation.kind === 'popup') {
      launch.popup = reservation.popup;
    }

    stopChatGptPoll();
    window.sessionStorage.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
    state.chatgpt.attempt = null;

    try {
      if (startOver) {
        try {
          const resp = await api('api/chatgpt/sign-out', {
            method: 'POST',
            body: JSON.stringify({ forget: true }),
          });
          if (!launchCurrent(launch)) { closeChatGptPopup(launch); return; }
          if (resp && resp.revoked === false) {
            state.chatgpt.pendingNotice = copy.byo_setup?.chatgpt?.revoke_unconfirmed || '';
          }
        } catch (err) {
          if (!launchCurrent(launch)) {
            closeChatGptPopup(launch);
            return;
          }
          if (err?.reasonCode !== 'settings_saved_unlogged') {
            closeChatGptPopup(launch);
            setChatGptNotice('sign_out_failed', { tone: 'error' });
            renderAll();
            return;
          }
        }
      }

      const response = await api('api/chatgpt/sign-in', { method: 'POST', body: '{}' });
      if (!launchCurrent(launch)) { closeChatGptPopup(launch); return; }

      const rawUrl = response?.authorize_url;
      const allowed = window.solstoneOutside.allowedUrl(rawUrl);

      if (!allowed) {
        closeChatGptPopup(launch);
        state.chatgpt.screen = 'card';
        setChatGptNotice('sign_in_failed', { tone: 'error' });
        renderAll();
        return;
      }

      const attempt = { id: response.attempt_id, authorizeUrl: rawUrl };
      state.chatgpt.attempt = attempt;
      window.sessionStorage.setItem(CHATGPT_ATTEMPT_STORAGE_KEY, JSON.stringify(attempt));

      const navigated = window.solstoneOutside.navigateReserved(reservation, rawUrl, 'assign');

      if (!navigated) {
        closeChatGptPopup(launch);
        if (launchCurrent(launch)) {
          window.sessionStorage.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
          state.chatgpt.attempt = null;
          state.chatgpt.screen = 'card';
          setChatGptNotice('sign_in_failed', { tone: 'error' });
          renderAll();
        }
        return;
      }

      state.chatgpt.screen = 'pending';
      state.chatgpt.liveStatus = 'pending_status';
      state.byoMode = 'chatgpt';
      setSelectedByoProvider('chatgpt');
      renderAll();
      stopChatGptPoll();
      const token = ++state.chatgpt.pollToken;
      pollChatGptAttempt(attempt.id, token).catch(() => {});
    } catch (err) {
      closeChatGptPopup(launch);
      if (!launchCurrent(launch)) return;
      state.chatgpt.screen = 'card';
      window.sessionStorage.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
      state.chatgpt.attempt = null;
      if (err?.reasonCode === 'busy') {
        setChatGptNotice('busy', { tone: 'error', holdCard: true });
      } else if (err?.reasonCode === 'storage_error') {
        setChatGptNotice('storage', { tone: 'error', holdCard: true });
      } else {
        setChatGptNotice('sign_in_failed', { tone: 'error' });
      }
      renderAll();
    } finally {
      releaseChatGptGate(launch);
    }
  }

  function reopenChatGptTab() {
    const url = state.chatgpt.attempt?.authorizeUrl;
    const allowed = window.solstoneOutside.allowedUrl(url);

    if (!allowed) {
      setChatGptNotice('sign_in_failed', { tone: 'error' });
      renderAll();
      return;
    }

    const reservation = window.solstoneOutside.reserveBlank();

    if (!reservation || reservation.kind === 'blocked') {
      setChatGptNotice('popup_blocked', { tone: 'error', owner: true });
      renderAll();
      return;
    }

    let navigated = false;
    try {
      navigated = window.solstoneOutside.navigateReserved(reservation, url, 'assign');
    } catch (_err) {}
    if (!navigated) {
      closeChatGptPopup({ popup: reservation.popup });
      setChatGptNotice('sign_in_failed', { tone: 'error' });
      renderAll();
    }
  }

  function cancelChatGptSignIn() {
    if (state.chatgpt.activeLaunch) {
      releaseChatGptGate(state.chatgpt.activeLaunch);
      state.chatgpt.activeLaunch.cancelled = true;
      closeChatGptPopup(state.chatgpt.activeLaunch);
    }
    stopChatGptPoll();
    window.sessionStorage.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
    state.chatgpt.attempt = null;
    state.chatgpt.pendingNotice = null;
    state.chatgpt.screen = 'card';
    clearChatGptNotice(true);
    refreshChatGpt().catch(() => {});
    renderAll();
  }

  async function finishChatGptSignIn() {
    const input = $('chatgptRedirectUrl');
    const redirectUrl = String(input?.value || '');
    if (!redirectUrl) return;
    const attemptId = state.chatgpt.attempt?.id;
    const url = attemptId
      ? `api/chatgpt/sign-in/${encodeURIComponent(attemptId)}/finish`
      : 'api/chatgpt/sign-in/finish';
    try {
      const response = await api(url, {
        method: 'POST',
        body: JSON.stringify({ redirect_url: redirectUrl }),
      });
      if (response?.reason === 'signed_in') {
        await refreshChatGpt();
        if (state.chatgpt.status?.signed_in) {
          stopChatGptPoll();
          window.sessionStorage.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
          state.chatgpt.attempt = null;
          state.chatgpt.pendingNotice = null;
          if (input) input.value = '';
          state.chatgpt.screen = 'panel';
          clearChatGptNotice(true);
          loadChatGptModels().catch(() => {});
          renderAll();
        }
        return;
      }
      const leavingReasons = [
        'denied',
        'plan_usage_not_granted',
        'account_mismatch',
        'registration_refused',
        'exchange_failed',
        'superseded',
        'storage',
      ];
      if (response && leavingReasons.includes(response.reason)) {
        stopChatGptPoll();
        window.sessionStorage.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
        state.chatgpt.attempt = null;
        state.chatgpt.pendingNotice = null;
        state.chatgpt.screen = 'card';
        const key = response.reason === 'exchange_failed' ? 'unfinished' : response.reason;
        const holdCard = key !== 'superseded';
        setChatGptNotice(key, { tone: 'error', holdCard });
        await refreshChatGpt();
        return;
      }
      state.chatgpt.liveStatus = 'finish_refused';
      renderAll();
    } catch (_err) {
      state.chatgpt.liveStatus = 'finish_refused';
      renderAll();
    }
  }

  async function signOutChatGpt({ forget = false } = {}) {
    clearChatGptNotice(true);
    stopChatGptPoll();
    window.sessionStorage.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
    state.chatgpt.attempt = null;
    state.chatgpt.pendingNotice = null;
    state.chatgpt.models = [];
    state.chatgpt.selectedModel = '';
    state.chatgpt.screen = 'card';

    let revokedUnconfirmed = false;
    try {
      const resp = await api('api/chatgpt/sign-out', {
        method: 'POST',
        body: JSON.stringify({}),
      });
      if (resp && resp.revoked === false) {
        revokedUnconfirmed = true;
      }
    } catch (err) {
      if (err?.reasonCode !== 'settings_saved_unlogged') {
        setChatGptNotice('sign_out_failed', { tone: 'error' });
      }
    }
    if (revokedUnconfirmed) {
      setChatGptNotice('revoke_unconfirmed', { tone: 'error', owner: true, holdCard: true });
    }
    await refreshChatGpt();
  }

  async function loadChatGptModels() {
    state.chatgpt.modelsLoading = true;
    try {
      const response = await api('api/chatgpt/models');
      state.chatgpt.models = response.models || [];
      const listedSlugs = state.chatgpt.models.map((m) => m.slug);
      const activeIsChatGpt = state.providers?.active?.provider === 'chatgpt' || state.providers?.active_lane?.provider === 'chatgpt';
      const activeModel = state.providers?.active?.model || state.providers?.active_lane?.model || '';
      const rememberedModel = state.providers?.byo_models?.chatgpt || '';

      if (activeIsChatGpt) {
        if (activeModel && listedSlugs.includes(activeModel)) {
          state.chatgpt.selectedModel = activeModel;
        } else {
          state.chatgpt.selectedModel = '';
          if (activeModel) {
            setChatGptNotice('model_not_found', { tone: 'error', owner: true });
          }
        }
      } else if (rememberedModel && listedSlugs.includes(rememberedModel)) {
        state.chatgpt.selectedModel = rememberedModel;
      } else {
        state.chatgpt.selectedModel = '';
      }
    } catch (err) {
      state.chatgpt.models = [];
      state.chatgpt.selectedModel = '';
      await refreshChatGpt();
      const code = err?.reasonCode;
      if (code === 'sign_in_required' || code === 'chatgpt_sign_in_required') {
        if (state.chatgpt.status) {
          state.chatgpt.status.signed_in = false;
        }
        state.chatgpt.screen = 'card';
        setChatGptNotice('signed_out', { tone: 'error' });
      } else if (code === 'not_eligible' || code === 'chatgpt_not_eligible') {
        state.chatgpt.screen = 'panel';
        setChatGptNotice('not_eligible', { tone: 'error', owner: true });
      } else if (!state.chatgpt.notice?.owner) {
        setChatGptNotice('models_failed', { tone: 'error' });
      }
    } finally {
      state.chatgpt.modelsLoading = false;
      renderAll();
    }
  }

  async function saveChatGptModel() {
    const model = state.chatgpt.selectedModel;
    if (!model) {
      setChatGptNotice('model_missing', { tone: 'error', owner: true });
      renderAll();
      return;
    }
    clearChatGptNotice(true);
    state.chatgpt.isSavingModel = true;
    try {
      const providers = await api('api/providers', {
        method: 'PUT',
        body: JSON.stringify({ lane: 'byo', provider: 'chatgpt', model }),
      });
      state.providers = providers;
      await refreshChatGpt();
      renderAll();
      followBrainCheck();
    } catch (err) {
      if (err?.reasonCode === 'settings_saved_unlogged') {
        await refreshChatGpt();
        renderAll();
        return;
      }
      if (err?.reasonCode === 'chatgpt_sign_in_required') {
        state.chatgpt.screen = 'card';
        setChatGptNotice('signed_out', { tone: 'error' });
      } else if (err?.reasonCode === 'chatgpt_not_eligible') {
        setChatGptNotice('not_eligible', { tone: 'error', owner: true });
      } else if (err?.reasonCode === 'model_not_found') {
        setChatGptNotice('model_not_found', { tone: 'error', owner: true });
      } else if (err?.reasonCode === 'model_missing') {
        setChatGptNotice('model_missing', { tone: 'error', owner: true });
      } else {
        setChatGptNotice('save_refused', { tone: 'error', owner: true });
      }
      renderAll();
      await refreshChatGpt();
    } finally {
      state.chatgpt.isSavingModel = false;
    }
  }

  async function resumeChatGptAttempt() {
    const pollToken = state.chatgpt.pollToken;
    const raw = window.sessionStorage?.getItem(CHATGPT_ATTEMPT_STORAGE_KEY);
    if (!raw) return false;
    let attempt = null;
    try {
      attempt = JSON.parse(raw);
    } catch (_) {
      window.sessionStorage?.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
      return false;
    }
    if (!attempt || typeof attempt !== 'object' || !attempt.id) {
      window.sessionStorage?.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
      return false;
    }
    const allowed = attempt.authorizeUrl && window.solstoneOutside.allowedUrl(attempt.authorizeUrl);
    if (!allowed) {
      window.sessionStorage?.removeItem(CHATGPT_ATTEMPT_STORAGE_KEY);
      setChatGptNotice('sign_in_failed', { tone: 'error' });
      renderAll();
      return false;
    }

    try {
      const resp = await api(`api/chatgpt/sign-in/${encodeURIComponent(attempt.id)}`);
      if (state.chatgpt.pollToken !== pollToken
        || window.sessionStorage?.getItem(CHATGPT_ATTEMPT_STORAGE_KEY) !== raw) return false;
      if (resp?.state === 'pending') {
        state.chatgpt.attempt = attempt;
        state.chatgpt.screen = 'pending';
        state.chatgpt.liveStatus = 'pending_status';
        state.byoMode = 'chatgpt';
        setSelectedByoProvider('chatgpt');
        if (state.chatgpt.pollInFlight == null && !state.chatgpt.pollTimer) {
          pollChatGptAttempt(attempt.id, state.chatgpt.pollToken).catch(() => {});
        }
        return true;
      }
    } catch (_) {}
    return false;
  }

  async function openLane(lane) {
    if (lane === 'local' && (localCapabilityBlocked() || localCapabilityRefusedUnknown())) {
      return;
    }
    if (lane === 'confidential') {
      showView('confidential-setup');
      return;
    }
    if (lane === 'local' && laneIsUsable(lane) && activeBrain().kind !== lane) {
      activateLane(lane).catch((err) => setMessage(`${lane}LaneStatus`, err.message, 'error'));
      return;
    }
    if (lane === 'byo') {
      clearByoModelResolutionTargets();
      const provider = defaultByoProvider();
      setSelectedByoProvider(provider);
      resetByoDraft();
      const validation = state.keys.key_validation?.[provider];
      const mode = byoEntryMode(provider, validation);
      if (mode === 'model') state.byoSelectedModel = preselectByoModel(provider, state.providers);
      if (provider === 'chatgpt') {
        state.byoMode = 'chatgpt';
        const pollToken = state.chatgpt.pollToken;
        const resumed = await resumeChatGptAttempt();
        if (state.chatgpt.pollToken !== pollToken) return;
        if (!resumed) {
          if (state.chatgpt.status?.signed_in) {
            state.chatgpt.screen = 'panel';
            if (state.chatgpt.models.length === 0) {
              loadChatGptModels().catch(() => {});
            }
          } else {
            state.chatgpt.screen = 'card';
          }
        }
      } else {
        state.byoMode = mode === 'paste' && configuredProviders().length === 0 && activeBrain().kind !== 'byo' ? 'pick' : mode;
      }
      renderByo();
    }
    showView(`${lane}-setup`);
  }

  function bindOpenView(el) {
    el.addEventListener('click', (event) => {
      event.preventDefault();
      event.stopPropagation();
      const lane = el.dataset.lane || el.closest('[data-lane]')?.dataset.lane;
      if (lane) {
        openLane(lane);
      } else {
        showView(el.dataset.openView || 'main');
      }
    });
    el.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      event.preventDefault();
      const lane = el.dataset.lane || el.closest('[data-lane]')?.dataset.lane;
      if (lane) {
        openLane(lane);
      } else {
        showView(el.dataset.openView || 'main');
      }
    });
  }

  function bind() {
    markup().querySelectorAll('[data-open-view]').forEach(bindOpenView);
    $('byoModeKey')?.addEventListener('click', () => {
      if (selectedByoProvider() === 'local') {
        setSelectedByoProvider(configuredProviders()[0] || 'anthropic');
      }
      resetByoDraft();
      state.byoMode = 'pick';
      renderByo();
      renderMainLanes();
    });
    $('byoModeEndpoint')?.addEventListener('click', () => {
      setSelectedByoProvider('local');
      resetByoDraft();
      state.byoMode = 'endpoint';
      renderByo();
      renderMainLanes();
    });
    $('byoThinkingToggle')?.addEventListener('change', (event) => {
      // A change while its write is out is undone by the redraw at its end.
      // Turning thinking on starts at the smallest budget.
      pressing(event.target, null, () => setByoThinkingBudget(event.target.checked ? BYO_THINKING_BUDGETS[0] : 0));
    });
    markup().querySelectorAll('input[name="byoThinkingBudget"]').forEach((input) => {
      input.addEventListener('change', () => pressing(input, null, () => setByoThinkingBudget(Number(input.value))));
    });
    markup().querySelectorAll('[data-byo-provider]').forEach((button) => {
      button.addEventListener('click', () => {
        changeByoProvider(button.dataset.byoProvider);
      });
    });
    markup().querySelectorAll('[data-switch-lane]').forEach((button) => {
      button.addEventListener('click', () => {
        const lane = button.dataset.switchLane;
        if (!lane) return;
        pressing(button, () => [laneControl(lane)], () => switchLane(lane).then(() => showView('main')))
          .catch((err) => setMessage('switchStatus', err.message, 'error'));
      });
    });
    $('byoProvider')?.addEventListener('change', () => {
      changeByoProvider($('byoProvider')?.value || defaultByoProvider());
    });
    $('byoBackLink')?.addEventListener('click', () => {
      if (state.byoMode === 'paste' || state.byoMode === 'model' || state.byoMode === 'chatgpt') {
        resetByoDraft();
        state.byoMode = 'pick';
        renderByo();
        return;
      }
      showView('main');
    });
    // Each write's press, with where focus goes if its control has gone (the
    // nearest control, then the part's heading, if it doesn't say).
    const pressed = (id, landing, work) => pressing($(id), landing, work);
    $('byoSaveKey')?.addEventListener('click', () => pressed('byoSaveKey', () => [$('byoModelHeading')], saveByoKey).catch((err) => setMessage('byoKeyStatus', err.message, 'error')));
    $('byoClearKey')?.addEventListener('click', () => pressed('byoClearKey', () => [$('byoKeyInput')], clearByoKey).catch((err) => setMessage('byoKeyStatus', err.message, 'error')));
    $('byoCheckAgain')?.addEventListener('click', () => pressed('byoCheckAgain', null, recheckByoKey).catch((err) => setMessage('byoModelStatus', err.message, 'error')));
    $('byoModelInput')?.addEventListener('input', (event) => {
      state.byoSelectedModel = String(event.target.value || '').trim();
      renderByo();
    });
    $('byoModelSave')?.addEventListener('click', () => pressed('byoModelSave', () => [laneControl('byo')], saveByoModel).catch((err) => setMessage('byoModelStatus', err.message, 'error')));
    $('byoDifferentKey')?.addEventListener('click', () => {
      resetByoDraft();
      state.byoMode = 'paste';
      const keyInput = $('byoKeyInput');
      if (keyInput) keyInput.value = '';
      renderByo();
    });
    // The sign-in's panel takes the card's place, and its heading says what's next.
    const signingIn = () => [$('chatgptPendingHeading')];
    $('chatgptCardContinue')?.addEventListener('click', () => pressed('chatgptCardContinue', signingIn, () => startChatGptSignIn({ startOver: false })));
    $('chatgptCardStartOver')?.addEventListener('click', () => pressed('chatgptCardStartOver', signingIn, () => startChatGptSignIn({ startOver: true })));
    $('chatgptCardManage')?.addEventListener('click', () => {
      setSelectedByoProvider('chatgpt');
      state.byoMode = 'chatgpt';
      state.chatgpt.screen = 'panel';
      if (state.chatgpt.models.length === 0) loadChatGptModels().catch(() => {});
      renderByo();
    });
    $('chatgptReopen')?.addEventListener('click', reopenChatGptTab);
    $('chatgptCancel')?.addEventListener('click', cancelChatGptSignIn);
    $('chatgptFinish')?.addEventListener('click', () => pressed('chatgptFinish', () => [$('chatgptModelHeading')], finishChatGptSignIn));
    $('chatgptSignOut')?.addEventListener('click', () => pressed('chatgptSignOut', () => [$('chatgptCardContinue')], () => signOutChatGpt({ forget: false })));
    $('chatgptPanelStartOver')?.addEventListener('click', () => pressed('chatgptPanelStartOver', signingIn, () => startChatGptSignIn({ startOver: true })));
    $('chatgptPickOther')?.addEventListener('click', () => {
      state.byoMode = 'pick';
      renderByo();
    });
    $('chatgptUseModel')?.addEventListener('click', () => pressed('chatgptUseModel', null, saveChatGptModel));
    $('chatgptGlanceContinue')?.addEventListener('click', () => pressed('chatgptGlanceContinue', signingIn, () => {
      showView('byo-setup');
      return startChatGptSignIn({ startOver: false });
    }));
    $('chatgptGlanceStartOver')?.addEventListener('click', () => pressed('chatgptGlanceStartOver', signingIn, () => {
      showView('byo-setup');
      return startChatGptSignIn({ startOver: true });
    }));
    $('chatgptGlancePickOther')?.addEventListener('click', () => {
      showView('byo-setup');
      state.byoMode = 'pick';
      renderByo();
    });
    ctx.on(document, 'visibilitychange', () => {
      if (document.visibilityState === 'hidden') {
        if (state.chatgpt.pollTimer) {
          ctx.clearTimeout(state.chatgpt.pollTimer);
          state.chatgpt.pollTimer = null;
        }
      } else if (document.visibilityState === 'visible') {
        if (!$('thinkingDevicesInvite')?.hidden) refreshDevicesInvite();
        if (
          state.chatgpt.screen === 'pending' &&
          state.chatgpt.attempt?.id &&
          viewFromHash() === 'byo-setup' &&
          state.chatgpt.pollInFlight == null &&
          !state.chatgpt.pollTimer
        ) {
          pollChatGptAttempt(state.chatgpt.attempt.id, state.chatgpt.pollToken).catch(() => {});
        } else {
          refreshChatGpt().catch(() => {});
        }
      }
    });
    $('confidentialEnable')?.addEventListener('click', () => enableConfidential().catch((err) => setMessage('confidentialLaneOperation', err.message, 'error')));
    $('confidentialRecheck')?.addEventListener('click', () => pressed('confidentialRecheck', null, recheckConfidential).catch((err) => setMessage('confidentialLaneOperation', err.message, 'error')));
    $('confidentialDisable')?.addEventListener('click', () => pressed('confidentialDisable', () => [laneControl('confidential')], disableConfidential).catch((err) => setMessage('confidentialLaneOperation', err.message, 'error')));
    $('confidentialAudioToggle')?.addEventListener('change', (event) => setConfidentialAudio(event.target.checked));
    $('localRefresh')?.addEventListener('click', () => {
      clearLocalSetupError();
      stopInstallPoll();
      stopRuntimePoll();
      stopConfidentialPoll();
      Promise.all([
        refreshProviders(),
        refreshLocalAvailability(),
        refreshInstallStatus({autoResume: true}),
        refreshLocalRuntime({autoResume: true}),
        refreshChatGpt(),
      ]).catch((err) => showLocalSetupFailure(err));
    });
    $('localBootstrap')?.addEventListener('click', () => startLocalBootstrap().catch((err) => showLocalSetupFailure(err)));
    $('localCancel')?.addEventListener('click', () => cancelLocalBootstrap().catch((err) => showLocalSetupFailure(err)));
    $('localRuntimeRetry')?.addEventListener('click', () => pressed('localRuntimeRetry', null, retryLocalRuntime).catch((err) => showLocalSetupFailure(err)));
    $('localActivate')?.addEventListener('click', () => pressed('localActivate', null, () => activateLane('local')).catch((err) => showLocalSetupFailure(err)));
    $('localModelSelect')?.addEventListener('change', () => {
      clearLocalSetupError();
      stopInstallPoll();
      stopRuntimePoll();
      stopConfidentialPoll();
      state.install = null;
      renderAll();
      window.SolFocus.keepAfterPick($('localModelSelect'), ctx);
      Promise.all([
        refreshLocalAvailability(),
        refreshProviders(),
        refreshInstallStatus({autoResume: true}),
        refreshLocalRuntime({autoResume: true}),
      ]).catch((err) => showLocalSetupFailure(err));
    });
    $('localEndpointSave')?.addEventListener('click', () => pressed('localEndpointSave', () => [laneControl('byo')], saveLocalEndpoint).catch((err) => setMessage('localEndpointStatus', err.message, 'error')));
    // The chooser's endpoint button is where the owner was once the form goes.
    $('localEndpointClear')?.addEventListener('click', () => pressed('localEndpointClear', () => [$('byoModeEndpoint')], clearLocalEndpoint).catch((err) => setMessage('localEndpointStatus', err.message, 'error')));
    $('localEndpointClearFromLocal')?.addEventListener('click', () => pressed('localEndpointClearFromLocal', null, clearLocalEndpoint).catch((err) => showLocalSetupFailure(err)));
    ctx.on(window, 'hashchange', () => routeThinkingHash('history'));
  }

  // A journal with no paired device has nothing to think about. Point at the
  // network page's devices section, where a device is added. A failed read
  // leaves the line hidden: it is an invitation, not a status.
  async function refreshDevicesInvite() {
    const invite = $('thinkingDevicesInvite');
    if (!invite) return;
    let none = false;
    try {
      const payload = await ctx.read('/app/network/api/clients', {noAuthRedirect: true});
      none = Array.isArray(payload?.clients) && payload.clients.length === 0;
    } catch (_err) {
      none = false;
    }
    const text = copy.devices_invite?.text;
    const link = copy.devices_invite?.link;
    writeHidden(invite, !(none && text && link));
    if (invite.hidden) return;
    setText('thinkingDevicesInviteText', text);
    setText('thinkingDevicesInviteLink', `${link} →`);
  }

  // The reads after the first paint are independent, except that the providers
  // read names the selected local model, so it follows the model list. Each one
  // draws its own part when it answers; the first failure is reported once all
  // have settled.
  async function loadAfterFirstPaint() {
    const loads = [refreshKeys(), refreshChatGpt()];
    if (bundledLocalUnavailable()) {
      loads.push(refreshProviders());
    } else {
      loads.push(refreshLocalModels().then(() => Promise.all([
        refreshInstallStatus({autoResume: true}),
        refreshLocalRuntime({autoResume: viewFromHash() === 'local-setup'}),
        refreshLocalAvailability(),
        refreshProviders(),
      ])));
    }
    const failed = (await Promise.allSettled(loads)).find((result) => result.status === 'rejected');
    if (failed) throw failed.reason;
  }

  // Thoughts per hour: its own section and a line on the setup glance. The
  // journal does the counting (`/app/thinking/api/pace`); this only draws it.
  function attachPace({onSetup, onPace}) {
    const RANGES = ['day', 'week', 'month'];
    const SVG = 'http://www.w3.org/2000/svg';
    const paceState = {data: null, range: 'day', selected: -1, loading: null, failed: false};
    const $ = (id) => markup().querySelector(`#${id}`); // this view's markup only

    const number = (value) => Math.round(value).toLocaleString();
    const plural = (count, one, many) => `${number(count)} ${Math.round(count) === 1 ? one : many}`;
    const percent = (part, whole) => {
      if (!whole) return '0%';
      const value = (part / whole) * 100;
      if (value > 0 && value < 1) return window.JournalI18n.render('journal.window.thinking.window.state.text.under');
      return `${Math.round(value)}%`;
    };
    const dayLabel = (day) => window.JournalFormat.day(day).toLowerCase();
    const hourLabel = (start) => {
      const at = new Date(start * 1000);
      const text = window.JournalClock
        ? window.JournalClock.formatInstant(at, {hour: 'numeric'})
        : at.toLocaleString(undefined, {hour: 'numeric'});
      return text.toLowerCase().replace(/ /g, ' ');
    };
    const whenLabel = (bar) => (bar.start === undefined
      ? dayLabel(bar.day)
      : `${dayLabel(bar.day)}, ${hourLabel(bar.start)}`);

    function view() {
      return paceState.data?.views?.find((candidate) => candidate.key === paceState.range) || null;
    }

    function bars(current) {
      if (!current) return [];
      return current.hour_bars?.length ? current.hour_bars : (current.day_bars || []);
    }

    function setText(id, text) {
      const element = $(id);
      if (element) element.textContent = text;
    }

    function show(id, visible) {
      const element = $(id);
      if (element) element.hidden = !visible;
    }

    async function load() {
      if (paceState.loading) return paceState.loading;
      paceState.loading = (async () => {
        if (!paceState.data) {
          paceState.failed = false;
          render();
        }
        try {
          const data = await ctx.read('/app/thinking/api/pace');
          // Anything but the journal's answer is a failed read, never a drawing.
          if (!Array.isArray(data?.views) || data.views.length !== RANGES.length) throw new Error('pace answer');
          paceState.data = data;
          paceState.failed = false;
        } catch (_err) {
          if (ctx.disposed) return;
          paceState.failed = !paceState.data;
        } finally {
          paceState.loading = null;
        }
        if (!ctx.disposed) {
          render();
          renderGlance();
        }
      })();
      return paceState.loading;
    }

    function renderGlance() {
      const trail = $('thinkingPaceTrail');
      const median = paceState.data?.views?.[0]?.median;
      if (!trail) return;
      trail.hidden = median === null || median === undefined;
      if (!trail.hidden) {
        trail.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.value1.in.a.typical.hour.of', [['value1', String(plural(median, 'thought', 'thoughts'))]]);
      }
    }

    function renderTiles(current) {
      const thinking = current.thinking_hours;
      setText('paceMedian', current.median === null ? '—' : number(current.median));
      setText('paceMedianSub', thinking
        ? window.JournalI18n.render('journal.window.thinking.status.text.in.a.typical.hour.of.thinking', [['value1', String(number(thinking))], ['value2', String(plural(current.hours, 'hour', 'hours'))]])
        : window.JournalI18n.render('journal.window.thinking.status.text.nothing.finished.in.the.last.value1', [['value1', String(plural(current.hours, 'hour', 'hours'))]]));
      const busiest = current.busiest;
      setText('paceBusiest', busiest ? number(busiest.thoughts) : '—');
      setText('paceBusiestSub', busiest ? whenLabel(busiest) : window.JournalI18n.render('journal.window.thinking.status.text.no.busy.hour.yet'));
      setText('paceOlder', current.thoughts ? percent(current.older, current.thoughts) : '—');
      setText('paceOlderSub', window.JournalI18n.render('journal.window.thinking.status.text.of.the.thinking.was.about.older'));
      setText('paceUnfinished', number(current.unfinished));
      const ran = current.thoughts + current.unfinished;
      setText('paceUnfinishedSub', ran ? window.JournalI18n.render('journal.window.thinking.status.text.value1.of.what.your.journal.ran', [['value1', String(percent(current.unfinished, ran))]]) : '');
    }

    function chartTitle(current) {
      if (current.key === 'month') return window.JournalI18n.render('journal.window.thinking.window.explanation.each.day.of.the.last.by');
      if (current.key === 'week') return window.JournalI18n.render('journal.window.thinking.window.explanation.each.hour.of.the.last.days');
      return window.JournalI18n.render('journal.window.thinking.window.explanation.each.hour.of.the.last');
    }

    function readout(bar) {
      if (!bar) return window.JournalI18n.render('journal.window.thinking.window.explanation.point.at.a.bar.or.select');
      const total = bar.recent + bar.older;
      if (bar.start === undefined) {
        if (!bar.thinking_hours) return `${whenLabel(bar)}: nothing finished`;
        return window.JournalI18n.render('journal.window.thinking.window.explanation.value1.value2.in.a.typical.hour', [['value1', String(whenLabel(bar))], ['value2', String(plural(total, 'thought', 'thoughts'))], ['value3', String(plural(bar.thoughts, 'thought', 'thoughts'))], ['value4', String(plural(bar.thinking_hours, 'hour', 'hours'))], ['value5', String(percent(bar.older_thoughts, bar.thoughts))]]);
      }
      if (!total) return `${whenLabel(bar)}: nothing finished`;
      return window.JournalI18n.render('journal.window.thinking.window.explanation.value1.value2.value3.about.older.days', [['value1', String(whenLabel(bar))], ['value2', String(plural(total, 'thought', 'thoughts'))], ['value3', String(number(bar.older))]]);
    }

    // The axis top: four round steps that clear the tallest bar.
    function niceMax(value) {
      const raw = value / 4;
      if (raw <= 1) return 4;
      const power = 10 ** Math.floor(Math.log10(raw));
      const step = [1, 2, 2.5, 5, 10].map((factor) => factor * power)
        .find((candidate) => candidate >= raw && Number.isInteger(candidate)) || 10 * power;
      return step * 4;
    }

    function element(name, attributes = {}, parent = null) {
      const node = document.createElementNS(SVG, name);
      for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
      if (parent) parent.appendChild(node);
      return node;
    }

    function drawChart(current) {
      const holder = $('paceChart');
      if (!holder) return;
      holder.replaceChildren();
      const list = bars(current);
      const width = Math.max(280, holder.clientWidth || 640);
      const height = width < 520 ? 200 : 240;
      const left = 40;
      const narrow = width < 520;
      const right = narrow ? 24 : 92;
      const top = 12;
      const bottom = 28;
      const plotWidth = width - left - right;
      const plotHeight = height - top - bottom;
      const totals = list.map((bar) => bar.recent + bar.older);
      const max = niceMax(Math.max(current.median || 0, ...totals, 1));
      const y = (value) => top + plotHeight - (value / max) * plotHeight;
      const slot = plotWidth / Math.max(list.length, 1);
      const gap = slot >= 6 ? 2 : 0;
      const barWidth = Math.max(1, slot - gap);
      const svg = element('svg', {width, height, viewBox: `0 0 ${width} ${height}`, 'aria-hidden': 'true'}, holder);
      for (let tick = 0; tick <= 4; tick += 1) {
        const value = (max / 4) * tick;
        element('line', {class: 'pace-grid', x1: left, x2: left + plotWidth, y1: y(value), y2: y(value)}, svg);
        const label = element('text', {class: 'pace-axis', x: left - 6, y: y(value) + 4, 'text-anchor': 'end'}, svg);
        label.textContent = number(value);
      }
      list.forEach((bar, index) => {
        const x = left + index * slot + gap / 2;
        const group = element('g', {class: index === paceState.selected ? 'pace-bar selected' : 'pace-bar'}, svg);
        // Recent days sit on the baseline; older days stack above, past a gap.
        const recentTop = y(bar.recent);
        if (bar.recent > 0) {
          element('rect', {class: 'pace-recent', x, y: recentTop, width: barWidth, height: Math.max(1, y(0) - recentTop)}, group);
        }
        if (bar.older > 0) {
          const base = bar.recent > 0 ? recentTop - (gap ? 2 : 0) : y(0);
          const olderTop = Math.min(base - 1, y(bar.recent + bar.older));
          element('rect', {class: 'pace-older', x, y: olderTop, width: barWidth, height: base - olderTop}, group);
        }
      });
      if (current.median !== null && current.median !== undefined) {
        // The line sits on the number the tiles show.
        const typical = Math.round(current.median);
        element('line', {class: 'pace-median', x1: left, x2: left + plotWidth, y1: y(typical), y2: y(typical)}, svg);
        if (right > 30) {
          const label = element('text', {class: 'pace-axis pace-median-label', x: left + plotWidth + 6, y: y(typical) + 4}, svg);
          label.textContent = window.JournalI18n.render('journal.window.thinking.visible.text.typical.value1', [['value1', String(number(typical))]]);
        }
      }
      if (paceState.selected >= 0 && list[paceState.selected]) {
        const x = left + paceState.selected * slot + slot / 2;
        element('line', {class: 'pace-cursor', x1: x, x2: x, y1: top, y2: top + plotHeight}, svg);
      }
      // A handful of time labels along the bottom.
      const labels = current.key === 'month' ? (narrow ? 3 : 5) : (narrow ? 4 : 6);
      const every = Math.max(1, Math.round(list.length / labels));
      let midnights = 0;
      list.forEach((bar, index) => {
        const start = bar.start;
        // A week marks its midnights: every one when wide, every other when narrow.
        const marks = current.key === 'week'
          ? bar.hour === 0 && (!narrow || (midnights++ % 2 === 1))
          : index % every === every - 1;
        if (!marks) return;
        const text = element('text', {class: 'pace-axis', x: left + index * slot + slot / 2, y: height - 8, 'text-anchor': 'middle'}, svg);
        text.textContent = current.key === 'day' && start !== undefined ? hourLabel(start) : dayLabel(bar.day);
      });
      holder.dataset.left = String(left);
      holder.dataset.slot = String(slot);
    }

    function renderTable(current) {
      const body = $('paceTableBody');
      if (!body) return;
      setText('paceTableWhen', current.key === 'month' ? 'day' : 'hour');
      const byDay = current.key === 'month';
      setText('paceTableUnit', byDay ? window.JournalI18n.render('journal.window.thinking.status.text.typical.hour') : 'thoughts');
      setText('paceTableRecent', byDay ? window.JournalI18n.render('journal.window.thinking.status.text.thoughts.that.day') : window.JournalI18n.render('journal.window.thinking.status.text.recent.days'));
      setText('paceTableOlder', byDay ? window.JournalI18n.render('journal.window.thinking.status.text.about.older.days') : window.JournalI18n.render('journal.window.thinking.status.text.older.days'));
      const rows = bars(current).slice().reverse().map((bar) => {
        const row = document.createElement('tr');
        // A day's row gives its typical hour, then the day's own totals.
        const cells = bar.start === undefined
          ? [whenLabel(bar), number(bar.recent + bar.older), number(bar.thoughts), number(bar.older_thoughts)]
          : [whenLabel(bar), number(bar.recent + bar.older), number(bar.recent), number(bar.older)];
        cells.forEach((text, index) => {
          const cell = document.createElement(index === 0 ? 'th' : 'td');
          if (index === 0) cell.scope = 'row';
          cell.textContent = text;
          row.appendChild(cell);
        });
        return row;
      });
      body.replaceChildren(...rows);
    }

    function render() {
      const current = view();
      const empty = Boolean(paceState.data) && paceState.data.views.every((candidate) => candidate.thoughts === 0);
      show('paceLoading', !paceState.data && !paceState.failed);
      show('paceFailed', paceState.failed);
      show('paceEmpty', empty);
      show('paceBody', Boolean(current) && !empty);
      RANGES.forEach((range) => {
        const button = $(`paceRange-${range}`);
        if (button) button.setAttribute('aria-pressed', String(range === paceState.range));
      });
      if (empty) {
        const now = paceState.data.this_hour;
        setText('paceEmptyText', now
          ? window.JournalI18n.render('journal.window.thinking.status.text.your.journal.is.thinking.value1.so', [['value1', String(plural(now, 'thought', 'thoughts'))]])
          : window.JournalI18n.render('journal.window.thinking.status.text.your.journal.hasn.t.finished.any'));
        show('paceEmptySetup', !now);
      }
      if (!current || empty) return;
      renderTiles(current);
      setText('paceChartTitle', chartTitle(current));
      const list = bars(current);
      if (paceState.selected >= list.length) paceState.selected = -1;
      setText('paceReadout', readout(list[paceState.selected]));
      const through = hourLabel(paceState.data.through);
      setText('paceThrough', paceState.data.this_hour
        ? window.JournalI18n.render('journal.window.thinking.status.text.counted.through.value1.this.hour.so', [['value1', String(through)], ['value2', String(plural(paceState.data.this_hour, 'thought', 'thoughts'))]])
        : window.JournalI18n.render('journal.window.thinking.status.text.counted.through.value1', [['value1', String(through)]]));
      drawChart(current);
      renderTable(current);
    }

    function select(index) {
      const list = bars(view());
      paceState.selected = Math.max(-1, Math.min(index, list.length - 1));
      setText('paceReadout', readout(list[paceState.selected]));
      drawChart(view());
    }

    function pointerIndex(event) {
      const holder = $('paceChart');
      const rect = holder.getBoundingClientRect();
      const left = Number(holder.dataset.left || 0);
      const slot = Number(holder.dataset.slot || 1);
      const index = Math.floor((event.clientX - rect.left - left) / slot);
      return index >= 0 && index < bars(view()).length ? index : -1;
    }

    function bind() {
      RANGES.forEach((range) => {
        $(`paceRange-${range}`)?.addEventListener('click', () => {
          paceState.range = range;
          paceState.selected = -1;
          render();
        });
      });
      const holder = $('paceChart');
      holder?.addEventListener('pointermove', (event) => {
        const index = pointerIndex(event);
        if (index !== paceState.selected) select(index);
      });
      // A tap reads a bar and keeps it: touch has no hover to follow.
      holder?.addEventListener('pointerdown', (event) => {
        if (event.pointerType !== 'mouse') select(pointerIndex(event));
      });
      holder?.addEventListener('pointerleave', (event) => {
        if (event.pointerType === 'mouse' && document.activeElement !== holder) select(-1);
      });
      holder?.addEventListener('blur', () => select(-1));
      holder?.addEventListener('keydown', (event) => {
        const list = bars(view());
        if (!list.length) return;
        let next = paceState.selected;
        if (event.key === 'ArrowLeft') next = paceState.selected <= 0 ? 0 : paceState.selected - 1;
        else if (event.key === 'ArrowRight') next = paceState.selected < 0 ? list.length - 1 : Math.min(list.length - 1, paceState.selected + 1);
        else if (event.key === 'Home') next = 0;
        else if (event.key === 'End') next = list.length - 1;
        else return;
        event.preventDefault();
        select(next);
      });
      $('paceRetry')?.addEventListener('click', () => load());
      $('paceEmptySetup')?.addEventListener('click', () => onSetup());
      $('thinkingPaceTrail')?.addEventListener('click', () => onPace());
      ctx.on(window, 'resize', () => {
        if (!$('thinkingPacePanel')?.hidden && view()) drawChart(view());
      });
      ctx.on(document, 'visibilitychange', () => {
        if (document.visibilityState === 'visible') load();
      });
    }

    bind();
    return {
      // The panel was opened: draw what is known, then read again.
      show() {
        render();
        return load();
      },
      // The setup glance's line.
      glance: load,
    };
  }

  async function init() {
    const loaded = await loadInitialState();
    if (!loaded) return;
    bind();
    bindThinkingSectionTabs();
    bindThinkingRuns();
    pace = attachPace({
      onSetup: () => navigateThinkingSection('setup', 'pointer'),
      onPace: () => navigateThinkingSection('pace', 'pointer'),
    });
    setSelectedByoProvider(defaultByoProvider());

    await resumeChatGptAttempt();

    renderAll();
    routeThinkingHash('reload');
    reportThinkingReady('content');
    refreshDevicesInvite();
    if (window.location.hash !== PACE_HASH) pace?.glance().catch(() => {});
    try {
      await loadAfterFirstPaint();
      if (state.chatgpt.status?.signed_in) {
        loadChatGptModels().catch(() => {});
      }
      // A turn-on started before this page loaded is still followed here, so
      // the lane shows how it ends without another reload.
      const confidentialOperation = state.providers.active_lane?.confidential_operation;
      if (confidentialOperation && !confidentialOperationIsTerminal(confidentialOperation)) {
        startConfidentialPoll();
      }
      if (viewFromHash() === 'main') await followLocalConfirmation();
    } catch (err) {
      setMessage('thinkingActiveDetail', err.message, 'error');
    }
  }

  init();
} });
