// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const PULSE_URL = '/app/home/api/pulse';
  const BRIEFING_URL = '/app/home/api/briefing';
  const FIRST_MINUTES_URL = '/app/home/api/first-minutes';
  const AGENTS_STATE_URL = '/app/agents/api/state';
  const SETUP_ACTION_URL = '/init/api/action';
  const SECTION_STATE_KEY = 'pulse-section-state';
  const SECTION_IDS = ['pulse-narrative', 'pulse-today', 'pulse-needs'];
  // Collapsed-by-default is right on a day with nothing in it. On a day with
  // material the two sections that describe the day are the two the owner came
  // for, so they open and they lead. X-03.
  const SECTION_DEFAULTS_QUIET = {
    'pulse-narrative': 'true',
    'pulse-today': 'true',
    'pulse-needs': 'false'
  };
  const SECTION_DEFAULTS_LIVE = {
    'pulse-narrative': 'false',
    'pulse-today': 'false',
    'pulse-needs': 'false'
  };
  const briefingSectionOrder = ['yesterday'];
  const briefingRenderOrder = ['yesterday', 'needs_attention'];
  const briefingSectionLabels = {
    yesterday: 'yesterday',
    needs_attention: 'needs attention'
  };

  let homeInitialized = false;
  let interactionsWired = false;
  let realtimeWired = false;
  let root = null;
  let surface = null;
  let lastPulse = null;
  let lastBackupIssue = null;
  let backupObservedAt = null;
  let pulseRequest = null;
  let vitalsRequest = null;
  let narrativeRequest = null;
  let trailingPulseRefresh = null;
  let pulseReadVersion = 0;
  let pulseAppliedVersion = 0;
  let pulseReadFailed = false;
  let pulseObservedAt = null;
  let heldWorkSnapshot = null;
  let heldWorkCurrent = false;
  let pageLoadedAt = Date.now();
  let queuedWorkAuthoritySeen = false;
  let heldWorkExpiry = null;
  let briefingSections = {};
  const ACTIVITY_PREVIEW = 6;
  let todayActivitiesExpanded = false;

  let firstMinutesPayload = null;
  let firstMinutesClosed = false;
  let setupPayload = null;
  let agentsPayload = null;
  let agentsSettled = false;
  let firstMinutesRequest = null;
  let agentsRequest = null;
  let downloadTimer = null;
  let speechRetryInFlight = false;

  function esc(value) {
    return String(value ?? '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function markdown(raw) {
    if (window.AppServices && typeof window.AppServices.renderMarkdown === 'function') {
      return window.AppServices.renderMarkdown(raw || '');
    }
    return esc(raw || '').replace(/\n/g, '<br>');
  }

  function apiJson(url) {
    if (typeof window.apiJson !== 'function') {
      return Promise.reject(new Error('home: apiJson unavailable'));
    }
    return window.apiJson(url);
  }

  // Initial paint and live updates share the same observation. Event bursts
  // must not queue whole-journal reads while that observation is in flight.
  function readPulse() {
    if (!pulseRequest) {
      pulseRequest = apiJson(PULSE_URL).finally(function () { pulseRequest = null; });
    }
    return pulseRequest;
  }

  function logError(error, context) {
    if (typeof window.logError === 'function') {
      window.logError(error, { context });
    }
  }

  function surfaceLoading(text) {
    if (window.SurfaceState && typeof window.SurfaceState.loading === 'function') {
      return window.SurfaceState.loading({ text });
    }
    return '<div class="surface-state surface-state--loading" role="status" aria-busy="true">'
      + '<div class="surface-state-spinner" aria-hidden="true"></div>'
      + '<span class="surface-state-text" data-role="loading-status">' + esc(text) + '</span>'
      + '</div>';
  }

  function surfaceError(error, heading) {
    if (window.SurfaceState && typeof window.SurfaceState.error === 'function') {
      return window.SurfaceState.error({
        heading,
        desc: window.CONVEY_COPY?.RELOAD_HINT || 'reload to try again.',
        retry: true,
        serverMessage: window.SurfaceState.serverMessageFrom(error),
        detail: error
      });
    }
    return '<div class="surface-state surface-state--error" role="alert">'
      + '<h2 class="surface-state-heading">' + esc(heading) + '</h2>'
      + '<button type="button" class="surface-state-retry">Try again</button>'
      + '</div>';
  }

  function malformedHomeResponse(url, serverMessage) {
    if (typeof window.ApiError === 'function') {
      return new window.ApiError({
        status: 200,
        statusText: 'OK',
        serverMessage,
        url,
        cause: 'parse'
      });
    }
    const error = new Error(serverMessage);
    error.serverMessage = serverMessage;
    error.url = url;
    return error;
  }

  function isPlainObject(value) {
    return value !== null && typeof value === 'object' && !Array.isArray(value);
  }

  function validatePulse(data) {
    if (!isPlainObject(data) || !isPlainObject(data.health_glance)) {
      throw malformedHomeResponse(PULSE_URL, 'Malformed pulse response');
    }
    if (!Object.prototype.hasOwnProperty.call(data, 'narrative_content')) {
      throw malformedHomeResponse(PULSE_URL, 'Malformed pulse response');
    }
    if (!Array.isArray(data.needs_you_items)) {
      throw malformedHomeResponse(PULSE_URL, 'Malformed pulse response');
    }
  }

  function validateBriefing(data) {
    if (!isPlainObject(data) || typeof data.exists !== 'boolean' || typeof data.phase !== 'string') {
      throw malformedHomeResponse(BRIEFING_URL, 'Malformed briefing response');
    }
    if (data.exists && !isPlainObject(data.sections)) {
      throw malformedHomeResponse(BRIEFING_URL, 'Malformed briefing response');
    }
    if (data.needs_deduped && !Array.isArray(data.needs_deduped)) {
      throw malformedHomeResponse(BRIEFING_URL, 'Malformed briefing response');
    }
  }

  function setSurfaceHtml(html) {
    if (!surface) return;
    surface.innerHTML = html;
    keepGettingStartedFirst();
    renderBriefingSections(briefingSections);
  }

  // The first-minutes card leads the page, right under the status line,
  // whichever read finished first and inserted it.
  function keepGettingStartedFirst() {
    if (!surface || typeof surface.querySelector !== 'function') return;
    const card = surface.querySelector('[data-home-surface="getting-started"]');
    const vitals = surface.querySelector('[data-home-surface="vitals"]');
    if (!card || !vitals || vitals.nextElementSibling === card) return;
    if (typeof vitals.after === 'function') vitals.after(card);
  }

  function replaceHomeSurface(name, html, previousNames) {
    if (!surface || typeof surface.querySelector !== 'function') return;
    const current = surface.querySelector('[data-home-surface="' + name + '"]');
    if (current) {
      if (html) {
        current.outerHTML = html;
      } else {
        current.remove();
      }
      keepGettingStartedFirst();
      renderBriefingSections(briefingSections);
      restoreSectionState();
      return;
    }
    if (!html) return;
    let anchor = null;
    previousNames.forEach(function (previousName) {
      const candidate = surface.querySelector('[data-home-surface="' + previousName + '"]');
      if (candidate) anchor = candidate;
    });
    if (anchor && typeof anchor.insertAdjacentHTML === 'function') {
      anchor.insertAdjacentHTML('afterend', html);
    } else if (typeof surface.insertAdjacentHTML === 'function') {
      surface.insertAdjacentHTML('beforeend', html);
    }
    keepGettingStartedFirst();
    renderBriefingSections(briefingSections);
    restoreSectionState();
  }

  function renderRefreshError(containerId, error, heading) {
    const container = document.getElementById(containerId);
    if (!container || !window.SurfaceState || typeof window.SurfaceState.error !== 'function') return;
    clearPulseRefreshError(containerId);
    container.insertAdjacentHTML(
      'afterend',
      '<div class="surface-state-refresh-error">'
      + window.SurfaceState.error({
        heading,
        desc: window.CONVEY_COPY?.RELOAD_HINT || 'reload to try again.',
        serverMessage: window.SurfaceState.serverMessageFrom(error),
        detail: error
      })
      + '</div>'
    );
  }

  function bindSurfaceRetry() {
    if (!surface || typeof surface.querySelector !== 'function') return;
    const retry = surface.querySelector('.surface-state-retry');
    if (!retry) return;
    retry.addEventListener('click', function () {
      loadPulse();
    });
  }

  function loadPulse() {
    if (!surface) return;
    surface.innerHTML = surfaceLoading('loading your day…');
    const read = ++pulseReadVersion;
    return readPulse()
      .then(function (data) {
        if (read < pulseAppliedVersion) return;
        pulseAppliedVersion = read;
        renderPulse(data);
      })
      .catch(function (error) {
        if (read < pulseAppliedVersion) return;
        pulseAppliedVersion = read;
        logError(error, 'home: loadPulse failed');
        surface.innerHTML = surfaceError(error, "Couldn't load pulse");
        bindSurfaceRetry();
      });
  }

  function dayHasMaterial(pulse) {
    if (Number(pulse?.segment_count || 0) > 0) return true;
    return Array.isArray(pulse?.activities) && pulse.activities.length > 0;
  }

  // One list decides both the first paint and where a live refresh reinserts a
  // card that was not on the page before.
  function cardOrder(pulse) {
    return dayHasMaterial(pulse)
      ? ['vitals', 'getting-started', 'narrative', 'today', 'yesterday', 'briefing', 'reflection', 'needs', 'connections']
      : ['vitals', 'getting-started', 'yesterday', 'briefing', 'narrative', 'reflection', 'today', 'needs', 'connections'];
  }

  // The order the page was actually painted with. refreshVitals and
  // refreshNarrative merge fresh payloads into lastPulse, and a realtime
  // observed or status event fires them mid-session, so recomputing the order
  // from the merged pulse would reorder the page under the owner the moment
  // the day's first segment lands. Only a full render re-decides the order.
  let paintedCardOrder = null;

  function activeCardOrder() {
    return paintedCardOrder || cardOrder(lastPulse);
  }

  // The live order is the only one that leads with the narrative, so the same
  // frozen list also says which section defaults the page was painted with.
  function activeDayIsLive() {
    const orderWithoutGettingStarted = activeCardOrder().filter(function (name) {
      return name !== 'getting-started';
    });
    return orderWithoutGettingStarted[1] === 'narrative';
  }

  function predecessors(name) {
    const order = activeCardOrder();
    const index = order.indexOf(name);
    return index > 0 ? order.slice(0, index) : [];
  }

  function renderPulse(data) {
    validatePulse(data);
    lastPulse = data;
    pulseReadFailed = false;
    pulseObservedAt = Date.now();
    observeBackup(data);
    paintedCardOrder = cardOrder(data);
    briefingSections = isPlainObject(data.briefing_sections) ? data.briefing_sections : {};
    const cards = [renderVitalsHtml(data)];
    if (data.home_state === 'welcome') {
      if (isGettingStartedVisible()) {
        cards.push(renderGettingStartedHtml());
      } else {
        cards.push(renderWelcomeHtml());
      }
      const needs = welcomeNeedsYouHtml(data);
      if (needs) cards.push(needs);
      setSurfaceHtml(cards.join(''));
      return;
    }
    paintedCardOrder.slice(1).forEach(function (name) {
      cards.push(CARD_RENDERERS[name](data));
    });
    setSurfaceHtml(cards.join(''));
    placeTodayZoneNote();
    restoreSectionState();
    refreshBriefing();
  }

  function heldWorkIssues() {
    if (!heldWorkSnapshot) return [];
    const queues = heldWorkSnapshot.queues;
    const names = { daily: 'daily processing', activity: 'activity processing',
      segment: 'segment processing', flush: 'finishing activities', weekly: 'weekly reflection',
      indexer: 'search updates' };
    const scopes = heldWorkSnapshot.held.filter(hold => queues[hold.partition] > 0)
      .map(hold => names[hold.partition] || 'queued processing');
    if (heldWorkSnapshot.queue_hold && Object.values(queues).some(count => count > 0)) {
      scopes.unshift('all queued work');
    }
    return scopes.map(scope => ({ text: scope + ' is on hold for a safety check.',
      severity: 'amber', href: '/app/health/#processingHolds' }));
  }

  function observeBackup(pulse) {
    if (pulse.backup?.available !== true || typeof pulse.backup.issue !== 'boolean') return;
    backupObservedAt = Date.now();
    lastBackupIssue = pulse.backup.issue === true
      ? { text: "the backup key couldn't open your backup destination. check backup in health.",
          severity: 'amber', href: '/app/health/#backup' } : null;
  }

  function homeHealthGlance(pulse) {
    const g = Object.assign({}, pulse.health_glance || {});
    const baseIssues = g.issues || [];
    const holds = heldWorkIssues();
    const workCurrent = heldWorkCurrent && heldWorkSnapshot
      && Date.now() - heldWorkSnapshot.observedAt < 30000;
    const backupCurrent = !pulseReadFailed && pulse.backup?.available === true
      && typeof pulse.backup.issue === 'boolean';
    const backupIssues = lastBackupIssue ? [lastBackupIssue] : [];
    const current = (pulseReadFailed ? [] : baseIssues).concat(workCurrent ? holds : [])
      .concat(backupCurrent ? backupIssues : []);
    const retained = issue => Object.assign({}, issue, { text: 'last known: ' + issue.text, severity: 'amber' });
    g.issues = baseIssues.map(issue => pulseReadFailed ? retained(issue) : issue)
      .concat(holds.map(issue => workCurrent ? issue : retained(issue)))
      .concat(backupIssues.map(issue => backupCurrent ? issue : retained(issue)));
    const notes = [];
    const checked = at => at ? ' last check: ' + new Date(at).toLocaleString() + '.' : '';
    if (pulseReadFailed) notes.push({text:'service status is unavailable.' + checked(pulseObservedAt), href:'/app/health/'});
    const workGrace = !queuedWorkAuthoritySeen && Date.now() - pageLoadedAt < 15000;
    if (!workCurrent && !workGrace) notes.push({text:'queued work status is unavailable.' + checked(heldWorkSnapshot?.observedAt), href:'/app/health/#processingHolds'});
    const backupUnavailable = !backupCurrent && (pulse.backup || backupObservedAt);
    if (backupUnavailable) notes.push({text:'backup status is unavailable.' + checked(backupObservedAt), href:'/app/health/#backup'});
    if (current.length) {
      g.verdict = 'attention';
      g.severity = current.some(issue => issue.severity === 'red') ? 'red' : 'amber';
      g.headline = current.length === 1 ? '1 thing needs your attention' : current.length + ' things need your attention';
    } else if (notes.length) {
      g.verdict = 'unavailable';
      g.severity = 'amber';
      g.headline = pulseReadFailed ? 'service status is unavailable right now.'
        : (!workCurrent && !workGrace) ? 'queued work status is unavailable right now.' : 'backup status is unavailable right now.';
      g.last_observation = null;
      g.cta = null;
    }
    return { glance: g, notes };
  }

  function renderVitalsHtml(pulse) {
    const { glance: g, notes } = homeHealthGlance(pulse);
    const severity = g.severity || 'amber'; // 'neutral' is truthy; do not fold it into this fallback
    let html = '<div class="pulse-vitals" id="pulse-vitals" data-home-surface="vitals" role="status" aria-live="polite">'
      + '<div class="pulse-vitals-item"><span class="pulse-vitals-dot ' + esc(severity) + '" aria-hidden="true"></span>'
      + '<span class="pulse-vitals-verdict ' + esc(severity) + '">' + esc(g.headline || '') + '</span></div>';
    if (g.issues?.length) {
      (g.issues || []).forEach(function (issue) {
        html += '<a class="pulse-vitals-chip ' + esc(issue.severity || 'amber') + '" href="' + esc(issue.href || '#') + '">'
          + esc(issue.text || '') + ' →</a>';
      });
    } else if (g.last_observation) {
      html += '<div class="pulse-vitals-sep"></div><div class="pulse-vitals-item">last reached your journal ' + esc(g.last_observation) + '</div>';
    }
    // The way to add a device stays beside any count: a journal with no
    // devices needs it most on its first day.
    if (g.cta) {
      html += '<div class="pulse-vitals-sep"></div><a class="pulse-vitals-item pulse-vitals-cta" href="' + esc(g.cta.href || '#') + '">'
        + esc(g.cta.text || '') + '</a>';
    }
    notes.forEach(note => {
      html += '<a class="pulse-vitals-note" href="' + esc(note.href) + '">' + esc(note.text) + ' →</a>';
    });
    html += '<a class="pulse-vitals-health-link" href="/app/health/#registeredClientsCard">health →</a>';
    // Not yet, and that's fine: a calm row on any verdict, never a chip and
    // never in the count (the day-0 rule).
    const note = isPlainObject(g.note) ? g.note : null;
    if (note && note.text) {
      html += '<a class="pulse-vitals-note" href="' + esc(note.href || '/app/health/') + '">'
        + '<span class="pulse-vitals-ring" aria-hidden="true"></span>'
        + '<span>' + esc(note.text) + ' →</span></a>';
    }
    html += '</div>';
    return html;
  }

  function renderVitals(pulse) {
    validatePulse(pulse);
    const current = document.getElementById('pulse-vitals');
    const active = document.activeElement;
    const focusedHref = current?.contains(active) && active.tagName === 'A' ? active.getAttribute('href') : null;
    const html = renderVitalsHtml(pulse);
    if (current?.outerHTML === html) return;
    replaceHomeSurface('vitals', html, []);
    if (focusedHref) {
      const next = Array.from(document.querySelectorAll('#pulse-vitals a'))
        .find(link => link.getAttribute('href') === focusedHref)
        || document.querySelector('#pulse-vitals .pulse-vitals-health-link');
      next?.focus({ preventScroll: true });
    }
  }

  function renderWelcomeHtml() {
    return '<div class="pulse-welcome" data-home-surface="welcome">'
      + '<h2>welcome to your journal</h2>'
      + '<p>this is where your day comes together: narrative summaries, calendar events, and tasks. as the solstone app takes in what you share with it, and all of it goes into your journal, sections will appear automatically.</p>'
      + '</div>';
  }

  function formatDownloadQuantity(done, total) {
    if (!Number.isFinite(done) || !Number.isFinite(total) || total <= 0) {
      return '';
    }
    const doneGb = (done / 1e9).toFixed(1);
    const totalGb = (total / 1e9).toFixed(1);
    return ' · ' + doneGb + ' GB of ' + totalGb + ' GB';
  }

  function renderDownloadBar(kind, done, total) {
    if (!Number.isFinite(done) || !Number.isFinite(total) || total <= 0) {
      return '';
    }
    return '<progress class="pulse-progress" data-download-bar="' + esc(kind) + '" value="' + esc(done) + '" max="' + esc(total) + '"></progress>';
  }

  function renderSayASentence() {
    const proof = firstMinutesPayload?.proof;
    const state = proof?.state;
    if (!state || state === 'none') {
      return '';
    }

    const time = proof.start_at
      ? (window.JournalFormat ? window.JournalFormat.timestamp(proof.start_at) : proof.start_at)
      : '';
    const escTime = esc(time);

    let innerHtml = '';
    if (state !== 'heard') {
      innerHtml += '<h4>say a sentence out loud</h4>'
        + '<p>it appears here as text.</p>';
    }

    if (state === 'waiting') {
      innerHtml += '<p>waiting for the next segment. this can take a few minutes.</p>';
    } else if (state === 'no_audio') {
      innerHtml += '<p>nothing with sound has reached your journal yet. turn on the microphone in the solstone app to try this.</p>';
    } else if (state === 'transcribing') {
      innerHtml += '<p>the segment from ' + escTime + ' hasn\'t been written out yet.</p>';
    } else if (state === 'nothing_heard') {
      innerHtml += '<p>no speech was found in the segment from ' + escTime + '. say something, and it shows up in the next one.</p>';
    } else if (state === 'failed') {
      innerHtml += '<p>the segment from ' + escTime + ' couldn\'t be written out.</p>'
        + '<a href="' + esc(proof.href || '/app/health/') + '">see health →</a>';
    } else if (state === 'heard') {
      if (proof.fresh === true) {
        innerHtml += '<p>from ' + escTime + ': <q>' + esc(proof.text || '') + '</q></p>'
          + '<a href="' + esc(proof.href || '#') + '">open it →</a>';
      } else {
        innerHtml += '<p>speech from ' + escTime + ' was written out.</p>'
          + '<a href="' + esc(proof.href || '#') + '">open it →</a>';
      }
    }

    return '<div class="pulse-getting-started-step" data-proof-state="' + esc(state) + '">'
      + innerHtml
      + '</div>';
  }

  function agentCheckState() {
    if (!agentsSettled) {
      return { inFlight: true, done: false, showStep: false };
    }
    if (agentsPayload && agentsPayload.failed) {
      return { inFlight: false, done: true, showStep: false };
    }
    if (!agentsPayload || !Array.isArray(agentsPayload.connections)) {
      return { inFlight: false, done: false, showStep: false };
    }
    if (agentsPayload.connections.length > 0) {
      return { inFlight: false, done: true, showStep: false };
    }
    return { inFlight: false, done: false, showStep: true };
  }

  function isGettingStartedVisible() {
    if (!firstMinutesPayload || firstMinutesPayload.window_open !== true) {
      return false;
    }
    const agentState = agentCheckState();
    if (agentState.inFlight) {
      return false;
    }
    const checks = firstMinutesPayload.checks || {};
    const phoneNotDone = checks.phone !== true;
    const thinkingNotDone = checks.thinking !== true;
    const agentNotDone = !agentState.done;
    const heardNotDone = checks.heard === false;
    return phoneNotDone || thinkingNotDone || agentNotDone || heardNotDone;
  }

  function renderDeviceLine(device) {
    const state = device.state || 'other';
    const name = esc(device.name || '');
    let copy = '';
    if (state === 'awaiting_first') {
      copy = 'the solstone app on ' + name + ' is paired. what you share with it arrives in your journal about every 5 minutes.';
    } else if (state === 'receiving' || state === 'quiet') {
      if (device.last_added_label) {
        copy = 'the solstone app on ' + name + ' last added to your journal ' + esc(device.last_added_label) + '.';
      } else {
        copy = 'the solstone app on ' + name + ' is paired.';
      }
    } else {
      copy = 'the solstone app on ' + name + ' is paired.';
    }
    return '<div class="pulse-device-line" data-device-state="' + esc(state) + '">'
      + '<p>' + copy + '</p>'
      + '</div>';
  }

  function renderAgentStepHtml() {
    let commandBlock = '';
    const door = agentsPayload?.local_door;
    if (door && door.enabled === true && door.listening === true && typeof door.address === 'string') {
      const address = door.address;
      const cmd = 'claude mcp add --transport http journal "' + address + '"';
      commandBlock = '<div class="pulse-agent-command" data-agent-command>'
        + '<p>on your journal\'s computer, run:</p>'
        + '<pre><code>' + esc(cmd) + '</code></pre>'
        + '<button type="button" class="pulse-copy-button" data-copy-agent-command data-command="' + esc(cmd) + '">copy</button>'
        + '<p>then type <code>/mcp</code> in Claude Code, choose <code>journal</code> and authenticate.</p>'
        + '</div>';
    }
    return '<div class="pulse-getting-started-step" data-agent-step>'
      + '<h4>connect an agent</h4>'
      + '<p>let Claude Code or another agent read your journal.</p>'
      + commandBlock
      + '<a href="/app/agents/">other agents and pairing codes →</a>'
      + '</div>';
  }

  function renderPhoneStepHtml() {
    return '<div class="pulse-getting-started-step" data-phone-step>'
      + '<h4>add your phone</h4>'
      + '<p>get solstone on your phone at solstone.app/phone, then pair it with your journal.</p>'
      + '<a href="/app/network/#devices">pair a device →</a>'
      + '</div>';
  }

  function renderThinkingStepHtml(step) {
    let title = '';
    let body = '';
    let href = '';
    let linkText = '';
    if (step === 'choose') {
      title = 'choose where the thinking happens';
      body = '<p>your journal keeps everything until you do.</p>';
      href = '/app/thinking/';
      linkText = 'choose →';
    } else if (step === 'cant_run') {
      title = 'your journal\'s computer can\'t run local thinking';
      body = '<p>choose another way when you\'re ready. your journal keeps everything until then.</p>';
      href = '/app/thinking/';
      linkText = 'choose →';
    } else if (step === 'byo') {
      title = 'finish setting up your own model';
      href = '/app/thinking/#byo-setup';
      linkText = 'finish →';
    } else if (step === 'confidential') {
      title = 'finish setting up confidential processing';
      href = '/app/thinking/#confidential-setup';
      linkText = 'finish →';
    } else if (step === 'local') {
      title = 'finish setting up local thinking';
      href = '/app/thinking/#local-setup';
      linkText = 'finish →';
    } else {
      return '';
    }
    return '<div class="pulse-getting-started-step" data-thinking-step="' + esc(step) + '">'
      + '<h4>' + esc(title) + '</h4>'
      + body
      + '<a href="' + esc(href) + '">' + esc(linkText) + '</a>'
      + '</div>';
  }

  function renderGettingStartedHtml() {
    if (!firstMinutesPayload) return '';

    let html = '<div class="pulse-getting-started" id="getting-started" data-home-surface="getting-started">'
      + '<h2>getting started</h2>';

    if (firstMinutesPayload.devices_known !== false && Array.isArray(firstMinutesPayload.devices)) {
      firstMinutesPayload.devices.forEach(function (device) {
        html += renderDeviceLine(device);
      });
    }

    if (setupPayload && setupPayload.downloads) {
      const dl = setupPayload.downloads;
      if (dl.thinking) {
        const state = dl.thinking.state;
        if (state === 'downloading') {
          const qty = formatDownloadQuantity(dl.thinking.bytes_done, dl.thinking.bytes_total);
          const bar = renderDownloadBar('thinking', dl.thinking.bytes_done, dl.thinking.bytes_total);
          html += '<div class="pulse-download" data-download="thinking" data-download-state="downloading">'
            + '<p>downloading the local model' + qty + '</p>'
            + bar
            + '<a href="/app/thinking/#local-setup">cancel on the thinking page →</a>'
            + '</div>';
        } else if ((state === 'error' || state === 'cancelled') && setupPayload?.thinking?.capability !== 'blocked') {
          html += '<div class="pulse-download" data-download="thinking" data-download-state="' + esc(state) + '">'
            + '<p>the local model didn&#39;t finish downloading.</p>'
            + '<a href="/app/thinking/#local-setup">try again on the thinking page →</a>'
            + '</div>';
        }
      }
      if (dl.speech) {
        const state = dl.speech.state;
        if (state === 'downloading') {
          const qty = formatDownloadQuantity(dl.speech.bytes_done, dl.speech.bytes_total);
          const bar = renderDownloadBar('speech', dl.speech.bytes_done, dl.speech.bytes_total);
          html += '<div class="pulse-download" data-download="speech" data-download-state="downloading">'
            + '<p>downloading the speech model' + qty + '</p>'
            + bar
            + '</div>';
        } else if (state === 'error' || state === 'cancelled') {
          // A missing bundled speech package is fixed by repairing the
          // install, not by downloading again: say what the journal says to
          // do, and offer no retry.
          const packageMissing = dl.speech.error?.code === 'speech_package_unavailable';
          const guidance = packageMissing && typeof dl.speech.error.detail === 'string'
            ? '<p data-speech-guidance>' + esc(dl.speech.error.detail) + '</p>' : '';
          // Nothing was downloaded in that case, only a bundle checked, so the
          // download line would be false: the journal's guidance stands alone.
          if (packageMissing) {
            if (guidance) {
              html += '<div class="pulse-download" data-download="speech" data-download-state="' + esc(state) + '">'
                + guidance + '</div>';
            }
          } else {
            // Setup actions answer only on the journal's own computer, so a
            // linked device sees the line without a button it can't use.
            const retry = setupPayload.actions === true
              ? '<button type="button" data-speech-retry' + (speechRetryInFlight ? ' disabled' : '') + '>try again</button>'
              : '';
            html += '<div class="pulse-download" data-download="speech" data-download-state="' + esc(state) + '">'
              + '<p>the speech model didn&#39;t finish downloading.</p>'
              + retry
              + '</div>';
          }
        }
      }
    }

    if (firstMinutesPayload.first_segment) {
      const seg = firstMinutesPayload.first_segment;
      const time = window.JournalFormat ? window.JournalFormat.timestamp(seg.start_at) : seg.start_at;
      let text = '';
      if (seg.thinking === 'read') {
        text = 'your first segment, from ' + esc(time) + ', has arrived, and thinking has read it.';
      } else if (seg.thinking === 'pending') {
        text = 'your first segment, from ' + esc(time) + ', has arrived. thinking hasn\'t caught up with it yet.';
      } else {
        text = 'your first segment, from ' + esc(time) + ', has arrived.';
      }
      html += '<div class="pulse-first-segment" data-segment-thinking="' + esc(seg.thinking || 'none') + '">'
        + '<p>' + text + '</p>'
        + '<a href="' + esc(seg.href || '#') + '">open it →</a>'
        + '</div>';
    }

    html += renderSayASentence();

    const stepsHtml = [];
    const agentState = agentCheckState();
    if (agentState.showStep) {
      stepsHtml.push(renderAgentStepHtml());
    }
    const checks = firstMinutesPayload.checks || {};
    if (checks.phone === false) {
      stepsHtml.push(renderPhoneStepHtml());
    }
    let step = firstMinutesPayload.thinking?.step;
    const capability = setupPayload?.thinking?.capability;
    const thinkingDownloadState = setupPayload?.downloads?.thinking?.state;
    // Setup saves a choice before any lane is active, so "nothing chosen"
    // can still have a saved intention behind it: continue that lane. Once a
    // lane is active the brain decides, so a later change by the owner wins.
    const savedChoice = setupPayload?.thinking?.choice;
    if (step === 'choose' && savedChoice === 'byo') {
      step = 'byo';
    } else if (step === 'choose' && savedChoice === 'spp') {
      step = 'confidential';
    } else if (step === 'choose' && savedChoice === 'local') {
      step = 'local';
    }
    if (capability === 'blocked' && (step === 'choose' || step === 'local')) {
      step = 'cant_run';
    }
    if (step === 'local' && thinkingDownloadState === 'downloading') {
      step = 'none';
    }
    if (step && step !== 'none') {
      const thHtml = renderThinkingStepHtml(step);
      if (thHtml) stepsHtml.push(thHtml);
    }

    if (stepsHtml.length > 0) {
      html += '<h3 data-getting-started-next>next</h3>' + stepsHtml.join('');
    }

    html += '</div>';
    return html;
  }

  function updateGettingStartedSurface() {
    if (!surface) return;
    const isVisible = isGettingStartedVisible();
    const isWelcome = lastPulse && lastPulse.home_state === 'welcome';
    const currentGettingStarted = surface.querySelector('[data-home-surface="getting-started"]');
    const currentWelcome = surface.querySelector('[data-home-surface="welcome"]');

    if (isVisible) {
      const html = renderGettingStartedHtml();
      if (currentGettingStarted) {
        currentGettingStarted.outerHTML = html;
      } else if (currentWelcome) {
        currentWelcome.outerHTML = html;
      } else {
        replaceHomeSurface('getting-started', html, predecessors('getting-started'));
      }
      keepGettingStartedFirst();
    } else {
      if (currentGettingStarted) {
        if (isWelcome) {
          currentGettingStarted.outerHTML = renderWelcomeHtml();
        } else {
          currentGettingStarted.remove();
        }
      }
    }
  }

  function hasActiveDownload() {
    if (!setupPayload || !setupPayload.downloads) return false;
    const dl = setupPayload.downloads;
    return dl.thinking?.state === 'downloading' || dl.speech?.state === 'downloading';
  }

  function scheduleOrClearDownloadTimer() {
    if (firstMinutesClosed) {
      if (downloadTimer) {
        clearTimeout(downloadTimer);
        downloadTimer = null;
      }
      return;
    }
    const proof = firstMinutesPayload?.proof;
    const proofNeedsTimer = proof && (
      proof.state === 'waiting' ||
      proof.state === 'transcribing' ||
      (proof.state === 'heard' && proof.fresh === true)
    );
    if (hasActiveDownload() || proofNeedsTimer) {
      if (!downloadTimer) {
        downloadTimer = setTimeout(function () {
          downloadTimer = null;
          if (!firstMinutesClosed) {
            readFirstMinutes();
          }
        }, 15000);
      }
    } else {
      if (downloadTimer) {
        clearTimeout(downloadTimer);
        downloadTimer = null;
      }
    }
  }

  function readFirstMinutes() {
    if (firstMinutesClosed) return Promise.resolve(null);
    if (!firstMinutesRequest) {
      firstMinutesRequest = apiJson(FIRST_MINUTES_URL)
        .then(function (data) {
          if (!data || typeof data.window_open !== 'boolean') {
            firstMinutesPayload = null;
            setupPayload = null;
          } else if (data.window_open === false) {
            firstMinutesClosed = true;
            firstMinutesPayload = null;
            setupPayload = null;
            if (downloadTimer) {
              clearTimeout(downloadTimer);
              downloadTimer = null;
            }
          } else {
            firstMinutesPayload = data;
            // Setup progress and the saved choice come with first minutes,
            // which a linked device can read; `/init/api/state` stays local.
            setupPayload = isPlainObject(data.setup) ? data.setup : null;
          }
          scheduleOrClearDownloadTimer();
          updateGettingStartedSurface();
          return data;
        })
        .catch(function () {
          firstMinutesPayload = null;
          setupPayload = null;
          scheduleOrClearDownloadTimer();
          updateGettingStartedSurface();
          return null;
        })
        .finally(function () {
          firstMinutesRequest = null;
        });
    }
    return firstMinutesRequest;
  }

  function readAgents() {
    if (firstMinutesClosed) return Promise.resolve(null);
    if (typeof window.apiJson !== 'function') {
      agentsSettled = true;
      agentsPayload = { failed: true };
      updateGettingStartedSurface();
      return Promise.resolve(null);
    }
    if (!agentsRequest) {
      agentsRequest = window.apiJson(AGENTS_STATE_URL, { noAuthRedirect: true })
        .then(function (data) {
          agentsSettled = true;
          agentsPayload = isPlainObject(data) ? data : { connections: null };
          updateGettingStartedSurface();
          return data;
        })
        .catch(function () {
          agentsSettled = true;
          agentsPayload = { failed: true };
          updateGettingStartedSurface();
          return null;
        })
        .finally(function () {
          agentsRequest = null;
        });
    }
    return agentsRequest;
  }

  function handleSpeechRetry(button) {
    if (speechRetryInFlight) return;
    speechRetryInFlight = true;
    if (button) button.disabled = true;
    if (typeof window.apiJson !== 'function') {
      speechRetryInFlight = false;
      if (button) button.disabled = false;
      return;
    }
    window.apiJson(SETUP_ACTION_URL, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ action: 'retry_speech_download' }),
      noAuthRedirect: true,
    })
      .then(function () {
        speechRetryInFlight = false;
        readFirstMinutes();
      })
      .catch(function () {
        speechRetryInFlight = false;
        if (button) button.disabled = false;
      });
  }

  function handleCopyAgentCommand(button) {
    const cmd = button.getAttribute('data-command') || '';
    if (!cmd) return;
    if (window.convey && typeof window.convey.copyToClipboard === 'function') {
      window.convey.copyToClipboard(cmd);
    } else if (navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
      navigator.clipboard.writeText(cmd).catch(function () {});
    }
  }

  // "pulse" is the name of the run that wrote this, not a thing the owner has
  // a word for. The card says what it holds. X-03.
  function narrativeHeading(pulse) {
    const raw = String(pulse?.narrative_header || '').trim();
    if (!raw || raw === 'pulse') return 'recent activity';
    return raw;
  }

  function narrativeScope(pulse) {
    if (pulse.narrative_source !== 'pulse') return '';
    const scope = pulse.narrative_window;
    const keys = ['segments', 'activities', 'input_segments', 'input_activities'];
    if (!scope || !keys.every(key => Number.isSafeInteger(scope[key]) && scope[key] >= 0)) {
      return '<p class="pulse-narrative-meta">the input scope wasn\'t saved with this summary.</p>';
    }
    let text = 'based on ' + scope.segments + ' of ' + scope.input_segments + ' processed segments and '
      + scope.activities + ' of ' + scope.input_activities + ' activity entries in this batch. entries may overlap; this isn\'t a whole-day summary.';
    if (scope.since_ms > 0 && Number.isFinite(scope.since_ms) && Math.abs(scope.since_ms) <= 8640000000000000) {
      text += ' processing window starts at ' + window.JournalFormat.timestamp(new Date(scope.since_ms).toISOString()) + '. this is processing time, not when the activity happened.';
    }
    const gaps = Array.isArray(scope.gaps) ? scope.gaps : [];
    return '<details class="pulse-narrative-meta"><summary>what this summary covers</summary><p>' + esc(text) + '</p>'
      + (gaps.length ? '<p>some source material was unavailable:</p><ul>' + gaps.map(gap => '<li>' + esc(gap) + '</li>').join('') + '</ul>' : '') + '</details>';
  }

  function renderNarrativeHtml(pulse) {
    if (pulse.narrative_content !== null && pulse.narrative_content !== undefined) {
      const updated = pulse.narrative_updated_at ? '<div class="pulse-narrative-meta">updated at ' + esc(window.JournalFormat.timestamp(pulse.narrative_updated_at)) + '</div>' : '';
      return '<div class="pulse-narrative" id="pulse-narrative" data-home-surface="narrative" data-section-collapsed="true">'
        + '<div class="pulse-section-toggle" role="button" tabindex="0" aria-expanded="false">'
        + '<h2 class="pulse-section-header">' + esc(narrativeHeading(pulse)) + '</h2>'
        + '<span class="pulse-section-summary">' + esc(pulse.narrative_updated_at ? 'updated ' + window.JournalFormat.timestamp(pulse.narrative_updated_at) : narrativeHeading(pulse)) + '</span>'
        + '</div>'
        + '<div class="pulse-section-body">'
        + '<div class="pulse-narrative-content" id="pulse-narrative-content">' + markdown(pulse.narrative_content || '') + '</div>'
        + '<a class="pulse-tell-more" href="/app/thinking/#runs/' + encodeURIComponent(pulse.today) + '/' + encodeURIComponent(pulse.narrative_source || 'pulse') + '">how this was written →</a>'
        + narrativeScope(pulse)
        + updated
        + '</div>'
        + '</div>';
    }
    if (Number(pulse.segment_count || 0) > 0) {
      return '<div class="pulse-narrative" id="pulse-narrative" data-home-surface="narrative">'
        + '<h2 class="pulse-section-header">' + esc(narrativeHeading(pulse)) + '</h2>'
        + '<div class="pulse-narrative-empty">analysis will be available after the next processing cycle.</div>'
        + '</div>';
    }
    return '<div class="pulse-empty-state" data-home-surface="narrative">'
      + '<h2 class="pulse-section-header">today\'s flow</h2>'
      + '<div class="pulse-empty-message">no narrative yet. one will appear after some audio goes into your journal.</div>'
      + '</div>';
  }

  function renderNarrative(pulse) {
    replaceHomeSurface('narrative', renderNarrativeHtml(pulse), predecessors('narrative'));
  }

  function renderWeeklyReflectionHtml(pulse) {
    const reflection = pulse?.latest_weekly_reflection;
    if (!reflection || typeof reflection !== 'object') return '';
    if (reflection.state === 'week' && typeof reflection.url === 'string' && reflection.url) {
      let html = '<div class="pulse-reflection" data-home-surface="reflection">'
        + '<h2 class="pulse-section-header">your week</h2>'
        + '<div class="pulse-reflection-title">' + esc(reflection.title || '') + '</div>';
      if (typeof reflection.memory === 'string' && reflection.memory) {
        html += '<div class="pulse-reflection-memory">' + markdown(reflection.memory) + '</div>';
      }
      if (typeof reflection.empty === 'string' && reflection.empty) {
        html += '<div class="pulse-reflection-empty">' + esc(reflection.empty) + '</div>';
      }
      html += '<a class="pulse-reflection-link" href="' + esc(reflection.url) + '">open your week →</a>'
        + '</div>';
      return html;
    }
    if (typeof reflection.line === 'string' && reflection.line) {
      return '<div class="pulse-reflection" data-home-surface="reflection">'
        + '<div class="pulse-reflection-line">' + esc(reflection.line) + '</div>'
        + '</div>';
    }
    return '';
  }

  function renderWeeklyReflection(pulse) {
    replaceHomeSurface('reflection', renderWeeklyReflectionHtml(pulse), predecessors('reflection'));
  }

  function plural(count, one, many) {
    return count + ' ' + (count === 1 ? one : many);
  }

  // The day's stats file measures time for the segments it managed to measure,
  // which is not the same set as the activity list. Every facet the list holds
  // gets a chip, and a facet with no measured time says so rather than being
  // left out of the row entirely. G1-101.
  // An activity written under two facets is collapsed onto one row that carries
  // every facet it touched in `facets` (readers.rs merge_faceted_activity).
  // Reading only `facet` credited the merge to whichever copy landed first and
  // dropped the others from the chips entirely (F-10).
  function activityFacets(activity) {
    const many = Array.isArray(activity.facets) ? activity.facets : [activity.facet];
    const names = [];
    many.forEach(function (value) {
      const facet = typeof value === 'string' ? value.trim() : '';
      if (facet && names.indexOf(facet) === -1) names.push(facet);
    });
    return names;
  }

  function facetChipRows(pulse, activities) {
    const measuredFacets = isPlainObject(pulse.facet_data) ? pulse.facet_data : {};
    const names = Object.keys(measuredFacets);
    activities.forEach(function (activity) {
      activityFacets(activity).forEach(function (facet) {
        if (names.indexOf(facet) === -1) names.push(facet);
      });
    });
    return names.map(function (name) {
      const data = measuredFacets[name];
      const minutes = isPlainObject(data) ? (parseInt(data.minutes || 0, 10) || 0) : 0;
      // No `count` here on purpose: facet_data counts the whole day, and the
      // label beside these chips counts the window the card shows.
      return { name: name, text: isPlainObject(data) ? minutes + 'm' : 'no time measured' };
    });
  }

  function renderTodayHtml(pulse) {
    const activities = Array.isArray(pulse.activities) ? pulse.activities : [];
    if (!activities.length) {
      return '<div class="pulse-empty-state" data-home-surface="today">'
        + '<h2 class="pulse-section-header">today</h2>'
        + '<div class="pulse-empty-message">no recent activity yet today.</div>'
        + '<a class="pulse-today-link" href="/app/activities/" data-copy-key="home-today-link">all of today\'s activities →</a>'
        + '</div>';
    }
    let html = '<div class="pulse-today" id="pulse-today" data-home-surface="today" data-section-collapsed="true">'
      + '<div class="pulse-section-toggle" role="button" tabindex="0" aria-expanded="false">'
      + '<h2 class="pulse-section-header">today</h2>'
      + '<span class="pulse-section-summary">' + esc(pulse.today_summary || '') + '</span>'
      + '</div>'
      + '<div class="pulse-section-body">';
    if (activities.length) {
      // The header counted all of them, the list showed six, and the chips
      // measured a third population. Each number now says what it counts, and
      // the rest of the list is one click away instead of gone. G1-101.
      const shown = todayActivitiesExpanded ? activities : activities.slice(0, ACTIVITY_PREVIEW);
      const label = shown.length === activities.length
        ? plural(activities.length, 'activity', 'activities')
        : shown.length + ' of ' + plural(activities.length, 'activity', 'activities');
      const showAll = shown.length === activities.length
        ? ''
        : '<button type="button" class="pulse-activities-more" data-activities-show-all>show all →</button>';
      html += '<div class="pulse-activities-label">' + esc(label) + showAll + '</div><div class="pulse-activities">';
      shown.forEach(function (activity) {
        const description = activity.description || activity.activity || '';
        // Two capture streams can record one stretch as two activities. Both
        // stay on the list, and the server names the source of each only when
        // another stream ran alongside it in the same facet.
        const source = typeof activity.source_label === 'string' ? activity.source_label.trim() : '';
        const day = activity.day || pulse.today || pulse.day || '';
        const facet = activity.facet || (Array.isArray(activity.facets) ? activity.facets[0] : '') || '';
        const id = activity.id || '';
        const href = '/app/activities/' + esc(day)
          + '?facet=' + esc(encodeURIComponent(facet))
          + '#' + esc(encodeURIComponent(facet)) + '/' + esc(encodeURIComponent(id));
        html += '<a class="pulse-activity" href="' + href + '">'
          + '<span class="pulse-activity-time">' + esc(activity.display_time ? window.JournalFormat.time(activity.display_time) : '') + '</span>'
          + '<span class="pulse-activity-text">' + esc(description)
          + (source ? '<span class="pulse-activity-source">' + esc(source) + '</span>' : '')
          + '</span>'
          + '</a>';
      });
      html += '</div>';
    }
    const facetChips = facetChipRows(pulse, activities);
    if (facetChips.length) {
      // Both halves of "N of M" have to count the same population. The
      // facet_data counts are whole-day and cover every facet, while
      // `activities` is the rolling window this card shows, so summing the
      // counts against it printed a numerator larger than its denominator.
      // Count the shown activities that land in a measured facet instead.
      // G1-101.
      const measuredFacets = isPlainObject(pulse.facet_data) ? pulse.facet_data : {};
      const measured = activities.filter(function (activity) {
        return activityFacets(activity).some(function (facet) {
          return isPlainObject(measuredFacets[facet]);
        });
      }).length;
      const chipLabel = activities.length
        ? 'time by facet · measured for ' + measured + ' of ' + plural(activities.length, 'activity', 'activities')
        : 'time by facet';
      html += '<div class="pulse-activities-label">' + esc(chipLabel) + '</div><div class="pulse-facet-dist">';
      facetChips.forEach(function (chip) {
        html += '<span class="pulse-facet-chip">' + esc(chip.name) + ' · ' + esc(chip.text) + '</span>';
      });
      html += '</div>';
    }
    html += '<a class="pulse-today-link" href="/app/activities/" data-copy-key="home-today-link">all of today\'s activities →</a>';
    html += '</div></div>';
    return html;
  }

  function renderToday(pulse) {
    replaceHomeSurface('today', renderTodayHtml(pulse), predecessors('today'));
    placeTodayZoneNote();
  }

  // Both the first paint and a re-render of the today card name the
  // journal's zone on its heading. The empty state carries the surface
  // marker but not the card's id.
  function placeTodayZoneNote() {
    const h2 = document.querySelector('[data-home-surface="today"] .pulse-section-header');
    if (h2 && window.JournalClock) {
      window.JournalClock.placeNote(h2, { inside: true });
    }
  }

  function renderYesterdayProcessingHtml(pulse) {
    const yesterday = pulse.yesterday_processing;
    if (!yesterday) return '';
    // Overnight trouble still opens itself when a run actually failed. When the
    // day is live and nothing failed, it stays folded to its one line and the
    // day leads instead. X-03.
    const foldForLiveDay = dayHasMaterial(pulse)
      && Number(yesterday.failed_run_count || 0) === 0;
    const collapsed = (yesterday.default_collapsed || foldForLiveDay) ? 'true' : 'false';
    let html = '<section class="pulse-yesterday" id="pulse-yesterday" data-home-surface="yesterday" data-collapsed="' + collapsed + '">'
      + '<div class="pulse-yesterday-header" role="button" tabindex="0" aria-expanded="' + (collapsed === 'true' ? 'false' : 'true') + '">'
      + '<h2 class="pulse-section-header">' + esc(yesterday.title || "Yesterday's processing") + '</h2>'
      + '</div>'
      + '<div class="pulse-yesterday-summary">' + esc(yesterday.summary_line || '') + '</div>'
      + '<div class="pulse-yesterday-body">';
    if (yesterday.first_week_framing) {
      html += '<p class="pulse-yesterday-framing">' + esc(yesterday.first_week_framing) + '</p>';
    }
    if (yesterday.mode === 'sparse') {
      (yesterday.sparse_lines || []).forEach(function (line) {
        html += '<p>' + esc(line) + '</p>';
      });
    } else {
      // The briefing card states the missing briefing in its own words and
      // links the same run. Yesterday's list does not say it a second time.
      // G1-103.
      const briefingHref = '/app/thinking/#runs/' + encodeURIComponent(pulse?.briefing_analysis_day || '') + '/morning_briefing';
      const briefingCardOwnsIt = pulse?.briefing_phase === 'missing' && !pulse?.briefing_exists;
      const gapLinks = (yesterday.gap_links || []).filter(function (link) {
        return !(briefingCardOwnsIt && link && link.href === briefingHref);
      });
      const details = yesterday.details || [];
      const gapListHtml = gapLinks.map(function (link) {
        return '<li><a href="' + esc(link.href || '#') + '">' + esc(link.text || '') + '</a></li>';
      }).join('');
      const detailsListHtml = details.map(function (line) {
        return '<li>' + esc(line) + '</li>';
      }).join('');
      if (gapLinks.length && details.length) {
        // Two different kinds of signal — what broke vs. neutral summary —
        // read as one undifferentiated list otherwise. Split them.
        const count = Number(yesterday.failed_run_count || 0);
        const countLabel = count > 0 ? ' · ' + count + (count === 1 ? ' run' : ' runs') : '';
        html += '<div class="pulse-yesterday-shelf-label">what didn\'t finish' + countLabel + '</div>'
          + '<ul class="pulse-yesterday-details">' + gapListHtml + '</ul>'
          + '<div class="pulse-yesterday-shelf-label pulse-yesterday-shelf-label-spaced">everything else</div>'
          + '<ul class="pulse-yesterday-details">' + detailsListHtml + '</ul>';
      } else if (gapLinks.length || details.length) {
        html += '<ul class="pulse-yesterday-details">' + gapListHtml + detailsListHtml + '</ul>';
      }
    }
    html += '</div></section>';
    return html;
  }

  function renderYesterdayProcessing(pulse) {
    replaceHomeSurface('yesterday', renderYesterdayProcessingHtml(pulse), predecessors('yesterday'));
  }

  function needsYouItemHtml(item) {
    const text = item && typeof item.text === 'string' ? item.text : '';
    if (item && item.disabled) {
      const reason = typeof item.reason === 'string' && item.reason ? '<span class="pulse-needs-reason">' + esc(item.reason) + '</span>' : '';
      return '<div class="pulse-needs-item pulse-needs-item-disabled">' + esc(text) + reason + '</div>';
    }
    if (item && item.kind === 'route') {
      const encoded = esc(JSON.stringify(item || {}));
      return '<div class="pulse-needs-item" role="button" tabindex="0" data-needs-you-item="' + encoded + '">' + esc(text) + '</div>';
    }
    return '<div class="pulse-needs-item">' + esc(text) + '</div>';
  }

  function renderNeedsYouHtml(pulse) {
    const items = Array.isArray(pulse.needs_you_items) ? pulse.needs_you_items : [];
    if (!items.length) {
      // health_glance flags its own issues (device/backlog/pipeline health) up in the
      // vitals banner, a separate signal from needs_you_items. Say "else" when it has
      // something flagged so this empty state never reads as a flat contradiction.
      const emptyMessage = pulse.health_glance && pulse.health_glance.verdict === 'attention'
        ? 'nothing else needs your attention right now.'
        : 'nothing needs your attention right now.';
      return '<div class="pulse-empty-state" data-home-surface="needs">'
        + '<h2 class="pulse-section-header">needs you</h2>'
        + '<div class="pulse-empty-message">' + esc(emptyMessage) + '</div>'
        + '</div>';
    }
    return '<div class="pulse-needs" id="pulse-needs" data-home-surface="needs" data-section-collapsed="false">'
      + '<div class="pulse-section-toggle" role="button" tabindex="0" aria-expanded="true">'
      + '<h2 class="pulse-section-header">needs you</h2>'
      + '<span class="pulse-section-summary">' + esc(pulse.needs_summary || '') + '</span>'
      + '</div>'
      + '<div class="pulse-section-body"><div class="pulse-needs-list">'
      + items.map(needsYouItemHtml).join('')
      + '</div></div></div>';
  }

  // The welcome window has no needs card of its own, but something that is
  // owed from the first day, like setting up your voice, still shows there.
  function welcomeNeedsYouHtml(pulse) {
    const items = Array.isArray(pulse.needs_you_items) ? pulse.needs_you_items : [];
    return items.length ? renderNeedsYouHtml(pulse) : '';
  }

  function renderNeedsYou(pulse) {
    const html = lastPulse && lastPulse.home_state === 'welcome'
      ? welcomeNeedsYouHtml(pulse)
      : renderNeedsYouHtml(pulse);
    const anchors = lastPulse && lastPulse.home_state === 'welcome'
      ? (isGettingStartedVisible() ? ['vitals', 'getting-started'] : ['vitals', 'welcome'])
      : predecessors('needs');
    replaceHomeSurface('needs', html, anchors);
  }

  // Every date on this dashboard is one shape, and the shape is the shared
  // ladder's: JournalFormat.day for a day, JournalFormat.time for a stamp
  // inside today. The connections shelf used to spell its own lowercase
  // 'sep 5' beside the activity list's 'Sep 6, 3:38 PM'. G1-105.
  // `JournalFormat.day` capitalises its relative tokens ("Yesterday",
  // "Wednesday") because it usually opens a line or a cell. Only the three
  // deictic words go lowercase mid-sentence; a weekday is a proper noun and
  // keeps its capital, as does an absolute day ("Thu Sep 3").
  const RELATIVE_DAY_WORDS = new Set([
    'today', 'yesterday', 'tomorrow'
  ]);

  function formatConnectionDay(day) {
    return window.JournalFormat.day(day);
  }

  function midSentenceDay(day) {
    const text = String(day === null || day === undefined ? '' : day);
    return RELATIVE_DAY_WORDS.has(text.toLowerCase()) ? text.toLowerCase() : text;
  }

  function connectionEntityHref(entityId) {
    return '/app/entities#' + String(entityId || '');
  }

  function connectionKindChipsHtml(neighbor, connections) {
    const kindWords = isPlainObject(connections.kind_words) ? connections.kind_words : {};
    const attendanceKinds = new Set(Array.isArray(connections.attendance_kinds) ? connections.attendance_kinds : []);
    const kinds = Array.isArray(neighbor.kinds) ? neighbor.kinds : [];
    const chips = kinds
      .filter(function (item) {
        return isPlainObject(item)
          && typeof item.kind === 'string'
          && !attendanceKinds.has(item.kind)
          && typeof kindWords[item.kind] === 'string';
      })
      .slice(0, 2)
      .map(function (item) {
        const label = kindWords[item.kind];
        return '<span class="pulse-connections-chip">' + esc(label) + '</span>';
      });
    if (!chips.length) return '';
    return '<span class="pulse-connections-chip-list">' + chips.join('') + '</span>';
  }

  // A bare four-digit count reads as authoritative without saying what it
  // covers. The count carries its unit and a thousands separator, and the day
  // beside it says which end of the window it is. G1-102.
  function connectionRowMeta(neighbor) {
    const count = Number(neighbor.count || 0);
    const moments = count === 1 ? '1 moment' : count.toLocaleString('en-US') + ' moments';
    const day = formatConnectionDay(neighbor.last_seen);
    return day ? moments + ' · most recently ' + midSentenceDay(day) : moments;
  }

  // The evidence line repeated the name back under itself on every row but one,
  // and the one row where it differed read like a truncation bug. G1-27.
  function connectionEvidenceHtml(neighbor, name) {
    const label = typeof neighbor.latest_label === 'string' ? neighbor.latest_label.trim() : '';
    if (!label || label.toLowerCase() === String(name || '').trim().toLowerCase()) return '';
    return '<span class="pulse-connections-evidence">transcribed as “' + esc(label) + '”</span>';
  }

  function connectionRowHtml(neighbor, connections) {
    const entityId = typeof neighbor.entity_id === 'string' ? neighbor.entity_id : '';
    const name = typeof neighbor.name === 'string' && neighbor.name ? neighbor.name : entityId;
    if (!entityId || !name) return '';
    return '<div class="pulse-connections-row">'
      + '<a class="pulse-connections-name" href="' + esc(connectionEntityHref(entityId)) + '">' + esc(name) + '</a>'
      + connectionKindChipsHtml(neighbor, connections)
      + '<span class="pulse-connections-meta">' + esc(connectionRowMeta(neighbor)) + '</span>'
      + connectionEvidenceHtml(neighbor, name)
      + '</div>';
  }

  function connectionAttendanceItemHtml(neighbor) {
    const entityId = typeof neighbor.entity_id === 'string' ? neighbor.entity_id : '';
    const name = typeof neighbor.name === 'string' && neighbor.name ? neighbor.name : entityId;
    if (!entityId || !name) return '';
    const count = Number(neighbor.count || 0);
    const eventCount = count === 1 ? '1 event' : String(count) + ' events';
    return '<a href="' + esc(connectionEntityHref(entityId)) + '">' + esc(name) + '</a>'
      + ' <span class="pulse-connections-cluster-count">' + esc(eventCount) + '</span>';
  }

  function connectionCalendarItemHtml(neighbor) {
    const entityId = typeof neighbor.entity_id === 'string' ? neighbor.entity_id : '';
    const name = typeof neighbor.name === 'string' && neighbor.name ? neighbor.name : entityId;
    if (!entityId || !name) return '';
    return '<a href="' + esc(connectionEntityHref(entityId)) + '">' + esc(name) + '</a>';
  }

  function renderConnectionsHtml(pulse) {
    const connections = isPlainObject(pulse?.connections) ? pulse.connections : null;
    if (!connections || typeof connections.state !== 'string') return '';
    if (connections.state === 'empty') {
      return '<div class="pulse-empty-state" data-home-surface="connections">'
        + '<h2 class="pulse-section-header">connections</h2>'
        + '<div class="pulse-empty-message">no connections yet. this is built from the people and things your days involve.</div>'
        + '</div>';
    }
    if (connections.state === 'unnamed') {
      // Every neighbor the index had was an unnamed voice cluster. Say that,
      // and point at the one place it can be fixed. X-02.
      return '<div class="pulse-empty-state" data-home-surface="connections">'
        + '<h2 class="pulse-section-header">connections</h2>'
        + '<div class="pulse-empty-message">no named connections yet. '
        + '<a href="/app/speakers/">name voices in speakers →</a></div>'
        + '</div>';
    }
    if (connections.state === 'unavailable') {
      return '<div class="pulse-empty-state" data-home-surface="connections">'
        + '<h2 class="pulse-section-header">connections</h2>'
        + '<div class="pulse-empty-message">connections are unavailable right now. '
        + '<a href="/app/health">check system health →</a></div>'
        + '</div>';
    }
    if (connections.state !== 'ok' || !Array.isArray(connections.neighbors)) return '';

    const neighbors = connections.neighbors.filter(isPlainObject);
    const mentionOnly = neighbor => Array.isArray(neighbor.kinds) && neighbor.kinds.length > 0 && neighbor.kinds.every(item => item.kind === 'mentioned');
    const mentionRows = neighbors.filter(mentionOnly).slice(0, 6);
    const relationshipRows = neighbors
      .filter(function (neighbor) { return neighbor.evidence_class !== 'attendance' && neighbor.evidence_class !== 'scheduled' && !mentionOnly(neighbor); })
      .slice(0, 6);
    const attendanceRows = neighbors
      .filter(function (neighbor) { return neighbor.evidence_class === 'attendance'; })
      .slice(0, 8);
    const calendarRows = neighbors
      .filter(function (neighbor) { return neighbor.evidence_class === 'scheduled'; })
      .slice(0, 8);
    const relationshipItems = relationshipRows.map(function (neighbor) {
      return connectionRowHtml(neighbor, connections);
    }).filter(Boolean);
    const attendanceItems = attendanceRows.map(connectionAttendanceItemHtml).filter(Boolean);
    const calendarItems = calendarRows.map(connectionCalendarItemHtml).filter(Boolean);
    let html = '<div class="pulse-connections" data-home-surface="connections">'
      + '<h2 class="pulse-section-header">connections</h2>';

    if (relationshipItems.length) {
      // The shelf label only tells the owner anything when a second shelf is
      // there to tell it apart from. Alone it is the card heading again. G1-28.
      const shelfLabel = attendanceItems.length || calendarItems.length
        ? '<div class="pulse-connections-shelf-label">connections found in your journal</div>'
        : '';
      html += '<div class="pulse-connections-shelf" data-shelf="journal">'
        + shelfLabel
        + '<div class="pulse-connections-list">'
        + relationshipItems.join('')
        + '</div></div>';
    } else if (!attendanceItems.length && !calendarItems.length && !mentionRows.length) {
      // Nothing to show is one line, not four blocks. The shelves below state
      // their own kind, so the note only earns its space when it is the only
      // thing the section has to say. G1-107, X-04.
      html += '<div class="pulse-connections-note">no direct connections yet.</div>';
    }

    if (attendanceItems.length) {
      html += '<div class="pulse-connections-shelf" data-shelf="attendance">'
        + '<div class="pulse-connections-shelf-label">often around, events only</div>'
        + '<div class="pulse-connections-cluster">' + attendanceItems.join(' · ') + '</div>'
        + '</div>';
    }

    if (calendarItems.length) {
      html += '<div class="pulse-connections-shelf" data-shelf="calendar">'
        + '<div class="pulse-connections-shelf-label">on your calendar only</div>'
        + '<div class="pulse-connections-cluster">' + calendarItems.join(' · ') + '</div>'
        + '</div>';
    }

    if (mentionRows.length) {
      // The summary says how many rows the disclosure holds, so it stays useful
      // as the only shelf on the card rather than restating the section. G1-28.
      const mentionSummary = mentionRows.length === 1
        ? '1 name mentioned in your journal'
        : mentionRows.length + ' names mentioned in your journal';
      html += '<details class="pulse-connections-shelf"><summary>' + esc(mentionSummary) + '</summary><div class="pulse-connections-list">'
        + mentionRows.map(neighbor => connectionRowHtml(neighbor, connections)).join('') + '</div></details>';
    }
    const rendered = relationshipItems.length + attendanceItems.length + calendarItems.length + mentionRows.length;
    // The coverage note and the way out of the section are one block, not two.
    // G1-107.
    const moreLink = Number(connections.total || 0) > rendered
      ? '<a class="pulse-connections-more" href="/app/entities">all connections →</a>'
      : '';
    if (typeof connections.horizon_note === 'string' && connections.horizon_note
        && typeof connections.horizon_day === 'string' && connections.horizon_day) {
      html += '<div class="pulse-connections-horizon">'
        + esc(connections.horizon_note.replace('{day}', midSentenceDay(formatConnectionDay(connections.horizon_day))))
        + ' '
        + moreLink
        + '</div>';
    } else if (moreLink) {
      html += '<div class="pulse-connections-footer">' + moreLink + '</div>';
    }
    return html + '</div>';
  }

  function renderConnections(pulse) {
    replaceHomeSurface('connections', renderConnectionsHtml(pulse), predecessors('connections'));
  }

  const CARD_RENDERERS = {
    vitals: renderVitalsHtml,
    'getting-started': function () {
      return isGettingStartedVisible() ? renderGettingStartedHtml() : '';
    },
    yesterday: renderYesterdayProcessingHtml,
    briefing: renderBriefingShellHtml,
    narrative: renderNarrativeHtml,
    reflection: renderWeeklyReflectionHtml,
    today: renderTodayHtml,
    needs: renderNeedsYouHtml,
    connections: renderConnectionsHtml
  };

  function normalizeBriefingFromPulse(pulse) {
    return {
      exists: Boolean(pulse.briefing_exists),
      phase: pulse.briefing_phase || 'eod',
      saved_lane: pulse.briefing_saved_lane ?? null,
      summary: pulse.briefing_summary || '',
      meta: pulse.briefing_meta || null,
      sections: isPlainObject(pulse.briefing_sections) ? pulse.briefing_sections : {},
      needs_deduped: Array.isArray(pulse.briefing_needs_deduped) ? pulse.briefing_needs_deduped : [],
      needs_shared_count: Number(pulse.briefing_needs_shared_count || 0),
      needs_badge: pulse.briefing_needs_badge || ''
    };
  }

  function renderBriefingShellHtml(pulse) {
    return renderBriefingCardHtml(normalizeBriefingFromPulse(pulse), pulse);
  }

  function formatBriefingTime(generated) {
    if (!generated) return '';
    const text = String(generated);
    if (text.indexOf('T') !== -1) {
      return text.split('T', 2)[1].substring(0, 5);
    }
    return text.substring(Math.max(0, text.length - 5));
  }

  // Before the nightly run has had its first chance, say when the first
  // briefing is due rather than calling it late or missing.
  const BRIEFING_NOT_YET = Object.assign(Object.create(null), {
    awaiting_engine: 'morning briefings start once you choose where the thinking happens.',
    first_night: "your first briefing is due by 10 am, after your journal's first night.",
    first_night_empty: 'your first briefing is due the morning after something goes into your journal.'
  });

  const BRIEFING_AWAITING_ENGINE_LANES = Object.assign(Object.create(null), {
    byo: 'morning briefings start once your own model is set up.',
    spp: 'morning briefings start once confidential processing is set up.',
    local: 'morning briefings start once local thinking is set up.'
  });

  function briefingPlaceholderHtml(data, pulseContext) {
    if (data.exists) return '';
    // A briefing that was never prepared used to render nothing at all from ten
    // in the morning onward. Silence is the worst failure mode. X-04.
    if (data.phase === 'missing') {
      return '<div class="pulse-briefing-placeholder">'
        + "your morning briefing wasn't prepared. "
        + '<a class="pulse-briefing-status-link" href="/app/thinking/#runs/' + esc(pulseContext?.briefing_analysis_day || '') + '/morning_briefing">see the run →</a>'
        + '</div>';
    }
    if (data.phase === 'awaiting_engine') {
      const text = BRIEFING_AWAITING_ENGINE_LANES[data.saved_lane] || BRIEFING_NOT_YET.awaiting_engine;
      return '<div class="pulse-briefing-placeholder">' + esc(text) + '</div>';
    }
    const notYet = BRIEFING_NOT_YET[data.phase];
    if (notYet) return '<div class="pulse-briefing-placeholder">' + esc(notYet) + '</div>';
    if (data.phase !== 'pending') return '';
    const lateness = pulseContext?.briefing_lateness || {};
    if (lateness.late) {
      return '<div class="pulse-briefing-placeholder">'
        + "your briefing is usually ready by 10 am; it's " + esc(lateness.late_hours || 0) + 'h late.'
        + '<a class="pulse-briefing-status-link" href="/app/thinking/#runs/' + esc(pulseContext?.briefing_analysis_day || '') + '/morning_briefing">check status</a>'
        + '</div>';
    }
    return '<div class="pulse-briefing-placeholder">your morning briefing is being prepared…</div>';
  }

  function renderBriefingCardHtml(data, pulseContext) {
    if (!data.exists && data.phase !== 'pending' && data.phase !== 'missing' && !BRIEFING_NOT_YET[data.phase]) return '';
    const existing = document.getElementById('pulse-briefing');
    const collapsed = data.phase === 'morning' ? 'false' : (existing?.dataset?.collapsed || (data.phase === 'morning' ? 'false' : 'true'));
    const metaText = data.meta && data.meta.generated ? formatBriefingTime(data.meta.generated) : '';
    const summary = data.summary ? '<div class="pulse-briefing-summary">' + esc(data.summary) + '</div>' : '';
    const badge = data.needs_badge ? '<span class="pulse-briefing-badge">' + esc(data.needs_badge) + '</span>' : '';
    const meta = metaText ? '<span class="pulse-briefing-meta">' + esc(metaText) + '</span>' : '';
    const body = data.exists
      ? '<div class="pulse-briefing-body" id="pulse-briefing-body">' + renderBriefingSectionsHtml(data) + '</div>'
      : '';
    // A header only announces itself as expandable when there is a body to
    // expand. In the 'missing' and 'pending' phases the card's one line sits
    // outside the collapsible body, so the header is a plain heading. G1-104.
    const headerAttrs = data.exists
      ? ' role="button" tabindex="0" aria-expanded="' + (collapsed === 'false' ? 'true' : 'false') + '"'
      : '';
    return '<div class="pulse-briefing-card" id="pulse-briefing" data-home-surface="briefing" data-phase="' + esc(data.phase) + '" data-collapsed="' + collapsed + '">'
      + '<div class="pulse-briefing-header"' + headerAttrs + '>'
      + '<h2 class="pulse-section-header">morning briefing</h2>'
      + badge
      + meta
      + '</div>'
      + summary
      + briefingPlaceholderHtml(data, pulseContext)
      + body
      + '</div>';
  }

  function renderBriefingSectionsHtml(data) {
    const sections = isPlainObject(data.sections) ? data.sections : {};
    const needs = Array.isArray(data.needs_deduped) ? data.needs_deduped : [];
    return briefingRenderOrder.map(function (key) {
      if (key === 'needs_attention') {
        if (!needs.length && !data.needs_badge) return '';
        const needsBody = needs.length
          ? '<div class="pulse-briefing-section-body" data-section-key="needs_attention"><ul>'
            + needs.map(function (item) {
              return '<li>' + markdown(String(item || '')) + '</li>';
            }).join('')
            + '</ul></div>'
          : '';
        return '<div class="pulse-briefing-section" data-section="needs_attention" data-collapsed="false">'
          + '<button class="pulse-briefing-section-toggle" aria-expanded="true">needs attention</button>'
          + needsBody
          + '</div>';
      }
      const raw = sections[key];
      if (!raw) return '';
      return '<div class="pulse-briefing-section" data-section="' + esc(key) + '" data-collapsed="false">'
        + '<button class="pulse-briefing-section-toggle" aria-expanded="true">' + esc(briefingSectionLabels[key]) + '</button>'
        + '<div class="pulse-briefing-section-body" data-section-key="' + esc(key) + '">' + markdown(raw) + '</div>'
        + '</div>';
    }).join('');
  }

  function renderBriefingSections(sections) {
    if (!document.querySelector) return;
    briefingSectionOrder.forEach(function (key) {
      const raw = sections[key];
      const el = document.querySelector('[data-section-key="' + key + '"]');
      if (!el) return;
      if (!raw) {
        el.innerHTML = '';
        return;
      }
      el.innerHTML = markdown(raw);
    });
  }

  function renderBriefing(data, pulseContext = lastPulse) {
    validateBriefing(data);
    briefingSections = isPlainObject(data.sections) ? data.sections : {};
    const html = renderBriefingCardHtml(data, pulseContext);
    replaceHomeSurface('briefing', html, predecessors('briefing'));
  }

  function renderBriefingError(error) {
    const html = '<div class="pulse-briefing-card" id="pulse-briefing" data-home-surface="briefing" data-phase="error" data-collapsed="false">'
      + surfaceError(error, "Couldn't refresh briefing")
      + '</div>';
    replaceHomeSurface('briefing', html, predecessors('briefing'));
    const retry = document.querySelector('#pulse-briefing .surface-state-retry');
    if (retry) {
      retry.addEventListener('click', function () {
        refreshBriefing();
      });
    }
  }

  function refreshVitals() {
    if (!firstMinutesClosed) {
      readFirstMinutes();
    }
    if (!vitalsRequest) {
      vitalsRequest = readVitals().finally(function () { vitalsRequest = null; });
    }
    return vitalsRequest;
  }

  function readVitals() {
    const read = ++pulseReadVersion;
    return readPulse()
      .then(function (data) {
        if (read < pulseAppliedVersion) return;
        pulseAppliedVersion = read;
        validatePulse(data);
        if (!lastPulse) { renderPulse(data); return; }
        lastPulse = Object.assign({}, lastPulse || {}, data);
        pulseReadFailed = false;
        pulseObservedAt = Date.now();
        observeBackup(data);
        renderNeedsYou(data);
        renderConnections(data);
        renderVitals(data);
        clearPulseRefreshError('pulse-vitals');
      })
      .catch(function (error) {
        if (read < pulseAppliedVersion) return;
        pulseAppliedVersion = read;
        pulseReadFailed = true;
        if (!lastPulse && surface) {
          surface.innerHTML = surfaceError(error, "Couldn't load pulse");
          bindSurfaceRetry();
        }
        if (lastPulse) renderVitals(lastPulse);
        logError(error, 'home: refreshVitals failed');
        renderRefreshError('pulse-vitals', error, "couldn't refresh service status. showing last known state.");
      });
  }

  function refreshNarrative() {
    if (!narrativeRequest) {
      narrativeRequest = readNarrative().finally(function () { narrativeRequest = null; });
    }
    return narrativeRequest;
  }

  function readNarrative() {
    const read = ++pulseReadVersion;
    return readPulse()
      .then(function (data) {
        validatePulse(data);
        // A newer vitals read does not paint the narrative. Keep this
        // refresher's own result even when its health evidence was superseded.
        if (lastPulse) {
          renderNarrative(data);
          clearPulseRefreshError('pulse-narrative');
        }
        if (read < pulseAppliedVersion) return;
        pulseAppliedVersion = read;
        if (!lastPulse) { renderPulse(data); return; }
        lastPulse = Object.assign({}, lastPulse || {}, data);
        pulseReadFailed = false;
        pulseObservedAt = Date.now();
        observeBackup(data);
        renderVitals(data);
        renderNeedsYou(data);
        renderConnections(data);
        clearPulseRefreshError('pulse-vitals');
      })
      .catch(function (error) {
        if (read < pulseAppliedVersion) return;
        pulseAppliedVersion = read;
        pulseReadFailed = true;
        if (!lastPulse && surface) {
          surface.innerHTML = surfaceError(error, "Couldn't load pulse");
          bindSurfaceRetry();
        }
        if (lastPulse) renderVitals(lastPulse);
        logError(error, 'home: refreshNarrative failed');
        renderRefreshError('pulse-narrative', error, "couldn't refresh narrative. showing last known state.");
      });
  }

  async function refreshBriefing() {
    try {
      const data = await apiJson(BRIEFING_URL);
      renderBriefing(data, lastPulse);
      clearPulseRefreshError('pulse-briefing');
    } catch (error) {
      logError(error, 'home: refreshBriefing failed');
      renderBriefingError(error);
    }
  }

  function clearPulseRefreshError(containerId) {
    const container = document.getElementById(containerId);
    const siblingError = container?.nextElementSibling;
    if (siblingError && siblingError.classList.contains('surface-state-refresh-error')) {
      siblingError.remove();
    }
  }

  function dispatchNeedsYouItem(item) {
    if (!item || typeof item !== 'object') return;
    if (item.disabled) return;
    if (item.kind === 'route') {
      const href = item.payload && item.payload.href;
      if (typeof href === 'string' && href.startsWith('/') && !href.startsWith('//')) {
        window.location.href = href;
      }
    }
  }

  function toggleBriefingCard() {
    const card = document.getElementById('pulse-briefing');
    if (!card || card.dataset.phase === 'pending' || BRIEFING_NOT_YET[card.dataset.phase]) return;
    // No body, nothing to toggle: the header is a heading in this phase. G1-104.
    if (!card.querySelector('.pulse-briefing-body')) return;
    card.dataset.collapsed = card.dataset.collapsed === 'true' ? 'false' : 'true';
    const header = card.querySelector('.pulse-briefing-header');
    if (header) header.setAttribute('aria-expanded', card.dataset.collapsed === 'false' ? 'true' : 'false');
  }

  function toggleYesterdayCard() {
    const card = document.getElementById('pulse-yesterday');
    if (!card) return;
    card.dataset.collapsed = card.dataset.collapsed === 'true' ? 'false' : 'true';
    const header = card.querySelector('.pulse-yesterday-header');
    if (header) header.setAttribute('aria-expanded', card.dataset.collapsed === 'false' ? 'true' : 'false');
  }

  function toggleBriefingSection(sectionEl) {
    if (!sectionEl) return;
    sectionEl.dataset.collapsed = sectionEl.dataset.collapsed === 'true' ? 'false' : 'true';
    const toggle = sectionEl.querySelector('.pulse-briefing-section-toggle');
    if (toggle) toggle.setAttribute('aria-expanded', sectionEl.dataset.collapsed === 'false' ? 'true' : 'false');
  }

  function toggleSection(container) {
    if (!container || !container.hasAttribute('data-section-collapsed')) return;
    const collapsed = container.dataset.sectionCollapsed === 'true' ? 'false' : 'true';
    container.dataset.sectionCollapsed = collapsed;
    const toggle = container.querySelector('.pulse-section-toggle');
    if (toggle) toggle.setAttribute('aria-expanded', collapsed === 'false' ? 'true' : 'false');
    try {
      const saved = JSON.parse(sessionStorage.getItem(SECTION_STATE_KEY) || '{}');
      saved[container.id] = collapsed;
      sessionStorage.setItem(SECTION_STATE_KEY, JSON.stringify(saved));
    } catch (_err) {
      // Storage may be unavailable in private contexts; collapse state is non-critical.
    }
  }

  function restoreSectionState() {
    // A stored choice always wins; the defaults only decide what a browser that
    // has never been told anything shows.
    const defaults = activeDayIsLive() ? SECTION_DEFAULTS_LIVE : SECTION_DEFAULTS_QUIET;
    try {
      const saved = JSON.parse(sessionStorage.getItem(SECTION_STATE_KEY) || '{}');
      SECTION_IDS.forEach(function (id) {
        const el = document.getElementById(id);
        if (!el || !el.hasAttribute('data-section-collapsed')) return;
        const collapsed = Object.prototype.hasOwnProperty.call(saved, id) ? saved[id] : defaults[id];
        el.dataset.sectionCollapsed = collapsed;
        const toggle = el.querySelector('.pulse-section-toggle');
        if (toggle) toggle.setAttribute('aria-expanded', collapsed === 'false' ? 'true' : 'false');
      });
    } catch (_err) {
      // Storage may be unavailable in private contexts; collapse state is non-critical.
    }
  }

  function closest(target, selector) {
    return target && typeof target.closest === 'function' ? target.closest(selector) : null;
  }

  function handleDashboardClick(event) {
    const target = event.target;
    const showAll = closest(target, '[data-activities-show-all]');
    if (showAll) {
      event.preventDefault();
      todayActivitiesExpanded = true;
      if (lastPulse) renderToday(lastPulse);
      return;
    }
    const briefingToggle = closest(target, '.pulse-briefing-section-toggle');
    if (briefingToggle) {
      event.preventDefault();
      toggleBriefingSection(briefingToggle.closest('.pulse-briefing-section'));
      return;
    }
    const briefingHeader = closest(target, '.pulse-briefing-header');
    if (briefingHeader) {
      event.preventDefault();
      toggleBriefingCard();
      return;
    }
    const yesterdayHeader = closest(target, '.pulse-yesterday-header');
    if (yesterdayHeader) {
      event.preventDefault();
      toggleYesterdayCard();
      return;
    }
    const sectionToggle = closest(target, '.pulse-section-toggle');
    if (sectionToggle) {
      event.preventDefault();
      toggleSection(sectionToggle.parentElement);
      return;
    }
    const needsYouEl = closest(target, '[data-needs-you-item]');
    if (needsYouEl) {
      event.preventDefault();
      try {
        dispatchNeedsYouItem(JSON.parse(needsYouEl.dataset.needsYouItem || '{}'));
      } catch (error) {
        logError(error, 'home: needs-you parse');
      }
      return;
    }
    const speechRetry = closest(target, '[data-speech-retry]');
    if (speechRetry) {
      event.preventDefault();
      handleSpeechRetry(speechRetry);
      return;
    }
    const copyCmd = closest(target, '[data-copy-agent-command]');
    if (copyCmd) {
      event.preventDefault();
      handleCopyAgentCommand(copyCmd);
      return;
    }
  }

  function handleDashboardKeydown(event) {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    const target = event.target;
    const showAll = closest(target, '[data-activities-show-all]');
    if (showAll) {
      event.preventDefault();
      todayActivitiesExpanded = true;
      if (lastPulse) renderToday(lastPulse);
      return;
    }
    const briefingToggle = closest(target, '.pulse-briefing-section-toggle');
    if (briefingToggle) {
      event.preventDefault();
      toggleBriefingSection(briefingToggle.closest('.pulse-briefing-section'));
      return;
    }
    const briefingHeader = closest(target, '.pulse-briefing-header');
    if (briefingHeader) {
      event.preventDefault();
      toggleBriefingCard();
      return;
    }
    const yesterdayHeader = closest(target, '.pulse-yesterday-header');
    if (yesterdayHeader) {
      event.preventDefault();
      toggleYesterdayCard();
      return;
    }
    const sectionToggle = closest(target, '.pulse-section-toggle');
    if (sectionToggle) {
      event.preventDefault();
      toggleSection(sectionToggle.parentElement);
      return;
    }
    const needsYouEl = closest(target, '[data-needs-you-item]');
    if (needsYouEl) {
      event.preventDefault();
      try {
        dispatchNeedsYouItem(JSON.parse(needsYouEl.dataset.needsYouItem || '{}'));
      } catch (error) {
        logError(error, 'home: needs-you parse');
      }
      return;
    }
    const speechRetry = closest(target, '[data-speech-retry]');
    if (speechRetry) {
      event.preventDefault();
      handleSpeechRetry(speechRetry);
      return;
    }
    const copyCmd = closest(target, '[data-copy-agent-command]');
    if (copyCmd) {
      event.preventDefault();
      handleCopyAgentCommand(copyCmd);
      return;
    }
  }

  function wireInteractions() {
    if (interactionsWired || !root) return;
    root.addEventListener('click', handleDashboardClick);
    root.addEventListener('keydown', handleDashboardKeydown);
    interactionsWired = true;
  }

  // Events sent while the stream is down are not replayed, so the first
  // connected state after a disconnect re-reads what those events would have
  // refreshed. The state reported at subscription is the baseline, not a
  // reconnect; a page that subscribed while still connecting recovers on connect.
  function connectionRecovery(refresh) {
    let lastConnected = null;
    return function (state) {
      const connected = !!(state && state.connected);
      const recovered = connected && lastConnected === false;
      lastConnected = connected;
      if (recovered) refresh();
    };
  }

  function invalidateHeldWork() {
    heldWorkCurrent = false;
    if (lastPulse) renderVitals(lastPulse);
  }

  function receiveSupervisorStatus(msg) {
    if (msg.event === 'status' || msg.event === 'status-error') {
      queuedWorkAuthoritySeen = true;
    }
    if (msg.event === 'status-error') {
      invalidateHeldWork();
      return;
    }
    if (msg.event !== 'status') return;
    heldWorkSnapshot = { queues: msg.queues || {}, held: msg.held || [],
      queue_hold: msg.queue_hold || null, observedAt: Date.now() };
    heldWorkCurrent = true;
    window.clearTimeout(heldWorkExpiry);
    // Match Health's freshness boundary; unrelated bus activity cannot renew it.
    heldWorkExpiry = window.setTimeout(invalidateHeldWork, 30000);
    if (lastPulse) renderVitals(lastPulse);
    refreshVitals();
  }

  // A finished narrative or a recovered connection invalidates any earlier
  // observation. Follow an in-flight read once, even if several events arrive.
  function refreshChangedPulse() {
    if (!firstMinutesClosed) {
      readFirstMinutes();
    }
    if (trailingPulseRefresh) return trailingPulseRefresh;
    const pending = [pulseRequest, vitalsRequest, narrativeRequest].filter(Boolean);
    const refresh = function () { return Promise.all([refreshVitals(), refreshNarrative()]); };
    if (!pending.length) return refresh();
    trailingPulseRefresh = Promise.allSettled(pending).then(function () {
      trailingPulseRefresh = null;
      return refresh();
    });
    return trailingPulseRefresh;
  }

  function wireRealtime() {
    if (realtimeWired || !window.appEvents) return;
    const recover = connectionRecovery(function () {
      refreshChangedPulse();
      refreshBriefing();
    });
    window.appEvents.onConnectionState?.(function (state) {
      if (!state?.connected) {
        pulseAppliedVersion = ++pulseReadVersion;
        pulseReadFailed = true;
        invalidateHeldWork();
      }
      recover(state);
    });
    window.appEvents.listen('supervisor', receiveSupervisorStatus);
    window.appEvents.listen('observe', function (msg) {
      if (msg.event === 'observed' || msg.event === 'status') refreshVitals();
    });
    window.appEvents.listen('cortex', function (msg) {
      if (msg.event === 'finish' && (msg.name === 'flow' || msg.name === 'pulse')) refreshChangedPulse();
      if (msg.event === 'finish' && msg.name === 'morning_briefing') refreshBriefing();
      if (msg.event === 'error') refreshVitals();
    });
    realtimeWired = true;
  }

  function initHome() {
    if (homeInitialized) return;
    root = document.querySelector('[data-home-root]');
    surface = document.querySelector('[data-pulse-surface]');
    if (!root || !surface) return;
    homeInitialized = true;
    const pathname = window.location.pathname || '';
    if (pathname.startsWith('/app/home/week/')) {
      const stem = pathname.replace(/^\/app\/home\/week\/?/, '').split('/')[0];
      if (stem && typeof window.initWeek === 'function') {
        window.initWeek(stem);
      }
      return;
    }
    wireInteractions();
    wireRealtime();
    loadPulse();
    readFirstMinutes();
    readAgents();
  }

  document.addEventListener('visibilitychange', function () {
    if (!firstMinutesClosed) {
      readAgents();
    }
  });

  window.toggleBriefingCard = toggleBriefingCard;
  window.toggleYesterdayCard = toggleYesterdayCard;
  window.toggleBriefingSection = toggleBriefingSection;
  window.toggleSection = toggleSection;

  document.addEventListener('workspace:mounted', function (event) {
    if (event?.detail?.appName !== 'home') return;
    initHome();
  });
})();
