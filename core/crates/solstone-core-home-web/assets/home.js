// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const PULSE_URL = '/app/home/api/pulse';
  const BRIEFING_URL = '/app/home/api/briefing';
  const FIRST_MINUTES_URL = '/app/home/api/first-minutes';
  const AGENTS_STATE_URL = '/app/agents/api/state';
  const SETUP_ACTION_URL = '/init/api/action';
  const ORIGINALS_URL = '/app/home/api/removals?offset=0';
  const ORIGINALS_PAGE = '/app/home/originals/';
  const WAITING_HEADING = 'waiting on you';
  // The page's sections in the order they stand. A live refresh that brings a
  // section back inserts it after whichever of these is already there.
  const SURFACE_ORDER = ['vitals', 'getting-started', 'account', 'pair', 'welcome'];
  const ACCOUNT_SIDES = ['yesterday', 'today'];

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
  // The yesterday side's account: the pulse's copy at a full paint, then each
  // briefing read's.
  let briefingData = null;
  // Which side of the day switch the last full paint opened on. A live
  // refresh never re-decides it, so the switch does not move under the owner.
  // Null in the welcome state, which has no switch.
  let paintedSide = null;
  let selectedSide = null;
  // The originals list's whole-list item total: undefined until read, null
  // when it couldn't be read. Page state, not pulse data, so every re-render
  // of the status block keeps it.
  let originalsTotal;
  let originalsRequest = null;
  let isFirstPage = false;
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
  }

  function setSurfaceHtml(html) {
    if (!surface) return;
    surface.innerHTML = html;
    keepGettingStartedFirst();
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

  function predecessors(name) {
    const index = SURFACE_ORDER.indexOf(name);
    return index > 0 ? SURFACE_ORDER.slice(0, index) : [];
  }

  // A full paint: the first read, a retry, or the journal leaving the welcome
  // state. Only here is the day switch's opening side decided. It opens on
  // yesterday until today has material, then on today.
  function renderPulse(data) {
    validatePulse(data);
    lastPulse = data;
    pulseReadFailed = false;
    pulseObservedAt = Date.now();
    observeBackup(data);
    briefingData = normalizeBriefingFromPulse(data);
    const welcome = data.home_state === 'welcome';
    paintedSide = welcome ? null : (dayHasMaterial(data) ? 'today' : 'yesterday');
    selectedSide = paintedSide;
    const cards = [renderVitalsHtml(data)];
    if (isGettingStartedVisible()) cards.push(renderGettingStartedHtml());
    if (welcome) {
      cards.push(renderWelcomeHtml());
      setSurfaceHtml(cards.join(''));
      return;
    }
    cards.push(renderAccountHtml(data));
    cards.push(renderPairHtml(data));
    setSurfaceHtml(cards.join(''));
    placeAccountZoneNote();
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

  // A needs-you item becomes a row as it is: a route links where the item
  // would have sent the owner, and a disabled item shows its reason instead.
  function needsRow(item) {
    const text = item && typeof item.text === 'string' ? item.text : '';
    if (!text) return null;
    if (item.disabled) {
      return { text, reason: typeof item.reason === 'string' ? item.reason : '' };
    }
    const href = item.kind === 'route' && item.payload ? item.payload.href : null;
    if (typeof href === 'string' && href.startsWith('/') && !href.startsWith('//')) {
      return { text, href };
    }
    return { text };
  }

  // A night that went wrong stays on the page as at most two rows. The
  // missing account gets its own row unless the yesterday side, open since the
  // paint, already says so; everything else that didn't finish is one row.
  function nightRows(pulse) {
    const night = pulse?.yesterday_processing;
    if (!isPlainObject(night) || night.mode !== 'degraded') return [];
    const runs = '/app/thinking/#runs/' + encodeURIComponent(pulse.briefing_analysis_day || '');
    const accountRun = runs + '/morning_briefing';
    const links = (Array.isArray(night.gap_links) ? night.gap_links : []).filter(isPlainObject);
    // The missing account and the briefing's own failed run are one event:
    // say it once, as the account.
    const isAccount = link => link.href === accountRun || String(link.href || '').startsWith(accountRun + '/');
    const accountLinks = links.filter(isAccount);
    const rows = [];
    const account = accountLinks.find(link => link.href === accountRun) || accountLinks[0];
    if (account && paintedSide !== 'yesterday' && !briefingData?.exists) {
      rows.push({ text: account.text, href: account.href });
    }
    const rest = links.filter(link => !isAccount(link));
    const unknown = Array.isArray(night.status_reasons) && night.status_reasons.includes('pipeline_unavailable');
    if (unknown || rest.length > 1) {
      rows.push({ text: night.summary_line || '', href: runs });
    } else if (rest.length === 1) {
      rows.push({ text: rest[0].text, href: rest[0].href });
    }
    return rows;
  }

  function originalsRow() {
    if (originalsTotal === undefined || originalsTotal === 0) return null;
    if (originalsTotal === null) {
      return { text: "originals · the list couldn't be loaded.", href: ORIGINALS_PAGE };
    }
    return { text: 'originals · ' + plural(originalsTotal, 'item', 'items'), href: ORIGINALS_PAGE };
  }

  function waitingRows(pulse, glance) {
    const rows = (glance.issues || []).map(issue => ({
      text: issue.text || '', href: issue.href || '/app/health/', severity: issue.severity || 'amber'
    }));
    (Array.isArray(pulse.needs_you_items) ? pulse.needs_you_items : []).forEach(item => {
      const row = needsRow(item);
      if (row) rows.push(row);
    });
    nightRows(pulse).forEach(row => rows.push(row));
    const originals = originalsRow();
    if (originals) rows.push(originals);
    return rows;
  }

  function waitingRowHtml(row) {
    const severity = row.severity === 'red' ? ' red' : '';
    const ring = '<span class="pulse-waiting-ring" aria-hidden="true"></span>';
    if (row.href) {
      return '<li class="pulse-waiting-row' + severity + '">' + ring
        + '<a href="' + esc(row.href) + '">' + esc(row.text) + '</a></li>';
    }
    const reason = row.reason ? '<span class="pulse-waiting-reason">' + esc(row.reason) + '</span>' : '';
    return '<li class="pulse-waiting-row' + severity + '">' + ring + '<span>' + esc(row.text) + reason + '</span></li>';
  }

  // One status line, and under it whatever is waiting on the owner, only when
  // something is. When the health verdict itself is what's waiting, the list's
  // heading takes the headline's place, so a count never sits above a list
  // holding a different number of rows.
  function renderVitalsHtml(pulse) {
    const { glance: g, notes } = homeHealthGlance(pulse);
    const severity = g.severity || 'amber'; // 'neutral' is truthy; do not fold it into this fallback
    const rows = waitingRows(pulse, g);
    const attention = g.verdict === 'attention' && rows.length > 0;
    let html = '<div class="pulse-status" id="pulse-vitals" data-home-surface="vitals" role="status" aria-live="polite">'
      + '<div class="pulse-vitals">'
      + '<div class="pulse-vitals-item"><span class="pulse-vitals-dot ' + esc(severity) + '" aria-hidden="true"></span>'
      + '<span class="pulse-vitals-verdict ' + esc(severity) + '" id="pulse-status-headline">'
      + esc(attention ? WAITING_HEADING : (g.headline || '')) + '</span></div>';
    if (!g.issues?.length && g.last_observation) {
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
    if (rows.length) {
      if (!attention) {
        html += '<h2 class="pulse-waiting-heading" id="pulse-waiting-heading">' + WAITING_HEADING + '</h2>';
      }
      html += '<ul class="pulse-waiting" aria-labelledby="' + (attention ? 'pulse-status-headline' : 'pulse-waiting-heading') + '">'
        + rows.map(waitingRowHtml).join('') + '</ul>';
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
    return '<p class="pulse-welcome" data-home-surface="welcome">'
      + 'this is where your day comes together: what happened, written up, and the people and things it involved. sections appear here as what you share with the solstone app goes into your journal.'
      + '</p>';
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

  // Getting started comes and goes on its own; the welcome line beside it
  // stays put.
  function updateGettingStartedSurface() {
    if (!surface || !lastPulse) return;
    const current = surface.querySelector('[data-home-surface="getting-started"]');
    if (isGettingStartedVisible()) {
      const html = renderGettingStartedHtml();
      if (current) {
        current.outerHTML = html;
      } else {
        replaceHomeSurface('getting-started', html, predecessors('getting-started'));
      }
      keepGettingStartedFirst();
    } else if (current) {
      current.remove();
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

  // The today side's written account. The tab is its heading, so it carries
  // none of its own.
  function renderNarrativeHtml(pulse) {
    if (pulse.narrative_content !== null && pulse.narrative_content !== undefined) {
      const updated = pulse.narrative_updated_at
        ? '<span>updated at ' + esc(window.JournalFormat.timestamp(pulse.narrative_updated_at)) + '</span>'
        : '';
      return '<div class="pulse-narrative" id="pulse-narrative">'
        + '<div class="pulse-narrative-content" id="pulse-narrative-content">' + markdown(pulse.narrative_content || '') + '</div>'
        + '<div class="pulse-account-meta">' + updated
        + '<a class="pulse-tell-more" href="/app/thinking/#runs/' + encodeURIComponent(pulse.today) + '/' + encodeURIComponent(pulse.narrative_source || 'pulse') + '">how this was written →</a>'
        + '</div>'
        + narrativeScope(pulse)
        + '</div>';
    }
    const empty = Number(pulse.segment_count || 0) > 0
      ? 'analysis will be available after the next processing cycle.'
      : 'no narrative yet. one will appear after some audio goes into your journal.';
    return '<div class="pulse-narrative" id="pulse-narrative">'
      + '<div class="pulse-narrative-empty">' + esc(empty) + '</div>'
      + '</div>';
  }

  function renderNarrative(pulse) {
    replaceAccountContent(surface?.querySelector?.('[data-account-part="narrative"]'), renderNarrativeHtml(pulse));
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
      return '<div class="pulse-today" id="pulse-today">'
        + '<div class="pulse-narrative-empty">no recent activity yet today.</div>'
        + '<a class="pulse-today-link" href="/app/activities/" data-copy-key="home-today-link">all of today\'s activities →</a>'
        + '</div>';
    }
    let html = '<div class="pulse-today" id="pulse-today">';
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
    html += '</div>';
    return html;
  }

  function renderToday(pulse) {
    replaceAccountContent(surface?.querySelector?.('[data-account-part="activities"]'), renderTodayHtml(pulse));
  }

  // Replace part of the account card in place. Focus that was inside it goes
  // back to the same link, or to the selected tab, rather than to the page.
  function replaceAccountContent(container, html) {
    if (!container) return;
    const active = document.activeElement;
    const hadFocus = !!active && typeof container.contains === 'function' && container.contains(active);
    const href = hadFocus && active.tagName === 'A' ? active.getAttribute('href') : null;
    container.innerHTML = html;
    if (!hadFocus) return;
    const same = href
      ? Array.from(container.querySelectorAll('a')).find(link => link.getAttribute('href') === href)
      : null;
    const next = same || document.getElementById('pulse-account-tab-' + selectedSide);
    next?.focus({ preventScroll: true });
  }

  // The day the shown side covers: yesterday's side the briefing's analysis
  // day, today's side today, each from the payload its content came from.
  function accountDayText(side, pulse) {
    const day = side === 'yesterday'
      ? (briefingData?.analysis_day || pulse?.briefing_analysis_day)
      : pulse?.today;
    if (!day) return '';
    return window.JournalFormat ? window.JournalFormat.dayFull(day) : String(day);
  }

  // The journal clock's zone note sits beside the date, so an owner on a
  // device in another zone sees it whichever side is shown.
  function placeAccountZoneNote() {
    const day = document.querySelector('[data-account-day]');
    if (day && window.JournalClock) window.JournalClock.placeNote(day, { inside: true });
  }

  function updateAccountDay() {
    const text = document.querySelector('[data-account-day-text]');
    if (text) text.textContent = accountDayText(selectedSide, lastPulse);
    placeAccountZoneNote();
  }

  function renderTodaySideHtml(pulse) {
    return '<div data-account-part="narrative">' + renderNarrativeHtml(pulse) + '</div>'
      + '<div data-account-part="activities">' + renderTodayHtml(pulse) + '</div>';
  }

  function renderAccountHtml(pulse) {
    const tab = side => '<button type="button" role="tab" class="pulse-account-tab"'
      + ' id="pulse-account-tab-' + side + '" aria-controls="pulse-account-panel-' + side + '"'
      + ' aria-selected="' + (selectedSide === side) + '" tabindex="' + (selectedSide === side ? '0' : '-1') + '"'
      + ' data-account-tab="' + side + '">' + side + '</button>';
    const panel = (side, inner) => '<div class="pulse-account-panel" role="tabpanel"'
      + ' id="pulse-account-panel-' + side + '" aria-labelledby="pulse-account-tab-' + side + '"'
      + ' data-account-panel="' + side + '"' + (selectedSide === side ? '' : ' hidden') + '>' + inner + '</div>';
    return '<section class="pulse-account" id="pulse-account" data-home-surface="account">'
      + '<div class="pulse-account-head">'
      + '<div class="pulse-account-tabs" role="tablist" aria-label="which day">' + ACCOUNT_SIDES.map(tab).join('') + '</div>'
      + '<span class="pulse-account-day" data-account-day><span data-account-day-text>' + esc(accountDayText(selectedSide, pulse)) + '</span></span>'
      + '</div>'
      + panel('yesterday', renderYesterdaySideHtml(briefingData, pulse))
      + panel('today', renderTodaySideHtml(pulse))
      + '</section>';
  }

  function selectAccountSide(side, moveFocus) {
    if (ACCOUNT_SIDES.indexOf(side) === -1 || !surface) return;
    selectedSide = side;
    surface.querySelectorAll('[data-account-tab]').forEach(tab => {
      const on = tab.dataset.accountTab === side;
      tab.setAttribute('aria-selected', String(on));
      tab.setAttribute('tabindex', on ? '0' : '-1');
    });
    surface.querySelectorAll('[data-account-panel]').forEach(panel => {
      if (panel.dataset.accountPanel === side) panel.removeAttribute('hidden');
      else panel.setAttribute('hidden', '');
    });
    updateAccountDay();
    if (moveFocus) document.getElementById('pulse-account-tab-' + side)?.focus();
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
    if (neighbor.latest_kind !== 'mentioned') return '';
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
    // Nothing yet is no card at all: sections appear as material arrives.
    if (connections.state === 'empty') return '';
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

  // Connections and your week stand side by side under the account.
  function renderPairHtml(pulse) {
    const parts = [renderConnectionsHtml(pulse), renderWeeklyReflectionHtml(pulse)].filter(Boolean);
    if (!parts.length) return '';
    return '<div class="pulse-pair' + (parts.length === 2 ? ' pulse-pair--two' : '') + '" data-home-surface="pair">'
      + parts.join('') + '</div>';
  }

  function renderPair(pulse) {
    replaceHomeSurface('pair', renderPairHtml(pulse), predecessors('pair'));
  }

  function normalizeBriefingFromPulse(pulse) {
    return normalizeBriefing({
      exists: pulse.briefing_exists,
      phase: pulse.briefing_phase,
      saved_lane: pulse.briefing_saved_lane,
      analysis_day: pulse.briefing_analysis_day,
      meta: pulse.briefing_meta,
      sections: pulse.briefing_sections
    });
  }

  function normalizeBriefing(data) {
    return {
      exists: Boolean(data.exists),
      phase: data.phase || 'eod',
      saved_lane: data.saved_lane ?? null,
      analysis_day: typeof data.analysis_day === 'string' ? data.analysis_day : null,
      meta: isPlainObject(data.meta) ? data.meta : null,
      sections: isPlainObject(data.sections) ? data.sections : {}
    };
  }

  // The briefing's stamp is the journal's own wall time with no offset. Show
  // its hour and minute as written: parsing it as an instant would shift it
  // by the browser's zone.
  function writtenAtTime(generated) {
    const match = /T(\d{2}):(\d{2})/.exec(String(generated || ''));
    if (!match || Number(match[1]) > 23 || Number(match[2]) > 59) return '';
    return new Date(2000, 0, 1, Number(match[1]), Number(match[2]))
      .toLocaleString(undefined, { hour: 'numeric', minute: '2-digit' });
  }

  // Before the nightly run has had its first chance, say when the first
  // account is due rather than calling it late or missing.
  const BRIEFING_NOT_YET = Object.assign(Object.create(null), {
    awaiting_engine: 'yesterday shows here once you choose where the thinking happens.',
    first_night: "your first day shows here by 10 am, after your journal's first night.",
    first_night_empty: 'yesterday shows here the morning after something goes into your journal.'
  });

  const BRIEFING_AWAITING_ENGINE_LANES = Object.assign(Object.create(null), {
    byo: 'yesterday shows here once your own model is set up.',
    spp: 'yesterday shows here once confidential processing is set up.',
    local: 'yesterday shows here once local thinking is set up.'
  });

  function briefingRunHref(data, pulseContext) {
    return '/app/thinking/#runs/' + encodeURIComponent(data?.analysis_day || pulseContext?.briefing_analysis_day || '') + '/morning_briefing';
  }

  // The yesterday side never renders empty. Silence is the worst failure mode.
  function briefingPlaceholderHtml(data, pulseContext) {
    if (data.exists) return '';
    const run = esc(briefingRunHref(data, pulseContext));
    if (data.phase === 'awaiting_engine') {
      const text = BRIEFING_AWAITING_ENGINE_LANES[data.saved_lane] || BRIEFING_NOT_YET.awaiting_engine;
      return '<div class="pulse-briefing-placeholder">' + esc(text) + '</div>';
    }
    const notYet = BRIEFING_NOT_YET[data.phase];
    if (notYet) return '<div class="pulse-briefing-placeholder">' + esc(notYet) + '</div>';
    if (data.phase === 'pending') {
      const lateness = pulseContext?.briefing_lateness || {};
      if (lateness.late) {
        return '<div class="pulse-briefing-placeholder">'
          + "yesterday is usually written up by 10 am. it's " + esc(lateness.late_hours || 0) + 'h late.'
          + '<a class="pulse-briefing-status-link" href="' + run + '">check status</a>'
          + '</div>';
      }
      return '<div class="pulse-briefing-placeholder">yesterday is being written up…</div>';
    }
    // Missing, and any hour with no account that no not-yet state explains.
    return '<div class="pulse-briefing-placeholder">'
      + "yesterday wasn't written up. "
      + '<a class="pulse-briefing-status-link" href="' + run + '">see the run →</a>'
      + '</div>';
  }

  function renderYesterdaySideHtml(data, pulseContext) {
    if (!data) return '';
    let body;
    if (data.exists) {
      const raw = typeof data.sections.yesterday === 'string' ? data.sections.yesterday : '';
      // An empty section carries no time: "written at" beside "nothing was
      // written up" would contradict it.
      const time = raw ? writtenAtTime(data.meta?.generated) : '';
      body = (raw
        ? '<div class="pulse-briefing-body" id="pulse-briefing-body">' + markdown(raw) + '</div>'
        : '<div class="pulse-briefing-placeholder">nothing from yesterday was written up.</div>')
        + '<div class="pulse-account-meta">'
        + (time ? '<span>written at ' + esc(time) + '</span>' : '')
        + '<a class="pulse-tell-more" href="' + esc(briefingRunHref(data, pulseContext)) + '">how this was written →</a>'
        + '</div>';
    } else {
      body = briefingPlaceholderHtml(data, pulseContext);
    }
    return '<div class="pulse-briefing" id="pulse-briefing" data-phase="' + esc(data.phase) + '">' + body + '</div>';
  }

  function yesterdayPanel() {
    return surface?.querySelector?.('#pulse-account-panel-yesterday') || null;
  }

  // A briefing read updates the yesterday side in place and the waiting list
  // with it: an account that arrives takes its missing-account row away.
  function renderBriefing(data, pulseContext = lastPulse) {
    validateBriefing(data);
    briefingData = normalizeBriefing(data);
    replaceAccountContent(yesterdayPanel(), renderYesterdaySideHtml(briefingData, pulseContext));
    if (selectedSide === 'yesterday') updateAccountDay();
    if (lastPulse) renderVitals(lastPulse);
  }

  function renderBriefingError(error) {
    const panel = yesterdayPanel();
    if (!panel) return;
    replaceAccountContent(panel, '<div class="pulse-briefing" id="pulse-briefing" data-phase="error">'
      + surfaceError(error, "couldn't load yesterday.")
      + '</div>');
    const retry = panel.querySelector('.surface-state-retry');
    if (retry) {
      retry.addEventListener('click', function () {
        refreshBriefing();
      });
    }
  }

  // The welcome page has no account to update in place: once the journal
  // has something to show, the page is painted afresh.
  function leavesWelcome(data) {
    return lastPulse?.home_state === 'welcome' && data.home_state !== 'welcome';
  }

  // The originals list's item total, for the waiting list's originals row.
  // Read once per mount, and again when the browser restores the page from
  // its back/forward cache; never polled.
  function readOriginalsTotal() {
    if (originalsRequest) return originalsRequest;
    originalsRequest = apiJson(ORIGINALS_URL)
      .then(function (data) {
        const page = isPlainObject(data) && Number.isSafeInteger(data.total_rows) && data.total_rows >= 0
          && ['list.ready', 'list.empty', 'list.review_too_large'].indexOf(data.state) !== -1;
        originalsTotal = page ? (data.state === 'list.empty' ? 0 : data.total_rows) : null;
      })
      .catch(function () {
        originalsTotal = null;
      })
      .finally(function () {
        originalsRequest = null;
        if (lastPulse) renderVitals(lastPulse);
      });
    return originalsRequest;
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
        if (!lastPulse || leavesWelcome(data)) { renderPulse(data); return; }
        lastPulse = Object.assign({}, lastPulse || {}, data);
        pulseReadFailed = false;
        pulseObservedAt = Date.now();
        observeBackup(data);
        renderPair(data);
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
        if (lastPulse && !leavesWelcome(data)) {
          renderNarrative(data);
          clearPulseRefreshError('pulse-narrative');
        }
        if (read < pulseAppliedVersion) return;
        pulseAppliedVersion = read;
        if (!lastPulse || leavesWelcome(data)) { renderPulse(data); return; }
        lastPulse = Object.assign({}, lastPulse || {}, data);
        pulseReadFailed = false;
        pulseObservedAt = Date.now();
        observeBackup(data);
        renderVitals(data);
        renderPair(data);
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

  function closest(target, selector) {
    return target && typeof target.closest === 'function' ? target.closest(selector) : null;
  }

  function handleDashboardClick(event) {
    const target = event.target;
    const tab = closest(target, '[data-account-tab]');
    if (tab) {
      event.preventDefault();
      selectAccountSide(tab.dataset.accountTab, false);
      return;
    }
    const showAll = closest(target, '[data-activities-show-all]');
    if (showAll) {
      event.preventDefault();
      todayActivitiesExpanded = true;
      if (lastPulse) renderToday(lastPulse);
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

  // The day switch is a tab list: arrow keys, Home and End move the selection.
  function handleDashboardKeydown(event) {
    const tab = closest(event.target, '[data-account-tab]');
    if (!tab) return;
    const index = ACCOUNT_SIDES.indexOf(tab.dataset.accountTab);
    let next = null;
    if (event.key === 'ArrowRight' || event.key === 'ArrowDown') next = ACCOUNT_SIDES[(index + 1) % ACCOUNT_SIDES.length];
    else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') next = ACCOUNT_SIDES[(index + ACCOUNT_SIDES.length - 1) % ACCOUNT_SIDES.length];
    else if (event.key === 'Home') next = ACCOUNT_SIDES[0];
    else if (event.key === 'End') next = ACCOUNT_SIDES[ACCOUNT_SIDES.length - 1];
    if (!next) return;
    event.preventDefault();
    selectAccountSide(next, true);
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
    // The originals page is the originals list alone; it reads nothing else
    // home reads.
    if (pathname.startsWith(ORIGINALS_PAGE)) {
      surface.innerHTML = '';
      return;
    }
    isFirstPage = true;
    wireInteractions();
    wireRealtime();
    loadPulse();
    readFirstMinutes();
    readAgents();
    readOriginalsTotal();
  }

  document.addEventListener('visibilitychange', function () {
    if (isFirstPage && !firstMinutesClosed) {
      readAgents();
    }
  });

  // Back from the originals page can restore this page from the browser's
  // cache without mounting it again, so read the total once more.
  window.addEventListener('pageshow', function (event) {
    if (isFirstPage && event && event.persisted) readOriginalsTotal();
  });

  window.selectAccountSide = selectAccountSide;

  document.addEventListener('workspace:mounted', function (event) {
    if (event?.detail?.appName !== 'home') return;
    initHome();
  });
})();
