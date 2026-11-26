// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  // Home on the view contract: everything below runs when home is mounted,
  // and what it adds to the page goes when it is disposed. Home reads again
  // on the stream's reconnect by itself (connectionRecovery).
  window.SolViews.register('home', { rereadsOnReconnect: true, mount: mountHome });

  function accountTimeFromFragment() {
    return window.location.hash?.match(/^#today=(\d{4}-\d{2}-\d{2}T\d{2}:\d{2})$/)?.[1];
  }

  function mountHome(ctx) {
    const reportHomeReady = ctx.ready;

    const accountTime = accountTimeFromFragment();
    const accountQuery = accountTime ? '?today=' + accountTime : '';
    const PULSE_URL = '/app/home/api/pulse' + accountQuery;
    const BRIEFING_URL = '/app/home/api/briefing' + accountQuery;
    const FIRST_MINUTES_URL = '/app/home/api/first-minutes';
    const AGENTS_STATE_URL = '/app/agents/api/state';
    const SHELL_BADGES_URL = '/api/shell/badges';
    const SYSTEM_RESTART_URL = '/api/system/restart';
    const SETUP_ACTION_URL = '/init/api/action';
    const ORIGINALS_PAGE = '/app/home/originals/';
    // The page's sections in the order they stand. A live refresh that brings a
    // section back inserts it after whichever of these is already there.
    const SURFACE_ORDER = ['vitals', 'ready-to-shape', 'getting-started', 'account', 'pair', 'welcome'];
    const ACCOUNT_SIDES = ['today', 'yesterday'];

    const SERVICE_NAMES = {
      supervisor: window.JournalI18n.render('journal.window.health.service.names.supervisor.system.manager'),
      convey: window.JournalI18n.render('journal.window.health.service.names.convey.web.interface'),
      cortex: window.JournalI18n.render('journal.window.health.service.names.cortex.model.runtime'),
      sense: window.JournalI18n.render('journal.window.health.service.names.sense.media.processor'),
      observe: window.JournalI18n.render('journal.window.health.service.names.observe.screen.audio'),
      think: window.JournalI18n.render('journal.window.health.service.names.think.background.analysis'),
      importer: window.JournalI18n.render('journal.window.health.service.names.importer.file.importer'),
      schedule: window.JournalI18n.render('journal.window.health.service.names.schedule.task.scheduler'),
      spl: window.JournalI18n.render('journal.window.health.service.names.spl.private.network'),
      parakeet: window.JournalI18n.render('journal.window.health.service.names.parakeet.transcription'),
    };

    const PIN_OFFER_KEYS = {
      body: 'journal.window.pin.offer.body.text',
      places: 'journal.window.pin.offer.map.text',
      location: 'journal.window.pin.offer.map.location.text',
      unnamed: 'journal.window.pin.offer.speakers.text',
      named: 'journal.window.pin.offer.speakers.named.text',
      experiences: 'journal.window.pin.offer.experiences.text',
    };

    let homeInitialized = false;
    let interactionsWired = false;
    let realtimeWired = false;
    let root = null;
    let surface = null;
    let lastPulse = null;
    let lastBackupIssue = null;
    let backupObservedAt = null;
    let shellBadges = isPlainObject(window.solShellBadges) ? window.solShellBadges : null;
    let shellBadgesFallbackRequested = false;
    let securityState = null;
    let securityRestartInFlight = false;
    let securityRestartError = '';
    let lastSecurityPayload = null;
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
    let isFirstPage = false;
    const ACTIVITY_PREVIEW = 6;
    let todayActivitiesExpanded = false;
    let yesterdayActivitiesDay = null;
    let yesterdayActivitiesData = null;
    let yesterdayActivitiesExpanded = false;
    let yesterdayActivitiesRequest = null;

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

    // Home's reads belong to this mount, through the page's read store. Home
    // keeps one read of each in flight itself (pulseRequest and the rest), so
    // each one it starts is a read of its own.
    function apiJson(url) {
      return ctx.read(url, { fresh: true });
    }

    // Initial paint and live updates share the same observation. Event bursts
    // must not queue whole-journal reads while that observation is in flight.
    // Whether home's last pulse or briefing read failed: a page that opened into
    // an outage reads again on the stream's first connect (connectionRecovery).
    let lastReadFailed = false;

    function readPulse() {
      if (!pulseRequest) {
        pulseRequest = apiJson(PULSE_URL).then(
          function (data) { lastReadFailed = false; return data; },
          function (error) { lastReadFailed = true; throw error; },
        ).finally(function () { pulseRequest = null; });
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
        return window.SurfaceState.loading({ text, busy: false });
      }
      return '<div class="surface-state surface-state--loading" role="status">'
        + '<div class="surface-state-spinner" aria-hidden="true"></div>'
        + '<span class="surface-state-text" data-role="loading-status">' + esc(text) + '</span>'
        + `<span class="surface-state-still" data-role="still-working" aria-hidden="true">${window.JournalI18n.renderHtml('journal.window.chrome.still.working')}</span>`
        + '</div>';
    }

    function surfaceError(error, heading) {
      if (window.SurfaceState && typeof window.SurfaceState.error === 'function') {
        return window.SurfaceState.error({
          heading,
          desc: window.CONVEY_COPY?.RELOAD_HINT || window.JournalI18n.render('journal.window.home.window.explanation.reload.to.try.again'),
          retry: true,
          serverMessage: window.SurfaceState.serverMessageFrom(error),
          detail: error
        });
      }
      return '<div class="surface-state surface-state--error" role="alert">'
        + '<h2 class="surface-state-heading">' + esc(heading) + '</h2>'
        + `<button type="button" class="surface-state-retry">${window.JournalI18n.renderHtml('journal.window.home.button.label.try.again')}</button>`
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
        throw malformedHomeResponse(PULSE_URL, window.JournalI18n.render('journal.window.home.response.pulse.invalid'));
      }
      if (!Object.prototype.hasOwnProperty.call(data, 'narrative_content')) {
        throw malformedHomeResponse(PULSE_URL, window.JournalI18n.render('journal.window.home.response.pulse.invalid'));
      }
      if (!Array.isArray(data.needs_you_items)) {
        throw malformedHomeResponse(PULSE_URL, window.JournalI18n.render('journal.window.home.response.pulse.invalid'));
      }
    }

    function validateBriefing(data) {
      if (!isPlainObject(data) || typeof data.exists !== 'boolean' || typeof data.phase !== 'string') {
        throw malformedHomeResponse(BRIEFING_URL, window.JournalI18n.render('journal.window.home.response.briefing.invalid'));
      }
      if (data.exists && !isPlainObject(data.sections)) {
        throw malformedHomeResponse(BRIEFING_URL, window.JournalI18n.render('journal.window.home.response.briefing.invalid'));
      }
    }

    function setSurfaceHtml(html) {
      if (!surface) return;
      surface.innerHTML = html;
      keepGettingStartedFirst();
    }

    // The first-minutes card leads the story cards right after status (and
    // ready-to-shape when present), whichever read finished first.
    function keepGettingStartedFirst() {
      if (!surface || typeof surface.querySelector !== 'function') return;
      const card = surface.querySelector('[data-home-surface="getting-started"]');
      const anchor = surface.querySelector('[data-home-surface="ready-to-shape"]')
        || surface.querySelector('[data-home-surface="vitals"]');
      if (!card || !anchor) return;
      if (typeof anchor.after === 'function') {
        if (anchor.nextElementSibling === card) return;
        anchor.after(card);
        return;
      }
      if (anchor.parentNode && typeof anchor.parentNode.insertBefore === 'function') {
        const siblings = anchor.parentNode.children;
        const idx = siblings && typeof siblings.indexOf === 'function' ? siblings.indexOf(anchor) : -1;
        const next = idx >= 0 ? (siblings[idx + 1] || null) : (anchor.nextSibling || null);
        if (next === card) return;
        anchor.parentNode.insertBefore(card, next);
      }
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
          desc: window.CONVEY_COPY?.RELOAD_HINT || window.JournalI18n.render('journal.window.home.window.explanation.reload.to.try.again'),
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
      surface.innerHTML = surfaceLoading(window.JournalI18n.render('journal.window.loading.home'));
      const read = ++pulseReadVersion;
      return readPulse()
        .then(function (data) {
          if (read < pulseAppliedVersion) return;
          pulseAppliedVersion = read;
          renderPulse(data);
          reportHomeReady(data.home_state === 'welcome' ? 'empty' : 'content');
        })
        .catch(function (error) {
          if (read < pulseAppliedVersion) return;
          pulseAppliedVersion = read;
          // The early briefing read was for this first page. A retry reads a
          // fresh one rather than draw what was read before the failure.
          earlyBriefing = null;
          logError(error, 'home: loadPulse failed');
          surface.innerHTML = surfaceError(error, window.JournalI18n.render('journal.window.home.response.pulse.failed'));
          bindSurfaceRetry();
          reportHomeReady('error');
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

    function currentShellBadges() {
      if (isPlainObject(shellBadges)) return shellBadges;
      if (isPlainObject(window.solShellBadges)) return window.solShellBadges;
      return null;
    }

    function serviceDisplayName(internal) {
      return SERVICE_NAMES[internal] || String(internal || '').replace(/[_:-]+/g, ' ');
    }

    function statusMarkSpec(reconciledGlance) {
      const shellStatus = isPlainObject(window.solShellStatus) ? window.solShellStatus : null;
      const base = typeof shellStatus?.statusMarksBase === 'string' && shellStatus.statusMarksBase
        ? shellStatus.statusMarksBase
        : '/static/sol-status/';
      let stem = typeof shellStatus?.stem === 'string' && /^[a-z0-9-]+$/i.test(shellStatus.stem)
        ? shellStatus.stem
        : 'mark-connecting';
      const glance = reconciledGlance || (lastPulse ? homeHealthGlance(lastPulse).glance : null);
      const hasUrgentAttention = Boolean(
        glance && glance.verdict === 'attention' && Array.isArray(glance.issues) && glance.issues.length > 0
      );
      if (hasUrgentAttention && (
        stem === 'mark'
        || stem === 'mark-paused'
        || stem === 'mark-connecting'
        || stem === 'mark-connecting-animated'
      )) {
        stem = 'mark-attention';
      }
      return { src: base + stem + '.svg' };
    }

    function syncStatusMarkIcon() {
      const vitals = document.getElementById?.('pulse-vitals');
      const img = vitals?.querySelector?.('[data-home-status-mark]')
        || document.querySelector?.('[data-home-status-mark]');
      if (!img) return;
      const { src } = statusMarkSpec();
      if (img.getAttribute?.('src') !== src) {
        img.hidden = false;
        img.setAttribute?.('src', src);
      }
      if (!img.__errorBound && typeof img.addEventListener === 'function') {
        img.__errorBound = true;
        img.addEventListener('error', function () {
          img.hidden = true;
        });
      }
    }

    function syncHomePinOffer(pulse) {
      const host = document.getElementById?.('home-pin-offer');
      if (!host) return;
      if (typeof window.solShellChrome?.mountPinOffer !== 'function'
          || typeof window.solShellChrome?.shouldShowPinOffer !== 'function') {
        host.replaceChildren?.();
        host.hidden = true;
        return;
      }
      const candidates = Array.isArray(pulse?.pin_candidates)
        ? pulse.pin_candidates.filter(isPlainObject)
        : [];
      const chosen = candidates.find(function (entry) {
        return typeof entry.view === 'string'
          && PIN_OFFER_KEYS[entry.kind]
          && window.solShellChrome.shouldShowPinOffer(entry.view);
      });
      if (!chosen) {
        host.replaceChildren?.();
        host.hidden = true;
        return;
      }
      host.hidden = false;
      window.solShellChrome.mountPinOffer(host, {
        view: chosen.view,
        message: window.JournalI18n.render(PIN_OFFER_KEYS[chosen.kind]),
      });
    }

    function syncHomeLiveBadge(pulse) {
      if (typeof window.solShellChrome?.setHomeBadgeDelta !== 'function') return;
      if (!pulse) {
        window.solShellChrome.setHomeBadgeDelta(null);
        return;
      }
      const workCurrent = heldWorkCurrent && heldWorkSnapshot
        && Date.now() - heldWorkSnapshot.observedAt < 30000;
      const liveIssues = [];
      if (workCurrent) {
        crashedServiceIssues().forEach(function (issue) { liveIssues.push(issue); });
        heldWorkIssues().forEach(function (issue) { liveIssues.push(issue); });
      }
      if (!pulseReadFailed) {
        nightRows(pulse).forEach(function (row) {
          liveIssues.push({ text: row.text, severity: 'amber', href: row.href });
        });
        damagedVoiceIssues(pulse).forEach(function (issue) { liveIssues.push(issue); });
      }
      const workGrace = !queuedWorkAuthoritySeen && Date.now() - pageLoadedAt < 15000;
      const liveUncertain = Boolean(pulseReadFailed || (!workCurrent && !workGrace) || securityState === 'unconfirmed');
      const severity = liveIssues.length === 0
        ? null
        : (liveIssues.some(function (issue) { return issue.severity === 'red'; }) ? 'red' : 'amber');
      window.solShellChrome.setHomeBadgeDelta({
        issues: liveIssues.length,
        severity,
        uncertain: liveUncertain,
      });
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
      observeOriginalsTotal(data);
      briefingData = normalizeBriefingFromPulse(data);
      const welcome = data.home_state === 'welcome';
      paintedSide = welcome ? null : (dayHasMaterial(data) ? 'today' : 'yesterday');
      selectedSide = paintedSide;
      const cards = [renderVitalsHtml(data)];
      const shapeHtml = renderReadyToShapeHtml(data);
      if (shapeHtml) cards.push(shapeHtml);
      if (isGettingStartedVisible()) cards.push(renderGettingStartedHtml());
      if (welcome) {
        earlyBriefing = null;
        cards.push(renderWelcomeHtml());
        setSurfaceHtml(cards.join(''));
        syncStatusMarkIcon();
        syncHomePinOffer(data);
        syncHomeLiveBadge(data);
        reportHomeReady('empty');
        return;
      }
      cards.push(renderAccountHtml(data));
      cards.push(renderPairHtml(data));
      setSurfaceHtml(cards.join(''));
      syncStatusMarkIcon();
      syncHomePinOffer(data);
      syncHomeLiveBadge(data);
      placeAccountZoneNote();
      if (selectedSide === 'yesterday') syncYesterdayActivities();
      refreshBriefing();
      reportHomeReady('content');
    }

    function crashedServiceIssues() {
      if (!heldWorkSnapshot || !Array.isArray(heldWorkSnapshot.crashed)) return [];
      return heldWorkSnapshot.crashed
        .filter(function (entry) { return isPlainObject(entry) && typeof entry.name === 'string' && entry.name.trim(); })
        .map(function (entry) {
          const label = serviceDisplayName(entry.name.trim());
          const retrying = entry.phase === 'backoff';
          return {
            text: window.JournalI18n.render(
              retrying
                ? 'journal.window.home.status.service.restarting'
                : 'journal.window.home.status.service.stopped',
              [['service', label]],
            ),
            severity: retrying ? 'amber' : 'red',
            href: '/app/health/',
          };
        });
    }

    function heldWorkIssues() {
      if (!heldWorkSnapshot) return [];
      const queues = heldWorkSnapshot.queues;
      const reasons = {
        daily: 'journal.window.home.held.daily.safety.check',
        activity: 'journal.window.home.held.activity.safety.check',
        segment: 'journal.window.home.held.segment.safety.check',
        flush: 'journal.window.home.held.flush.safety.check',
        weekly: 'journal.window.home.held.weekly.safety.check',
        indexer: 'journal.window.home.held.search.safety.check'
      };
      const messages = heldWorkSnapshot.held.filter(hold => queues[hold.partition] > 0)
        .map(hold => reasons[hold.partition] || 'journal.window.home.held.queued.safety.check');
      if (heldWorkSnapshot.queue_hold && Object.values(queues).some(count => count > 0)) {
        messages.unshift('journal.window.home.held.all.safety.check');
      }
      return messages.map(id => ({ text: window.JournalI18n.render(id),
        severity: 'amber', href: '/app/health/#processingHolds' }));
    }

    function observeBackup(pulse) {
      if (pulse.backup?.available !== true || typeof pulse.backup.issue !== 'boolean') return;
      backupObservedAt = Date.now();
      if (pulse.backup.issue !== true) {
        lastBackupIssue = null;
        return;
      }
      const key = pulse.backup.reason === 'auth_failed'
        ? 'journal.window.home.status.backup.auth.failed'
        : 'journal.window.home.status.backup.attention';
      lastBackupIssue = {
        text: window.JournalI18n.render(key),
        severity: 'amber',
        href: '/app/health/#backup',
      };
    }

    function damagedVoiceIssues(pulse) {
      const items = Array.isArray(pulse?.needs_you_items) ? pulse.needs_you_items : [];
      const out = [];
      items.forEach(function (item) {
        if (!isPlainObject(item) || item.source_id !== 'owner_voice:damaged') return;
        const text = typeof item.text === 'string' ? item.text.trim() : '';
        if (!text) return;
        const href = item.kind === 'route' && typeof item.payload?.href === 'string'
          && item.payload.href.startsWith('/') && !item.payload.href.startsWith('//')
          ? item.payload.href
          : '/app/speakers/';
        out.push({ text, severity: 'amber', href });
      });
      return out;
    }

    function homeHealthGlance(pulse) {
      const g = Object.assign({}, pulse.health_glance || {});
      const nightIssues = nightRows(pulse).map(function (row) {
        return { text: row.text || '', severity: 'amber', href: row.href || '/app/thinking/' };
      });
      const baseIssues = (g.issues || []).concat(nightIssues).concat(damagedVoiceIssues(pulse));
      const crashed = crashedServiceIssues();
      const holds = heldWorkIssues();
      const workCurrent = heldWorkCurrent && heldWorkSnapshot
        && Date.now() - heldWorkSnapshot.observedAt < 30000;
      const backupCurrent = !pulseReadFailed && pulse.backup?.available === true
        && typeof pulse.backup.issue === 'boolean';
      const backupIssues = lastBackupIssue ? [lastBackupIssue] : [];
      const current = (pulseReadFailed ? [] : baseIssues)
        .concat(workCurrent ? crashed : [])
        .concat(workCurrent ? holds : [])
        .concat(backupCurrent ? backupIssues : []);
      const retained = issue => Object.assign({}, issue, { text: window.JournalI18n.render('journal.window.home.vitals.last.known', [['issue', String(issue.text)]]), severity: 'amber' });
      g.issues = baseIssues.map(issue => pulseReadFailed ? retained(issue) : issue)
        .concat(crashed.map(issue => workCurrent ? issue : retained(issue)))
        .concat(holds.map(issue => workCurrent ? issue : retained(issue)))
        .concat(backupIssues.map(issue => backupCurrent ? issue : retained(issue)));
      const notes = [];
      const checked = at => at ? window.JournalI18n.render('journal.window.home.vitals.last.check.timestamp', [['checked_at', new Date(at).toLocaleString(window.JournalI18n.formatLocale)]]) : '';
      if (pulseReadFailed) notes.push({text: [window.JournalI18n.render('journal.window.home.text.service.status.is.unavailable'), checked(pulseObservedAt)].filter(Boolean).join(' '), href:'/app/health/'});
      const workGrace = !queuedWorkAuthoritySeen && Date.now() - pageLoadedAt < 15000;
      if (!workCurrent && !workGrace) notes.push({text: [window.JournalI18n.render('journal.window.home.text.queued.work.status.is.unavailable'), checked(heldWorkSnapshot?.observedAt)].filter(Boolean).join(' '), href:'/app/health/#processingHolds'});
      const backupUnavailable = !backupCurrent && (pulse.backup || backupObservedAt);
      if (backupUnavailable) notes.push({text: [window.JournalI18n.render('journal.window.home.text.backup.status.is.unavailable'), checked(backupObservedAt)].filter(Boolean).join(' '), href:'/app/health/#backup'});
      const securityUnconfirmed = securityState === 'unconfirmed';
      if (securityUnconfirmed) {
        notes.push({
          text: window.JournalI18n.render('journal.window.shared.status.pane.visible.text.couldn.t.check.for.security.updates'),
          href: '/app/health/#healthSystemDetails',
        });
      }
      if (current.length) {
        g.verdict = 'attention';
        g.severity = current.some(issue => issue.severity === 'red') ? 'red' : 'amber';
        g.headline = window.JournalI18n.render('journal.window.home.health.attention.count', [['quantity', window.JournalI18n.cardinalQuantity(g.issues.length)], ['count', String(g.issues.length)]]);
      } else if (notes.length) {
        g.verdict = 'unavailable';
        g.severity = 'amber';
        g.headline = pulseReadFailed ? window.JournalI18n.render('journal.window.home.window.explanation.service.status.is.unavailable.right.now')
          : (!workCurrent && !workGrace) ? window.JournalI18n.render('journal.window.home.window.explanation.queued.work.status.is.unavailable.right')
          : backupUnavailable ? window.JournalI18n.render('journal.window.home.window.explanation.backup.status.is.unavailable.right.now')
          : window.JournalI18n.render('journal.window.home.status.headline.uncertain');
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
      const day = night.analysis_day || pulse.briefing_analysis_day;
      const runs = '/app/thinking/#runs/' + encodeURIComponent(day || '');
      const accountRun = runs + '/morning_briefing';
      const links = (Array.isArray(night.gap_links) ? night.gap_links : []).filter(isPlainObject);
      // The missing account and the briefing's own failed run are one event:
      // say it once, as the account.
      const isAccount = link => link.href === accountRun || String(link.href || '').startsWith(accountRun + '/');
      const accountLinks = links.filter(isAccount);
      const rows = [];
      const account = accountLinks.find(link => link.href === accountRun) || accountLinks[0];
      const sameDay = day === pulse.briefing_analysis_day;
      if (account && (!sameDay || (paintedSide !== 'yesterday' && !briefingData?.exists))) {
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

    function originalsShapeRow() {
      if (originalsTotal === undefined || originalsTotal === 0) return null;
      if (originalsTotal === null) {
        return {
          text: window.JournalI18n.render('journal.window.home.text.originals.the.list.couldn.t.be'),
          href: ORIGINALS_PAGE,
        };
      }
      return {
        text: window.JournalI18n.render('journal.window.home.shape.originals.count', [
          ['quantity', window.JournalI18n.cardinalQuantity(originalsTotal)],
          ['count', String(originalsTotal)],
        ]),
        href: ORIGINALS_PAGE,
      };
    }

    function readyToShapeRows(pulse) {
      const shapeRows = [];
      const badges = currentShellBadges();
      const curationCount = Number.isFinite(badges?.curation) ? Math.max(0, Math.floor(badges.curation)) : 0;
      if (curationCount > 0) {
        shapeRows.push({
          text: window.JournalI18n.render('journal.window.home.shape.suggestions.count', [
            ['quantity', window.JournalI18n.cardinalQuantity(curationCount)],
            ['count', String(curationCount)],
          ]),
          href: '/app/curation/',
        });
      }
      const duplicatesCount = Number.isFinite(pulse?.possible_duplicates_count)
        ? Math.max(0, Math.floor(pulse.possible_duplicates_count))
        : 0;
      if (duplicatesCount > 0) {
        shapeRows.push({
          text: window.JournalI18n.render('journal.window.home.shape.duplicates.count', [
            ['quantity', window.JournalI18n.cardinalQuantity(duplicatesCount)],
            ['count', String(duplicatesCount)],
          ]),
          href: '/app/entities/?folder=duplicates',
        });
      }
      const originals = originalsShapeRow();
      if (originals) shapeRows.push(originals);
      (Array.isArray(pulse?.needs_you_items) ? pulse.needs_you_items : []).forEach(function (item) {
        if (!item) return;
        if (isPlainObject(item) && item.source_id === 'owner_voice:damaged') return;
        if (isPlainObject(item) && item.source_id === 'owner_voice:candidate_ready') {
          shapeRows.push({
            text: window.JournalI18n.render('journal.window.home.shape.voice.candidate'),
            href: '/app/speakers/#your-voice',
          });
          return;
        }
        if (isPlainObject(item) && item.source_id === 'owner_voice:not_established') {
          shapeRows.push({
            text: window.JournalI18n.render('journal.window.home.shape.voice.teach'),
            href: '/app/speakers/#your-voice',
          });
          return;
        }
        const row = needsRow(item);
        if (row) shapeRows.push(row);
      });
      return shapeRows;
    }

    function shapeRowHtml(row) {
      if (row.href) {
        return '<li class="pulse-shape-row"><a href="' + esc(row.href) + '"><span>' + esc(row.text) + '</span></a></li>';
      }
      const reason = row.reason ? '<span class="pulse-waiting-reason">' + esc(row.reason) + '</span>' : '';
      return '<li class="pulse-shape-row"><span>' + esc(row.text) + reason + '</span></li>';
    }

    function renderReadyToShapeHtml(pulse) {
      const shapeRows = readyToShapeRows(pulse);
      if (!shapeRows.length) return '';
      return '<section class="pulse-ready-to-shape" id="pulse-ready-to-shape" data-home-surface="ready-to-shape" aria-labelledby="pulse-shape-heading">'
        + '<h2 class="pulse-shape-heading" id="pulse-shape-heading">'
        + window.JournalI18n.renderHtml('journal.window.home.shape.heading') + '</h2>'
        + '<ul class="pulse-shape-list" aria-labelledby="pulse-shape-heading">'
        + shapeRows.map(shapeRowHtml).join('') + '</ul>'
        + '</section>';
    }

    function renderReadyToShape(pulse) {
      if (!surface) return;
      const html = renderReadyToShapeHtml(pulse);
      const current = surface.querySelector?.('[data-home-surface="ready-to-shape"]');
      if (!html && !current) return;
      if (current && current.outerHTML === html) return;
      replaceHomeSurface('ready-to-shape', html, predecessors('ready-to-shape'));
    }

    function durationPartsFromMs(ms) {
      let seconds = Math.floor(ms / 1000);
      if (!Number.isFinite(seconds) || seconds < 0) seconds = 0;
      if (seconds < 60) return { unit: 'second', count: seconds };
      const minutes = Math.floor(seconds / 60);
      if (minutes < 60) return { unit: 'minute', count: minutes };
      const hours = Math.floor(minutes / 60);
      if (hours < 24) return { unit: 'hour', count: hours };
      const days = Math.floor(hours / 24);
      if (days < 7) return { unit: 'day', count: days };
      if (days < 28) return { unit: 'week', count: Math.floor(days / 7) };
      if (days < 60) return { unit: 'month', count: 1 };
      return { unit: 'month', count: Math.floor(days / 30) };
    }

    function formatAgoMs(ms) {
      if (!Number.isFinite(ms) || ms < 45000) {
        return window.JournalI18n.render('journal.window.home.health.age.just.now');
      }
      const { unit, count } = durationPartsFromMs(ms);
      return window.JournalI18n.render('journal.window.home.time.ago.' + unit, [
        ['quantity', window.JournalI18n.cardinalQuantity(count)],
        ['count', String(count)],
      ]);
    }

    function formatQuietDuration(ms) {
      if (!Number.isFinite(ms) || ms < 45000) {
        return window.JournalI18n.render('journal.window.home.health.age.just.now');
      }
      if (typeof window.relativeTime === 'function') {
        return window.relativeTime(ms);
      }
      const { unit, count } = durationPartsFromMs(ms);
      return window.JournalI18n.render('journal.window.time.duration.' + unit + '.count', [
        ['quantity', window.JournalI18n.cardinalQuantity(count)],
        ['count', String(count)],
      ]);
    }

    function clientElapsedMs(client) {
      if (Number.isFinite(client?.capture_elapsed_ms) && client.capture_elapsed_ms >= 0) {
        return client.capture_elapsed_ms;
      }
      if (typeof client?.last_accepted_ingest_at === 'string' && client.last_accepted_ingest_at) {
        const parsed = Date.parse(client.last_accepted_ingest_at);
        if (Number.isFinite(parsed)) {
          const diff = Date.now() - parsed;
          return diff >= 0 ? diff : 0;
        }
      }
      return null;
    }

    function deviceUtilityLinksHtml(pulse) {
      let links = '<a href="/app/sources/">' + window.JournalI18n.renderHtml('journal.window.view.sources.label') + '</a>';
      if (Number(pulse?.segment_count || 0) > 0) {
        links += '<a href="/app/transcripts/">' + window.JournalI18n.renderHtml('journal.window.home.status.transcripts.today') + '</a>';
      }
      return '<div class="pulse-device-utilities">' + links + '</div>';
    }

    function clientHasSourceAttention(client) {
      if (!isPlainObject(client?.source_delivery)) return false;
      return Object.values(client.source_delivery).some(function (row) {
        return isPlainObject(row) && row.state === 'needs_attention';
      });
    }

    function renderDeviceStripHtml(pulse) {
      const capture = pulse?.capture_health;
      if (!isPlainObject(capture)) return '';
      const clients = Array.isArray(capture.clients) ? capture.clients.filter(isPlainObject) : [];
      const unassessed = Array.isArray(capture.unassessed) ? capture.unassessed.filter(isPlainObject) : [];
      const awaitingUnassessed = unassessed
        .filter(function (row) { return row.reason === 'awaiting_first_delivery'; });
      const declaredQuietUnassessed = unassessed
        .filter(function (row) { return row.reason === 'declared_quiet'; });
      const registry = typeof capture.registry === 'string' ? capture.registry : '';
      const isEmptyRegistry = registry === 'registry_empty'
        || (capture.status === 'no_clients'
          && clients.length === 0
          && awaitingUnassessed.length === 0
          && declaredQuietUnassessed.length === 0
          && registry !== 'registry_unknown'
          && registry !== 'partial_registry');
      if (isEmptyRegistry) {
        return '<div class="pulse-device-strip" data-home-device-strip="empty">'
          + '<div class="pulse-device-empty"><span>'
          + window.JournalI18n.renderHtml('journal.window.home.status.devices.none')
          + '</span> <a href="/app/network/#devices">'
          + window.JournalI18n.renderHtml('journal.window.home.status.devices.pair')
          + '</a></div>'
          + deviceUtilityLinksHtml(pulse)
          + '</div>';
      }
      if (clients.length === 0 && awaitingUnassessed.length === 0 && declaredQuietUnassessed.length === 0) {
        return '<div class="pulse-device-strip" data-home-device-strip="utilities">'
          + deviceUtilityLinksHtml(pulse)
          + '</div>';
      }
      const fallbackDevice = window.JournalI18n.render('journal.window.health.status.an.unnamed.device');
      const chips = [];
      clients.forEach(function (client) {
        const label = String(client.name || '').trim() || fallbackDevice;
        if (client.failing === true || clientHasSourceAttention(client)) {
          chips.push('<a class="pulse-device-chip pulse-device-chip--failing" data-device-chip="failing" href="/app/health/#registeredClientsCard">'
            + window.JournalI18n.renderHtml('journal.window.home.status.device.turned.away', [['device', label]])
            + '</a>');
          return;
        }
        const elapsed = clientElapsedMs(client);
        if (client.status === 'active') {
          chips.push('<span class="pulse-device-chip pulse-device-chip--active" data-device-chip="active">'
            + window.JournalI18n.renderHtml('journal.window.home.status.device.active', [
              ['device', label],
              ['relative', formatAgoMs(elapsed === null ? 0 : elapsed)],
            ])
            + '</span>');
          return;
        }
        if (elapsed === null) {
          chips.push('<span class="pulse-device-chip pulse-device-chip--quiet" data-device-chip="awaiting">'
            + window.JournalI18n.renderHtml('journal.window.home.status.device.awaiting', [['device', label]])
            + '</span>');
          return;
        }
        chips.push('<span class="pulse-device-chip pulse-device-chip--quiet" data-device-chip="quiet">'
          + window.JournalI18n.renderHtml('journal.window.home.status.device.quiet', [
            ['device', label],
            ['age', formatQuietDuration(elapsed)],
          ])
          + '</span>');
      });
      declaredQuietUnassessed.forEach(function (entry) {
        const label = String(entry.name || '').trim() || fallbackDevice;
        chips.push('<span class="pulse-device-chip pulse-device-chip--quiet" data-device-chip="quiet">'
          + window.JournalI18n.renderHtml('journal.window.home.status.device.active', [
            ['device', label],
            ['relative', window.JournalI18n.render('journal.window.health.status.quiet')],
          ])
          + '</span>');
      });
      awaitingUnassessed.forEach(function (entry) {
        const label = String(entry.name || '').trim() || fallbackDevice;
        chips.push('<span class="pulse-device-chip pulse-device-chip--quiet" data-device-chip="awaiting">'
          + window.JournalI18n.renderHtml('journal.window.home.status.device.awaiting', [['device', label]])
          + '</span>');
      });
      return '<div class="pulse-device-strip" data-home-device-strip="paired">'
        + '<div class="pulse-device-list">' + chips.join('') + '</div>'
        + deviceUtilityLinksHtml(pulse)
        + '</div>';
    }

    function renderImportReviewRowHtml() {
      const badges = currentShellBadges();
      const reviews = Array.isArray(badges?.import_reviews)
        ? badges.import_reviews.filter(function (entry) { return isPlainObject(entry) && typeof entry.id === 'string' && entry.id; })
        : [];
      if (!reviews.length) return '';
      const totalWaiting = Number.isFinite(badges?.import)
        ? Math.max(reviews.length, Math.floor(badges.import))
        : reviews.length;
      const first = reviews[0];
      const sourceLabel = String(first.source || '').trim()
        || window.JournalI18n.render('journal.window.view.import.label');
      const subKey = first.swept_after_hour === true
        ? 'journal.window.home.status.import.swept.note'
        : 'journal.window.home.status.import.waiting.note';
      const moreCount = totalWaiting - 1;
      const moreHtml = moreCount > 0
        ? '<a href="/app/import/" data-import-review-more>'
          + window.JournalI18n.renderHtml('journal.window.home.status.import.more.count', [
            ['quantity', window.JournalI18n.cardinalQuantity(moreCount)],
            ['count', String(moreCount)],
          ])
          + '</a>'
        : '';
      return '<div class="pulse-import-reviews" data-home-import-reviews>'
        + '<div class="pulse-import-review-row" data-home-import-review="' + esc(first.id) + '">'
        + '<div class="pulse-import-review-copy">'
        + '<span class="pulse-import-review-title">'
        + window.JournalI18n.renderHtml('journal.window.home.status.import.ready', [['source', sourceLabel]])
        + '</span>'
        + '<span class="pulse-import-review-sub">' + window.JournalI18n.renderHtml(subKey) + '</span>'
        + '</div>'
        + '<div class="pulse-import-review-actions">'
        + '<a href="/app/import/' + encodeURIComponent(first.id) + '">'
        + window.JournalI18n.renderHtml('journal.window.home.status.import.action')
        + '</a>'
        + moreHtml
        + '</div>'
        + '</div>'
        + '</div>';
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

    function renderIntegritySlotHtml(pulse) {
      const integrity = isPlainObject(pulse?.integrity) ? pulse.integrity : null;
      const text = typeof integrity?.text === 'string' ? integrity.text.trim() : '';
      if (!integrity || !text || integrity.tone === 'none') {
        return '<div class="pulse-integrity-slot" data-home-integrity-slot hidden></div>';
      }
      const href = typeof integrity.href === 'string' && integrity.href.startsWith('/') && !integrity.href.startsWith('//')
        ? integrity.href
        : '/app/health/#integrity';
      const stateAttr = typeof integrity.state === 'string' ? integrity.state : '';
      const toneClass = integrity.tone === 'warn'
        ? ' pulse-integrity-slot--warn'
        : ' pulse-integrity-slot--calm';
      return '<div class="pulse-integrity-slot' + toneClass + '" data-home-integrity-slot data-integrity-state="' + esc(stateAttr) + '">'
        + '<a class="pulse-integrity-link" href="' + esc(href) + '">'
        + '<span class="pulse-integrity-ring" aria-hidden="true"></span>'
        + '<span>' + esc(text) + ' →</span>'
        + '</a></div>';
    }

    function renderVitalsHtml(pulse) {
      const { glance: g, notes } = homeHealthGlance(pulse);
      const severity = g.severity || 'amber'; // 'neutral' is truthy; do not fold it into this fallback
      const faultRows = (g.issues || []).map(function (issue) {
        return {
          text: issue.text || '',
          href: issue.href || '/app/health/',
          severity: issue.severity || 'amber',
        };
      });
      const hasCaptureHealth = isPlainObject(pulse?.capture_health);
      const { src: markSrc } = statusMarkSpec(g);
      let html = '<div class="pulse-status" id="pulse-vitals" data-home-surface="vitals" role="status" aria-live="polite">'
        + '<div class="pulse-vitals">'
        + '<div class="pulse-vitals-item"><span class="pulse-vitals-dot ' + esc(severity) + `" aria-hidden="true"></span>`
        + '<span class="pulse-vitals-verdict ' + esc(severity) + '" id="pulse-status-headline">'
        + esc(g.headline || '') + '</span></div>';
      if (!hasCaptureHealth) {
        if (!g.issues?.length && g.last_observation) {
          html += `<div class="pulse-vitals-sep"></div><div class="pulse-vitals-item">${window.JournalI18n.renderHtml('journal.window.home.vitals.last.reached.timestamp', [['last_reached_at', g.last_observation]])}</div>`;
        }
        if (g.cta) {
          html += '<div class="pulse-vitals-sep"></div><a class="pulse-vitals-item pulse-vitals-cta" href="' + esc(g.cta.href || '#') + '">'
            + esc(g.cta.text || '') + '</a>';
        }
      }
      html += '<a class="pulse-vitals-health-link" href="/app/health/">'
        + '<img class="pulse-status-mark" data-home-status-mark src="' + esc(markSrc) + '" alt="" aria-hidden="true">'
        + '<span>' + window.JournalI18n.renderHtml('journal.window.view.status.label') + '</span>'
        + '</a>';
      if (hasCaptureHealth) {
        html += renderDeviceStripHtml(pulse);
      }
      notes.forEach(note => {
        html += '<a class="pulse-vitals-note" href="' + esc(note.href) + '">'
          + '<span class="pulse-vitals-ring" aria-hidden="true"></span>'
          + '<span>' + esc(note.text) + ' →</span></a>';
      });
      // Not yet, and that's fine: a calm row on any verdict, never a chip and
      // never in the count (the day-0 rule).
      const note = isPlainObject(g.note) ? g.note : null;
      if (note && note.text) {
        html += '<a class="pulse-vitals-note" href="' + esc(note.href || '/app/health/') + '">'
          + '<span class="pulse-vitals-ring" aria-hidden="true"></span>'
          + '<span>' + esc(note.text) + ' →</span></a>';
      }
      html += '</div>';
      html += renderIntegritySlotHtml(pulse);
      html += renderImportReviewRowHtml();
      if (faultRows.length) {
        html += '<ul class="pulse-waiting" aria-labelledby="pulse-status-headline">'
          + faultRows.map(waitingRowHtml).join('') + '</ul>';
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
      if (current?.outerHTML !== html) {
        replaceHomeSurface('vitals', html, []);
        if (focusedHref) {
          const next = Array.from(document.querySelectorAll('#pulse-vitals a'))
            .find(link => link.getAttribute('href') === focusedHref)
            || document.querySelector('#pulse-vitals .pulse-vitals-health-link');
          next?.focus({ preventScroll: true });
        }
      }
      syncStatusMarkIcon();
      renderReadyToShape(pulse);
      syncHomePinOffer(pulse);
      syncHomeLiveBadge(pulse);
    }

    function renderWelcomeHtml() {
      return '<p class="pulse-welcome" data-home-surface="welcome">'
        + window.JournalI18n.renderHtml('journal.window.home.window.explanation.this.is.where.your.day.comes')
        + '</p>';
    }

    function downloadProgress(message, fallback, { bytes_done: done, bytes_total: total }) {
      if (!Number.isFinite(done) || !Number.isFinite(total) || total <= 0) {
        return window.JournalI18n.renderHtml(fallback);
      }
      return window.JournalI18n.renderHtml(message, [
        ['downloaded_gb', (done / 1e9).toFixed(1)],
        ['total_gb', (total / 1e9).toFixed(1)]
      ]);
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
        innerHtml += `<h4>${window.JournalI18n.renderHtml('journal.window.home.visible.text.say.a.sentence.out.loud')}</h4>`
          + `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.it.appears.here.as.text')}</p>`;
      }

      if (state === 'waiting') {
        innerHtml += `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.waiting.for.the.next.segment.this')}</p>`;
      } else if (state === 'no_audio') {
        innerHtml += `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.nothing.with.sound.has.reached.your')}</p>`;
      } else if (state === 'transcribing') {
        innerHtml += `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.the.segment.from')} ` + escTime + ` ${window.JournalI18n.renderHtml('journal.window.home.visible.text.hasn.t.been.written.out.yet')}</p>`;
      } else if (state === 'nothing_heard') {
        innerHtml += `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.no.speech.was.found.in.the')} ` + escTime + '. say something, and it shows up in the next one.</p>';
      } else if (state === 'failed') {
        innerHtml += `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.the.segment.from')} ` + escTime + ` ${window.JournalI18n.renderHtml('journal.window.home.visible.text.couldn.t.be.written.out')}</p>`
          + '<a href="' + esc(proof.href || '/app/health/') + `">${window.JournalI18n.renderHtml('journal.window.home.action.see.status')}</a>`;
      } else if (state === 'heard') {
        if (proof.fresh === true) {
          innerHtml += `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.from')} ` + escTime + ': <q>' + esc(proof.text || '') + '</q></p>'
            + '<a href="' + esc(proof.href || '#') + `">${window.JournalI18n.renderHtml('journal.window.home.action.open.it')}</a>`;
        } else {
          innerHtml += `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.speech.from')} ` + escTime + ` ${window.JournalI18n.renderHtml('journal.window.home.visible.text.was.written.out')}</p>`
            + '<a href="' + esc(proof.href || '#') + `">${window.JournalI18n.renderHtml('journal.window.home.action.open.it')}</a>`;
        }
      }

      return '<div class="pulse-getting-started-step" data-proof-state="' + esc(state) + '">'
        + innerHtml
        + '</div>';
    }

    function placesPrecision(read) {
      const cats = Array.isArray(read?.categories) ? read.categories : [];
      if (!cats.includes('places')) return 'full';
      if (cats.includes('places_names')) return 'names';
      if (cats.includes('places_approx')) return 'approx';
      if (cats.includes('places_full')) return 'full';
      return 'names';
    }

    function starterQuestionsForConnection(connection, journalKinds) {
      if (!isPlainObject(connection) || connection.paused) return [];
      const read = connection.permission?.read;
      if (!isPlainObject(read)) return [];
      const cats = new Set(Array.isArray(read.categories) ? read.categories : []);
      const isWhole = read.scope?.kind === 'whole_journal';
      const hasFacetIds = read.scope?.kind === 'facets'
        && Array.isArray(read.scope?.ids)
        && read.scope.ids.length > 0;
      const jk = isPlainObject(journalKinds) ? journalKinds : {};
      const out = [];
      if (jk.transcript_days_7_plus && jk.has_summaries && isWhole && cats.has('transcripts') && cats.has('facets')) {
        out.push({ id: 'chapter', promptKey: 'journal.window.agents.ideas.prompt.chapter', usesKey: 'journal.window.agents.ideas.uses.transcripts.summaries' });
      }
      if (jk.has_entities && jk.has_summaries && isWhole && cats.has('entities') && cats.has('facets') && cats.has('transcripts')) {
        out.push({ id: 'people-circles', promptKey: 'journal.window.agents.ideas.prompt.people.circles', usesKey: 'journal.window.agents.ideas.uses.entities.summaries.transcripts' });
      } else if (jk.has_entities && cats.has('entities') && (isWhole || hasFacetIds)) {
        out.push({ id: 'entities', promptKey: 'journal.window.agents.ideas.prompt.entities', usesKey: 'journal.window.agents.ideas.uses.entities' });
      }
      if (jk.has_places && isWhole && cats.has('places')) {
        const precision = placesPrecision(read);
        out.push({
          id: precision === 'names' ? 'places-list' : 'places-map',
          promptKey: precision === 'names' ? 'journal.window.agents.ideas.prompt.places.list' : 'journal.window.agents.ideas.prompt.places.map',
          usesKey: 'journal.window.agents.ideas.uses.places',
        });
      }
      if (jk.has_photos && isWhole && cats.has('photos')) {
        out.push({ id: 'photos', promptKey: 'journal.window.agents.ideas.prompt.photos', usesKey: 'journal.window.agents.ideas.uses.photos' });
      }
      if (out.length === 0 && jk.has_transcripts && isWhole && cats.has('transcripts')) {
        out.push({ id: 'today', promptKey: 'journal.window.agents.ideas.prompt.today', usesKey: 'journal.window.agents.ideas.uses.transcripts' });
      }
      return out.slice(0, 2);
    }

    function renderAgentStarterHtml(connection, items) {
      if (!connection || !Array.isArray(items) || !items.length) return '';
      const agentName = String(connection.name || '').trim()
        || window.JournalI18n.render('journal.window.view.agents.label');
      const rowsHtml = items.map(function (item) {
        const promptText = window.JournalI18n.render(item.promptKey);
        const qId = 'home-agent-idea-' + item.id;
        return '<li class="pulse-agent-idea-row" data-agent-idea="' + esc(item.id) + '">'
          + '<div class="pulse-agent-idea-main">'
          + '<q class="pulse-agent-idea-q" id="' + esc(qId) + '">' + esc(promptText) + '</q>'
          + '<small class="pulse-agent-idea-uses">' + window.JournalI18n.renderHtml(item.usesKey) + '</small>'
          + '</div>'
          + '<button type="button" class="pulse-copy-button" data-copy-agent-idea data-prompt="' + esc(promptText) + '" aria-describedby="' + esc(qId) + '">'
          + window.JournalI18n.renderHtml('journal.window.agents.button.label.copy')
          + '</button>'
          + '</li>';
      }).join('');
      return '<div class="pulse-agent-ideas" data-home-agent-ideas>'
        + '<h4 class="pulse-agent-ideas-head">'
        + window.JournalI18n.renderHtml('journal.window.agents.ideas.heading', [['agent_name', agentName]])
        + '</h4>'
        + '<p class="pulse-agent-ideas-lead">'
        + window.JournalI18n.renderHtml('journal.window.agents.ideas.lead', [['agent_name', agentName]])
        + '</p>'
        + '<ul class="pulse-agent-ideas-list">' + rowsHtml + '</ul>'
        + '</div>';
    }

    function agentCheckState() {
      if (!agentsSettled) {
        return { inFlight: true, done: false, showStep: false, showStarter: false };
      }
      if (agentsPayload && agentsPayload.failed) {
        return { inFlight: false, done: true, showStep: false, showStarter: false };
      }
      if (!agentsPayload || !Array.isArray(agentsPayload.connections)) {
        return { inFlight: false, done: false, showStep: false, showStarter: false };
      }
      if (agentsPayload.connections.length > 0) {
        const anyUsedThisWeek = agentsPayload.connections.some(function (conn) {
          return isPlainObject(conn) && Number(conn.requests_this_week) > 0;
        });
        if (!anyUsedThisWeek && isPlainObject(agentsPayload.journal_kinds)) {
          for (let i = 0; i < agentsPayload.connections.length; i += 1) {
            const conn = agentsPayload.connections[i];
            if (!isPlainObject(conn) || conn.requests_this_week !== 0) continue;
            const starterItems = starterQuestionsForConnection(conn, agentsPayload.journal_kinds);
            if (starterItems.length > 0) {
              return {
                inFlight: false,
                done: false,
                showStep: false,
                showStarter: true,
                starterConnection: conn,
                starterItems,
              };
            }
          }
        }
        return { inFlight: false, done: true, showStep: false, showStarter: false };
      }
      return { inFlight: false, done: false, showStep: true, showStarter: false };
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

    const FIRST_DEVICE_AGE = Object.assign(Object.create(null), {
      second: 'journal.window.home.device.last.added.second',
      minute: 'journal.window.home.device.last.added.minute',
      hour: 'journal.window.home.device.last.added.hour',
      day: 'journal.window.home.device.last.added.day',
      week: 'journal.window.home.device.last.added.week',
      month: 'journal.window.home.device.last.added.month'
    });

    function renderDeviceLine(device) {
      const state = device.state || 'other';
      let copy = '';
      if (state === 'awaiting_first') {
        copy = window.JournalI18n.renderHtml('journal.window.home.device.paired.awaiting', [['name', String(device.name || '')]]);
      } else if (state === 'receiving' || state === 'quiet') {
        const age = device.last_added_age;
        const message = age && FIRST_DEVICE_AGE[age.unit];
        if (message) {
          copy = window.JournalI18n.renderHtml(message, [
            ['name', String(device.name || '')],
            ['quantity', window.JournalI18n.cardinalQuantity(age.count)],
            ['count', String(age.count)]
          ]);
        } else {
          copy = window.JournalI18n.renderHtml('journal.window.home.device.paired', [['name', String(device.name || '')]]);
        }
      } else {
        copy = window.JournalI18n.renderHtml('journal.window.home.device.paired', [['name', String(device.name || '')]]);
      }
      return '<div class="pulse-device-line" data-device-state="' + esc(state) + '">'
        + '<p>' + copy + '</p>'
        + '</div>';
    }

    function renderAgentStepHtml() {
      let commandBlock = '';
      const door = agentsPayload?.local_door;
      if (agentsPayload?.local_agent_helpers === true && door && door.enabled === true && door.listening === true && typeof door.address === 'string') {
        const address = door.address;
        const cmd = 'claude mcp add --transport http journal "' + address + '"';
        commandBlock = '<div class="pulse-agent-command" data-agent-command>'
          + `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.on.your.journal.s.computer.run')}</p>`
          + '<pre><code>' + esc(cmd) + '</code></pre>'
          + '<button type="button" class="pulse-copy-button" data-copy-agent-command data-command="' + esc(cmd) + `">${window.JournalI18n.renderHtml('journal.window.home.action.copy')}</button>`
          + `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.then.type')} <code>/mcp</code> ${window.JournalI18n.renderHtml('journal.window.home.setup.agent.choose.connector', [['agent', 'Claude Code']])} <code>journal</code> ${window.JournalI18n.renderHtml('journal.window.home.visible.text.and.authenticate')}</p>`
          + '</div>';
      }
      return '<div class="pulse-getting-started-step" data-agent-step>'
        + `<h4>${window.JournalI18n.renderHtml('journal.window.home.visible.text.connect.an.agent')}</h4>`
        + `<p>${window.JournalI18n.renderHtml('journal.window.home.setup.agent.description', [['agent', 'Claude Code']])}</p>`
        + commandBlock
        + `<a href="/app/agents/">${window.JournalI18n.renderHtml('journal.window.home.link.label.other.agents.and.pairing.codes')}</a>`
        + '</div>';
    }

    function renderPhoneStepHtml() {
      return '<div class="pulse-getting-started-step" data-phone-step>'
        + `<h4>${window.JournalI18n.renderHtml('journal.window.home.visible.text.add.your.phone')}</h4>`
        + `<p>${window.JournalI18n.renderHtml('journal.window.home.setup.phone.download', [['site', 'solstone.app/mobile']])}</p>`
        + `<a href="/app/network/#devices">${window.JournalI18n.renderHtml('journal.window.home.link.label.pair.a.device')}</a>`
        + '</div>';
    }

    function renderThinkingStepHtml(step) {
      let title = '';
      let body = '';
      let href = '';
      let linkText = '';
      if (step === 'choose') {
        title = window.JournalI18n.render('journal.window.home.window.explanation.choose.where.the.thinking.happens');
        body = `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.your.journal.keeps.everything.until.you')}</p>`;
        href = '/app/thinking/';
        linkText = window.JournalI18n.render('journal.window.home.setup.action.choose');
      } else if (step === 'cant_run') {
        title = window.JournalI18n.render('journal.window.home.window.explanation.your.journal.s.computer.can.t');
        body = `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.choose.another.way.when.you.re')}</p>`;
        href = '/app/thinking/';
        linkText = window.JournalI18n.render('journal.window.home.setup.action.choose');
      } else if (step === 'byo') {
        title = window.JournalI18n.render('journal.window.home.window.explanation.finish.setting.up.your.own.model');
        href = '/app/thinking/#byo-setup';
        linkText = window.JournalI18n.render('journal.window.home.setup.action.finish');
      } else if (step === 'confidential') {
        title = window.JournalI18n.render('journal.window.home.window.explanation.finish.setting.up.confidential.processing');
        href = '/app/thinking/#confidential-setup';
        linkText = window.JournalI18n.render('journal.window.home.setup.action.finish');
      } else if (step === 'local') {
        title = window.JournalI18n.render('journal.window.home.window.explanation.finish.setting.up.local.thinking');
        href = '/app/thinking/#local-setup';
        linkText = window.JournalI18n.render('journal.window.home.setup.action.finish');
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
        + `<h2>${window.JournalI18n.renderHtml('journal.window.home.section.heading.getting.started')}</h2>`;

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
            const message = downloadProgress('journal.window.home.download.thinking.progress', 'journal.window.home.visible.text.downloading.the.local.model', dl.thinking);
            const bar = renderDownloadBar('thinking', dl.thinking.bytes_done, dl.thinking.bytes_total);
            html += '<div class="pulse-download" data-download="thinking" data-download-state="downloading">'
              + `<p>${message}</p>`
              + bar
              + `<a href="/app/thinking/#local-setup">${window.JournalI18n.renderHtml('journal.window.home.link.label.cancel.on.the.thinking.page')}</a>`
              + '</div>';
          } else if ((state === 'error' || state === 'cancelled') && setupPayload?.thinking?.capability !== 'blocked') {
            html += '<div class="pulse-download" data-download="thinking" data-download-state="' + esc(state) + '">'
              + `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.the.local.model.didn.t.finish')}</p>`
              + `<a href="/app/thinking/#local-setup">${window.JournalI18n.renderHtml('journal.window.home.link.label.try.again.on.the.thinking.page')}</a>`
              + '</div>';
          }
        }
        if (dl.speech) {
          const state = dl.speech.state;
          if (state === 'downloading') {
            const message = downloadProgress('journal.window.home.download.speech.progress', 'journal.window.home.visible.text.downloading.the.speech.model', dl.speech);
            const bar = renderDownloadBar('speech', dl.speech.bytes_done, dl.speech.bytes_total);
            html += '<div class="pulse-download" data-download="speech" data-download-state="downloading">'
              + `<p>${message}</p>`
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
                ? '<button type="button" data-speech-retry' + (speechRetryInFlight ? ' disabled' : '') + `>${window.JournalI18n.renderHtml('journal.window.home.visible.text.try.again')}</button>`
                : '';
              html += '<div class="pulse-download" data-download="speech" data-download-state="' + esc(state) + '">'
                + `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.the.speech.model.didn.t.finish')}</p>`
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
          text = window.JournalI18n.renderHtml('journal.window.home.first.segment.read', [['time', String(time)]]);
        } else if (seg.thinking === 'pending') {
          text = window.JournalI18n.renderHtml('journal.window.home.first.segment.pending', [['time', String(time)]]);
        } else {
          text = window.JournalI18n.renderHtml('journal.window.home.first.segment.arrived', [['time', String(time)]]);
        }
        html += '<div class="pulse-first-segment" data-segment-thinking="' + esc(seg.thinking || 'none') + '">'
          + '<p>' + text + '</p>'
          + '<a href="' + esc(seg.href || '#') + `">${window.JournalI18n.renderHtml('journal.window.home.action.open.it')}</a>`
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
        html += `<h3 data-getting-started-next>${window.JournalI18n.renderHtml('journal.window.home.section.heading.next')}</h3>` + stepsHtml.join('');
      }

      if (agentState.showStarter) {
        html += renderAgentStarterHtml(agentState.starterConnection, agentState.starterItems);
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
      if (typeof ctx.read !== 'function') {
        agentsSettled = true;
        agentsPayload = { failed: true };
        updateGettingStartedSurface();
        return Promise.resolve(null);
      }
      if (!agentsRequest) {
        agentsRequest = ctx.read(AGENTS_STATE_URL, { noAuthRedirect: true })
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
      if (typeof ctx.read !== 'function') {
        speechRetryInFlight = false;
        if (button) button.disabled = false;
        return;
      }
      ctx.read(SETUP_ACTION_URL, {
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

    function handleCopyAgentIdea(button) {
      const prompt = button.getAttribute('data-prompt') || '';
      if (!prompt) return;
      if (window.convey && typeof window.convey.copyToClipboard === 'function') {
        window.convey.copyToClipboard(prompt);
      } else if (navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
        navigator.clipboard.writeText(prompt).catch(function () {});
      }
      button.innerHTML = window.JournalI18n.renderHtml('journal.window.agents.visible.text.copied');
    }

    function handleSecurityRestart(button) {
      if (securityRestartInFlight) return;
      securityRestartInFlight = true;
      securityRestartError = '';
      if (button) button.disabled = true;
      if (lastSecurityPayload) renderSecurity(lastSecurityPayload);
      if (typeof ctx.read !== 'function') {
        securityRestartInFlight = false;
        if (button) button.disabled = false;
        return;
      }
      ctx.read(SYSTEM_RESTART_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
        noAuthRedirect: true,
      })
        .then(function () {
          securityRestartInFlight = false;
          readSecurity();
        })
        .catch(function (error) {
          securityRestartInFlight = false;
          const msg = (typeof error?.serverMessage === 'string' && error.serverMessage.trim())
            || (typeof error?.message === 'string' && error.message.trim())
            || '';
          securityRestartError = msg;
          if (lastSecurityPayload) renderSecurity(lastSecurityPayload);
          else if (button) button.disabled = false;
        });
    }

    function narrativeScope(pulse) {
      if (pulse.narrative_source !== 'pulse') return '';
      const scope = pulse.narrative_window;
      const keys = ['segments', 'activities', 'input_segments', 'input_activities'];
      if (!scope || !keys.every(key => Number.isSafeInteger(scope[key]) && scope[key] >= 0)) {
        return `<p class="pulse-narrative-meta" data-narrative-scope="unknown">${window.JournalI18n.renderHtml('journal.window.home.visible.text.what.this.summary.covers.wasn.t')}</p>`;
      }
      const sentences = [
        window.JournalI18n.render('journal.window.home.narrative.coverage.batch.counts', [
          ['segment_quantity', window.JournalI18n.cardinalQuantity(scope.input_segments)],
          ['activity_quantity', window.JournalI18n.cardinalQuantity(scope.input_activities)],
          ['processed_segments', String(scope.segments)], ['batch_segments', String(scope.input_segments)],
          ['processed_activities', String(scope.activities)], ['batch_activities', String(scope.input_activities)]
        ]),
        window.JournalI18n.render('journal.window.home.narrative.coverage.overlap')
      ];
      if (scope.since_ms > 0 && Number.isFinite(scope.since_ms) && Math.abs(scope.since_ms) <= 8640000000000000) {
        const start = window.JournalFormat.timestamp(new Date(scope.since_ms).toISOString());
        sentences.push(window.JournalI18n.render('journal.window.home.narrative.coverage.started', [['start_time', start]]),
          window.JournalI18n.render('journal.window.home.narrative.coverage.clock'));
      }
      const gaps = Array.isArray(scope.gaps) ? scope.gaps : [];
      return `<details class="pulse-narrative-meta"><summary>${window.JournalI18n.renderHtml('journal.window.home.disclosure.label.what.this.summary.covers')}</summary><p>` + esc(sentences.join(' ')) + '</p>'
        + (gaps.length ? `<p>${window.JournalI18n.renderHtml('journal.window.home.visible.text.some.source.material.was.unavailable')}</p><ul>` + gaps.map(gap => '<li>' + esc(gap) + '</li>').join('') + '</ul>' : '') + '</details>';
    }

    // The today side's written account. The tab is its heading, so it carries
    // none of its own.
    function renderNarrativeHtml(pulse) {
      if (pulse.narrative_content !== null && pulse.narrative_content !== undefined) {
        const updated = pulse.narrative_updated_at
          ? `<span>${window.JournalI18n.renderHtml('journal.window.home.narrative.updated.timestamp', [['updated_at', window.JournalFormat.timestamp(pulse.narrative_updated_at)]])}</span>`
          : '';
        return '<div class="pulse-narrative" id="pulse-narrative">'
          + '<div class="pulse-narrative-content" id="pulse-narrative-content">' + markdown(pulse.narrative_content || '') + '</div>'
          + '<div class="pulse-account-meta">' + updated
          + '<a class="pulse-tell-more" href="/app/thinking/#runs/' + encodeURIComponent(pulse.today) + '/' + encodeURIComponent(pulse.narrative_source || 'pulse') + `">${window.JournalI18n.renderHtml('journal.window.home.action.how.this.was.written')}</a>`
          + '</div>'
          + narrativeScope(pulse)
          + '</div>';
      }
      const empty = Number(pulse.segment_count || 0) > 0
        ? window.JournalI18n.render('journal.window.home.window.explanation.analysis.will.be.available.after.the')
        : window.JournalI18n.render('journal.window.home.window.explanation.no.narrative.yet.one.will.appear');
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
          + `<h2 class="pulse-section-header">${window.JournalI18n.renderHtml('journal.window.home.section.heading.your.week')}</h2>`
          + '<div class="pulse-reflection-title">' + esc(reflection.title || '') + '</div>';
        if (typeof reflection.memory === 'string' && reflection.memory) {
          html += '<div class="pulse-reflection-memory">' + markdown(reflection.memory) + '</div>';
        }
        if (typeof reflection.empty === 'string' && reflection.empty) {
          html += '<div class="pulse-reflection-empty">' + esc(reflection.empty) + '</div>';
        }
        html += '<a class="pulse-reflection-link" href="' + esc(reflection.url) + `">${window.JournalI18n.renderHtml('journal.window.home.action.open.your.week')}</a>`
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

    // 226 reads as "3h 46m"; under an hour stays "45m".
    function hoursAndMinutes(minutes) {
      const hours = Math.floor(minutes / 60);
      const rest = minutes % 60;
      if (!hours) return window.JournalI18n.render('journal.window.home.today.duration.minute.count', [['quantity', window.JournalI18n.cardinalQuantity(rest)], ['minutes', String(rest)]]);
      return rest
        ? window.JournalI18n.render('journal.window.home.today.duration.hour.and.minute.count', [
          ['quantity', window.JournalI18n.cardinalQuantity(hours)],
          ['minute_quantity', window.JournalI18n.cardinalQuantity(rest)], ['hours', String(hours)], ['minutes', String(rest)]
        ])
        : window.JournalI18n.render('journal.window.home.today.duration.hour.count', [['quantity', window.JournalI18n.cardinalQuantity(hours)], ['hours', String(hours)]]);
    }

    function facetChipRows(pulse, activities) {
      const measuredFacets = isPlainObject(pulse.facet_data) ? pulse.facet_data : {};
      const facetMarks = isPlainObject(pulse.facets) ? pulse.facets : {};
      const names = Object.keys(measuredFacets);
      activities.forEach(function (activity) {
        activityFacets(activity).forEach(function (facet) {
          if (names.indexOf(facet) === -1) names.push(facet);
        });
      });
      return names.map(function (name) {
        const data = measuredFacets[name];
        const meta = isPlainObject(facetMarks[name]) ? facetMarks[name] : null;
        const minutes = isPlainObject(data) ? (parseInt(data.minutes || 0, 10) || 0) : 0;
        // No `count` here on purpose: facet_data counts the whole day, and the
        // label beside these chips counts the window the card shows.
        return {
          name: name,
          title: meta && meta.title ? meta.title : name,
          meta: meta,
          text: isPlainObject(data) ? hoursAndMinutes(minutes) : window.JournalI18n.render('journal.window.home.text.no.time.measured'),
        };
      });
    }

    function renderTodayHtml(pulse) {
      const activities = Array.isArray(pulse.activities) ? pulse.activities : [];
      const facetMarks = isPlainObject(pulse.facets) ? pulse.facets : {};
      const enabledFacets = Object.keys(facetMarks).filter(function (name) {
        const f = facetMarks[name];
        return isPlainObject(f) && !f.muted;
      }).length;
      const showRowMarks = enabledFacets > 1;
      const day = accountTime && pulse.now?.slice(0, 16) === accountTime ? pulse.today : '';
      const allActivities = '<a class="pulse-today-link" href="/app/activities/' + esc(day)
        + `" data-copy-key="home-today-link">${window.JournalI18n.renderHtml('journal.window.home.action.all.of.todays.activities')}</a>`;
      const todayExpNote = '<div class="pulse-today-exp-note">'
        + '<span>' + window.JournalI18n.renderHtml('journal.window.home.today.experiences.note') + '</span>'
        + '<a class="pulse-today-exp-link" href="/app/experiences/">'
        + window.JournalI18n.renderHtml('journal.window.home.action.open.experiences')
        + '</a></div>';
      if (!activities.length) {
        return '<div class="pulse-today" id="pulse-today">'
          + `<div class="pulse-narrative-empty">${window.JournalI18n.renderHtml('journal.window.home.visible.text.no.recent.activity.yet.today')}</div>`
          + allActivities
          + todayExpNote
          + '</div>';
      }
      let html = '<div class="pulse-today" id="pulse-today">';
      if (activities.length) {
        // The header counted all of them, the list showed six, and the chips
        // measured a third population. Each number now says what it counts, and
        // the rest of the list is one click away instead of gone. G1-101.
        const shown = todayActivitiesExpanded ? activities : activities.slice(0, ACTIVITY_PREVIEW);
        const label = shown.length === activities.length
          ? window.JournalI18n.render('journal.window.home.today.activity.count', [['quantity', window.JournalI18n.cardinalQuantity(activities.length)], ['activity_count', String(activities.length)]])
          : window.JournalI18n.render('journal.window.home.today.activity.preview.count', [['quantity', window.JournalI18n.cardinalQuantity(activities.length)], ['preview_count', String(shown.length)], ['activity_count', String(activities.length)]]);
        const showAll = shown.length === activities.length
          ? ''
          : `<button type="button" class="pulse-activities-more" data-activities-show-all>${window.JournalI18n.renderHtml('journal.window.home.button.label.show.all')}</button>`;
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
          const mark = showRowMarks && window.ConveyFacetMark && isPlainObject(facetMarks[facet])
            ? window.ConveyFacetMark.html(facetMarks[facet], 'sm')
            : '';
          html += '<a class="pulse-activity" href="' + href + '">'
            + '<span class="pulse-activity-time">' + esc(activity.display_time ? window.JournalFormat.time(activity.display_time) : '') + '</span>'
            + mark
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
        // Only a partial measure needs its count: "5 of 5" says nothing.
        const chipLabel = activities.length && measured < activities.length
          ? window.JournalI18n.render('journal.window.home.today.facets.measured.count', [['quantity', window.JournalI18n.cardinalQuantity(activities.length)], ['measured_count', String(measured)], ['activity_count', String(activities.length)]])
          : window.JournalI18n.render('journal.window.home.window.explanation.time.in.each.facet');
        html += '<div class="pulse-activities-label">' + esc(chipLabel) + '</div><div class="pulse-facet-dist">';
        facetChips.forEach(function (chip) {
          const mark = window.ConveyFacetMark && chip.meta ? window.ConveyFacetMark.html(chip.meta, 'sm') : '';
          html += '<span class="pulse-facet-chip">' + mark + esc(chip.title) + ' · ' + esc(chip.text) + '</span>';
        });
        html += '</div>';
      }
      html += allActivities;
      html += todayExpNote;
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
        + ' data-account-tab="' + side + '">' + window.JournalI18n.renderHtml(side === 'today'
          ? 'journal.window.home.account.tab.today' : 'journal.window.home.account.tab.yesterday') + '</button>';
      const panel = (side, inner) => '<div class="pulse-account-panel" role="tabpanel"'
        + ' id="pulse-account-panel-' + side + '" aria-labelledby="pulse-account-tab-' + side + '"'
        + ' data-account-panel="' + side + '"' + (selectedSide === side ? '' : ' hidden') + '>' + inner + '</div>';
      return '<section class="pulse-account" id="pulse-account" data-home-surface="account">'
        + '<div class="pulse-account-head">'
        + `<div class="pulse-account-tabs" role="tablist" aria-label="${window.JournalI18n.renderHtml('journal.window.home.accessible.name.which.day')}">` + ACCOUNT_SIDES.map(tab).join('') + '</div>'
        + '<span class="pulse-account-day" data-account-day><span data-account-day-text>' + esc(accountDayText(selectedSide, pulse)) + '</span></span>'
        + '</div>'
        + panel('today', renderTodaySideHtml(pulse))
        + panel('yesterday', renderYesterdaySideHtml(briefingData, pulse))
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
      if (side === 'yesterday') syncYesterdayActivities();
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
      window.JournalI18n.render('journal.window.date.today'),
      window.JournalI18n.render('journal.window.date.yesterday'),
      window.JournalI18n.render('journal.window.date.tomorrow')
    ].map(word => word.toLocaleLowerCase(window.JournalI18n.formatLocale)));

    function formatConnectionDay(day) {
      const asOf = accountTime && lastPulse?.now?.slice(0, 16) === accountTime
        ? new Date(accountTime)
        : undefined;
      return window.JournalFormat.day(day, asOf);
    }

    function midSentenceDay(day) {
      const text = String(day === null || day === undefined ? '' : day);
      const lower = text.toLocaleLowerCase(window.JournalI18n.formatLocale);
      return RELATIVE_DAY_WORDS.has(lower) ? lower : text;
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
      const day = formatConnectionDay(neighbor.last_seen);
      return day
        ? window.JournalI18n.render('journal.window.home.connections.moments.recent.count', [['quantity', window.JournalI18n.cardinalQuantity(count)], ['count', count.toLocaleString('en-US')], ['day', midSentenceDay(day)]])
        : window.JournalI18n.render('journal.window.home.connections.moments.count', [['quantity', window.JournalI18n.cardinalQuantity(count)], ['count', count.toLocaleString('en-US')]]);
    }

    // The evidence line repeated the name back under itself on every row but one,
    // and the one row where it differed read like a truncation bug. G1-27.
    function connectionEvidenceHtml(neighbor, name) {
      const label = typeof neighbor.latest_label === 'string' ? neighbor.latest_label.trim() : '';
      if (!label || label.toLowerCase() === String(name || '').trim().toLowerCase()) return '';
      if (neighbor.latest_kind !== 'mentioned') return '';
      return `<span class="pulse-connections-evidence">${window.JournalI18n.renderHtml('journal.window.home.connections.evidence.transcription', [['label', label]])}</span>`;
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
      const eventCount = window.JournalI18n.render('journal.window.home.connections.events.count', [['quantity', window.JournalI18n.cardinalQuantity(count)], ['count', String(count)]]);
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
          + `<h2 class="pulse-section-header">${window.JournalI18n.renderHtml('journal.window.home.section.heading.connections')}</h2>`
          + `<div class="pulse-empty-message">${window.JournalI18n.renderHtml('journal.window.home.visible.text.no.named.connections.yet')} `
          + `<a href="/app/speakers/">${window.JournalI18n.renderHtml('journal.window.home.link.label.name.voices.in.speakers')}</a></div>`
          + '</div>';
      }
      if (connections.state === 'unavailable') {
        return '<div class="pulse-empty-state" data-home-surface="connections">'
          + `<h2 class="pulse-section-header">${window.JournalI18n.renderHtml('journal.window.home.section.heading.connections')}</h2>`
          + `<div class="pulse-empty-message">${window.JournalI18n.renderHtml('journal.window.home.visible.text.connections.are.unavailable.right.now')} `
          + `<a href="/app/health/">${window.JournalI18n.renderHtml('journal.window.home.link.label.check.journal.status')}</a></div>`
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
        + `<h2 class="pulse-section-header">${window.JournalI18n.renderHtml('journal.window.home.section.heading.connections')}</h2>`;

      if (relationshipItems.length) {
        // The shelf label only tells the owner anything when a second shelf is
        // there to tell it apart from. Alone it is the card heading again. G1-28.
        const shelfLabel = attendanceItems.length || calendarItems.length
          ? `<div class="pulse-connections-shelf-label">${window.JournalI18n.renderHtml('journal.window.home.visible.text.connections.found.in.your.journal')}</div>`
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
        html += `<div class="pulse-connections-note">${window.JournalI18n.renderHtml('journal.window.home.visible.text.no.direct.connections.yet')}</div>`;
      }

      if (attendanceItems.length) {
        html += '<div class="pulse-connections-shelf" data-shelf="attendance">'
          + `<div class="pulse-connections-shelf-label">${window.JournalI18n.renderHtml('journal.window.home.visible.text.often.around.events.only')}</div>`
          + '<div class="pulse-connections-cluster">' + attendanceItems.join(' · ') + '</div>'
          + '</div>';
      }

      if (calendarItems.length) {
        html += '<div class="pulse-connections-shelf" data-shelf="calendar">'
          + `<div class="pulse-connections-shelf-label">${window.JournalI18n.renderHtml('journal.window.home.visible.text.on.your.calendar.only')}</div>`
          + '<div class="pulse-connections-cluster">' + calendarItems.join(' · ') + '</div>'
          + '</div>';
      }

      if (mentionRows.length) {
        // The summary says how many rows the disclosure holds, so it stays useful
        // as the only shelf on the card rather than restating the section. G1-28.
        const mentionSummary = window.JournalI18n.render('journal.window.home.connections.mentions.count', [['quantity', window.JournalI18n.cardinalQuantity(mentionRows.length)], ['count', String(mentionRows.length)]]);
        html += '<details class="pulse-connections-shelf"><summary>' + esc(mentionSummary) + '</summary><div class="pulse-connections-list">'
          + mentionRows.map(neighbor => connectionRowHtml(neighbor, connections)).join('') + '</div></details>';
      }
      const rendered = relationshipItems.length + attendanceItems.length + calendarItems.length + mentionRows.length;
      // The coverage note and the way out of the section are one block, not two.
      // G1-107.
      const moreLink = Number(connections.total || 0) > rendered
        ? `<a class="pulse-connections-more" href="/app/entities">${window.JournalI18n.renderHtml('journal.window.home.link.label.all.connections')}</a>`
        : '';
      if (Number.isSafeInteger(connections.horizon_earlier_days)
          && typeof connections.horizon_day === 'string' && connections.horizon_day) {
        const days = connections.horizon_earlier_days;
        const day = midSentenceDay(formatConnectionDay(connections.horizon_day));
        html += '<div class="pulse-connections-horizon">'
          + window.JournalI18n.renderHtml('journal.window.home.connections.horizon.day', [['day', day]]) + ' '
          + window.JournalI18n.renderHtml('journal.window.home.connections.horizon.earlier.days', [
            ['quantity', window.JournalI18n.cardinalQuantity(days)], ['earlier_days', String(days)]
          ]) + ' '
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

    // The welcome page has no pair: a live refresh must not add one to it.
    function renderPair(pulse) {
      if (lastPulse?.home_state === 'welcome') return;
      replaceHomeSurface('pair', renderPairHtml(pulse), predecessors('pair'));
    }

    function normalizeBriefingFromPulse(pulse) {
      return normalizeBriefing({
        exists: pulse.briefing_exists,
        phase: pulse.briefing_phase,
        saved_lane: pulse.briefing_saved_lane,
        analysis_day: pulse.briefing_analysis_day,
        meta: pulse.briefing_meta,
        sections: pulse.briefing_sections,
        body: pulse.briefing_body,
        experiences: pulse.briefing_experiences
      });
    }

    function normalizeBriefing(data) {
      return {
        exists: Boolean(data.exists),
        phase: data.phase || 'eod',
        saved_lane: data.saved_lane ?? null,
        analysis_day: typeof data.analysis_day === 'string' ? data.analysis_day : null,
        meta: isPlainObject(data.meta) ? data.meta : null,
        sections: isPlainObject(data.sections) ? data.sections : {},
        body: Array.isArray(data.body) ? data.body.filter(isPlainObject) : [],
        experiences: Array.isArray(data.experiences) ? data.experiences.filter(isPlainObject) : []
      };
    }

    // The briefing's stamp is the journal's own wall time with no offset. Show
    // its hour and minute as written: parsing it as an instant would shift it
    // by the browser's zone.
    function clockLabel(hhmm) {
      const match = /^(\d{2}):(\d{2})$/.exec(String(hhmm || ''));
      if (!match || Number(match[1]) > 23 || Number(match[2]) > 59) return '';
      return new Date(2000, 0, 1, Number(match[1]), Number(match[2]))
        .toLocaleString(window.JournalI18n.formatLocale, { hour: 'numeric', minute: '2-digit' })
        .toLowerCase();
    }

    function writtenAtTime(generated) {
      const match = /T(\d{2}:\d{2})/.exec(String(generated || ''));
      return match ? clockLabel(match[1]) : '';
    }

    // Before the nightly run has had its first chance, say when the first
    // account is due rather than calling it late or missing.
    const BRIEFING_NOT_YET = Object.assign(Object.create(null), {
      awaiting_engine: window.JournalI18n.render('journal.window.home.window.explanation.yesterday.shows.here.once.you.choose'),
      first_night: window.JournalI18n.render('journal.window.home.window.explanation.your.first.day.shows.here.by'),
      first_night_empty: window.JournalI18n.render('journal.window.home.window.explanation.yesterday.shows.here.the.morning.after')
    });

    const BRIEFING_AWAITING_ENGINE_LANES = Object.assign(Object.create(null), {
      byo: window.JournalI18n.render('journal.window.home.window.explanation.yesterday.shows.here.once.your.own'),
      spp: window.JournalI18n.render('journal.window.home.window.explanation.yesterday.shows.here.once.confidential.processing'),
      local: window.JournalI18n.render('journal.window.home.window.explanation.yesterday.shows.here.once.local.thinking')
    });

    function briefingRunHref(data, pulseContext) {
      return '/app/thinking/#runs/' + encodeURIComponent(data?.analysis_day || pulseContext?.briefing_analysis_day || '') + '/morning_briefing';
    }

    // The yesterday side never renders empty. Silence is the worst failure mode.
    function briefingPlaceholderHtml(data, pulseContext) {
      if (data.exists) return '';
      // Nothing the owner did yesterday, so no briefing is coming. The body
      // lines stand alone; with none, say only that.
      if (data.phase === 'none_owed') {
        if ((Array.isArray(data.body) && data.body.length) || (Array.isArray(data.experiences) && data.experiences.length)) return '';
        return `<div class="pulse-briefing-placeholder">${window.JournalI18n.renderHtml('journal.window.home.visible.text.nothing.from.yesterday.to.write.up')}</div>`;
      }
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
            + window.JournalI18n.renderHtml('journal.window.home.briefing.expected.time') + ' ' + window.JournalI18n.renderHtml('journal.window.home.briefing.late.hour.count', [
              ['quantity', window.JournalI18n.cardinalQuantity(Number(lateness.late_hours || 0))],
              ['hours', String(lateness.late_hours || 0)]
            ])
            + '<a class="pulse-briefing-status-link" href="' + run + `">${window.JournalI18n.renderHtml('journal.window.home.action.check.status')}</a>`
            + '</div>';
        }
        return `<div class="pulse-briefing-placeholder">${window.JournalI18n.renderHtml('journal.window.home.visible.text.yesterday.is.being.written.up')}</div>`;
      }
      // Missing, and any hour with no account that no not-yet state explains.
      return '<div class="pulse-briefing-placeholder">'
        + window.JournalI18n.renderHtml('journal.window.home.night.briefing.not.written') + ' '
        + '<a class="pulse-briefing-status-link" href="' + run + `">${window.JournalI18n.renderHtml('journal.window.home.action.see.the.run')}</a>`
        + '</div>';
    }

    // A time on the journal's clock, "HH:MM", said the way this page says every
    // time, and on which date when it isn't the day's.
    function bodyMoment(at) {
      if (!isPlainObject(at)) return '';
      const time = clockLabel(at.time);
      if (!time) return '';
      const date = /^(\d{4})(\d{2})(\d{2})$/.exec(String(at.date || ''));
      if (!date) return time;
      const day = new Date(Number(date[1]), Number(date[2]) - 1, Number(date[3]))
        .toLocaleString(window.JournalI18n.formatLocale, { month: 'short', day: 'numeric' })
        .toLowerCase();
      return window.JournalI18n.render('journal.window.home.body.time.with.day', [['time', String(time)], ['day', String(day)]]);
    }

    function listed(names) {
      if (names.length < 2) return names.join('');
      return window.JournalI18n.render('journal.window.home.body.sources.and', [['start', String(names.slice(0,-1).join(', '))], ['last', String(names[names.length - 1])]]);
    }

    // One workout, night or session, in plain words: what, when, how far, and
    // what recorded it. Never an assessment.
    function bodyLineText(line) {
      const start = bodyMoment(line.start);
      const end = bodyMoment(line.end);
      const clock = start && end ? 'range' : start ? 'start' : 'untimed';
      const distance = typeof line.distance === 'string' ? line.distance : '';
      const from = Array.isArray(line.from)
        ? line.from.filter((name) => typeof name === 'string' && name) : [];
      const source = line.logged ? 'logged' : from.length ? 'recorded' : 'no-source';
      const arrangement = [clock, distance ? 'distance' : 'no-distance', source,
        line.part_removed ? 'part-removed' : 'intact'].join('-');
      return window.JournalI18n.render('journal.window.home.body.activity.line', [
        ['arrangement', arrangement], ['activity', String(line.name || '')],
        ['started_at', start], ['ended_at', end],
        ['distance', distance], ['source_names', listed(from)]
      ]);
    }

    // The day's workouts, sleep and mindfulness, one plain line each, composed
    // when read. They sit apart from the briefing, which never wrote them.
    function bodyLinesHtml(lines) {
      if (!Array.isArray(lines) || !lines.length) return '';
      return `<div class="pulse-activities-label">${window.JournalI18n.renderHtml('journal.window.home.visible.text.from.your.devices.and.apps')}</div>`
        + '<div class="pulse-briefing-body"><ul>'
        + lines.map((line) => '<li>' + esc(bodyLineText(line)) + '</li>').join('')
        + '</ul></div>';
    }

    function formatExperienceTimeRange(startedAt, endedAt) {
      const start = writtenAtTime(startedAt);
      const end = writtenAtTime(endedAt);
      if (start && end && start !== end) return start + ' - ' + end;
      return start || end || '';
    }

    function renderYesterdayExperiencesHtml(data, pulseContext) {
      const experiences = Array.isArray(data?.experiences) ? data.experiences.filter(isPlainObject) : [];
      if (!experiences.length) return '';
      const facetMarks = isPlainObject(pulseContext?.facets) ? pulseContext.facets : {};
      const day = data?.analysis_day || pulseContext?.briefing_analysis_day || '';
      const openHref = day ? '/app/experiences/' + encodeURIComponent(day) : '/app/experiences/';
      const cards = experiences.map(function (exp) {
        const facet = typeof exp.primary_facet === 'string' ? exp.primary_facet : '';
        const meta = isPlainObject(facetMarks[facet]) ? facetMarks[facet] : null;
        const facetTitle = meta && meta.title ? meta.title : facet;
        const mark = window.ConveyFacetMark && meta ? window.ConveyFacetMark.html(meta, 'sm') : '';
        const timeRange = formatExperienceTimeRange(exp.started_at, exp.ended_at);
        const metaParts = [facetTitle, timeRange].filter(Boolean);
        const href = typeof exp.href === 'string' && exp.href.startsWith('/') ? exp.href : openHref;
        const title = typeof exp.title === 'string' && exp.title.trim() ? exp.title.trim() : facetTitle;
        const summary = typeof exp.summary === 'string' ? exp.summary.trim() : '';
        return '<a class="pulse-exp-card" href="' + esc(href) + '">'
          + '<div class="pulse-exp-card-meta">' + mark + '<span>' + esc(metaParts.join(' · ')) + '</span></div>'
          + '<div class="pulse-exp-card-title">' + esc(title) + '</div>'
          + (summary ? '<div class="pulse-exp-card-summary">' + esc(summary) + '</div>' : '')
          + '</a>';
      }).join('');
      return '<div class="pulse-yesterday-experiences" data-yesterday-experiences>'
        + '<div class="pulse-yesterday-section-head">'
        + '<h3 class="pulse-yesterday-subhead">' + window.JournalI18n.renderHtml('journal.window.home.yesterday.experiences.heading') + '</h3>'
        + '<a class="pulse-yesterday-more" href="' + esc(openHref) + '">' + window.JournalI18n.renderHtml('journal.window.home.action.open.experiences') + '</a>'
        + '</div>'
        + '<div class="pulse-exp-list">' + cards + '</div>'
        + '</div>';
    }

    function renderYesterdayActivitiesHtml(day, view, pulseContext) {
      const items = Array.isArray(view?.items)
        ? view.items.filter(function (it) { return isPlainObject(it) && !it.hidden; })
        : [];
      if (!items.length) return '';
      const facetMarks = isPlainObject(pulseContext?.facets) ? pulseContext.facets : {};
      const enabledFacets = Object.keys(facetMarks).filter(function (name) {
        const f = facetMarks[name];
        return isPlainObject(f) && !f.muted;
      }).length;
      const showRowMarks = enabledFacets > 1;
      const shown = yesterdayActivitiesExpanded ? items : items.slice(0, ACTIVITY_PREVIEW);
      const showAll = shown.length === items.length
        ? ''
        : `<button type="button" class="pulse-activities-more" data-yesterday-activities-show-all>${window.JournalI18n.renderHtml('journal.window.home.button.label.show.all')}</button>`;
      const rows = shown.map(function (it) {
        const facet = typeof it.facet === 'string' ? it.facet : '';
        const id = typeof it.id === 'string' ? it.id : '';
        const href = '/app/activities/' + esc(day)
          + '?facet=' + esc(encodeURIComponent(facet))
          + '#' + esc(encodeURIComponent(facet)) + '/' + esc(encodeURIComponent(id));
        const mark = showRowMarks && window.ConveyFacetMark && isPlainObject(facetMarks[facet])
          ? window.ConveyFacetMark.html(facetMarks[facet], 'sm')
          : '';
        const time = it.start ? clockLabel(it.start) || it.start : '';
        const label = it.title || it.description || '';
        return '<a class="pulse-activity" href="' + href + '">'
          + '<span class="pulse-activity-time">' + esc(time) + '</span>'
          + mark
          + '<span class="pulse-activity-text">' + esc(label) + '</span>'
          + '</a>';
      }).join('');
      return '<div class="pulse-yesterday-section-head">'
        + '<h3 class="pulse-yesterday-subhead">' + window.JournalI18n.renderHtml('journal.window.home.yesterday.activities.heading') + '</h3>'
        + showAll
        + '</div>'
        + '<div class="pulse-activities">' + rows + '</div>'
        + '<a class="pulse-today-link" href="/app/activities/' + esc(day) + '">'
        + window.JournalI18n.renderHtml('journal.window.home.action.all.of.yesterdays.activities')
        + '</a>';
    }

    function paintYesterdayActivitiesHost() {
      const host = surface?.querySelector?.('[data-yesterday-activities-host]');
      if (!host) return;
      const day = briefingData?.analysis_day || lastPulse?.briefing_analysis_day || '';
      if (!day || yesterdayActivitiesDay !== day || !yesterdayActivitiesData) {
        host.innerHTML = '';
        host.hidden = true;
        return;
      }
      const html = renderYesterdayActivitiesHtml(day, yesterdayActivitiesData, lastPulse);
      host.innerHTML = html;
      host.hidden = !html;
    }

    function syncYesterdayActivities() {
      const day = briefingData?.analysis_day || lastPulse?.briefing_analysis_day || '';
      if (!/^\d{8}$/.test(day) || typeof ctx?.read !== 'function') return;
      if (yesterdayActivitiesDay === day && yesterdayActivitiesData) {
        paintYesterdayActivitiesHost();
        return;
      }
      if (yesterdayActivitiesRequest && yesterdayActivitiesRequest.day === day) return;
      const req = { day };
      yesterdayActivitiesRequest = req;
      ctx.read('/app/activities/api/day/' + day + '/view', { noAuthRedirect: true })
        .then(function (data) {
          if (yesterdayActivitiesRequest !== req) return;
          yesterdayActivitiesDay = day;
          yesterdayActivitiesData = isPlainObject(data) ? data : null;
          paintYesterdayActivitiesHost();
        })
        .catch(function () {
          if (yesterdayActivitiesRequest === req) {
            yesterdayActivitiesRequest = null;
          }
        });
    }

    function renderYesterdaySideHtml(data, pulseContext) {
      if (!data) return '';
      let body = renderYesterdayExperiencesHtml(data, pulseContext);
      if (data.exists) {
        const raw = typeof data.sections.yesterday === 'string' ? data.sections.yesterday : '';
        // An empty section carries no time: "written at" beside "nothing was
        // written up" would contradict it.
        const time = raw ? writtenAtTime(data.meta?.generated) : '';
        body += (raw
          ? '<div class="pulse-briefing-body" id="pulse-briefing-body">' + markdown(raw) + '</div>'
          : `<div class="pulse-briefing-placeholder">${window.JournalI18n.renderHtml('journal.window.home.visible.text.nothing.from.yesterday.was.written.up')}</div>`)
          + '<div class="pulse-account-meta">'
          + (time ? `<span>${window.JournalI18n.renderHtml('journal.window.home.briefing.written.timestamp', [['written_at', time]])}</span>` : '')
          + '<a class="pulse-tell-more" href="' + esc(briefingRunHref(data, pulseContext)) + `">${window.JournalI18n.renderHtml('journal.window.home.action.how.this.was.written')}</a>`
          + '</div>';
      } else {
        body += briefingPlaceholderHtml(data, pulseContext);
      }
      body += bodyLinesHtml(data.body);
      body += '<div class="pulse-yesterday-activities" data-yesterday-activities-host hidden></div>';
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
      if (selectedSide === 'yesterday') {
        updateAccountDay();
        syncYesterdayActivities();
      } else {
        paintYesterdayActivitiesHost();
      }
      if (lastPulse) renderVitals(lastPulse);
    }

    function renderBriefingError(error) {
      const panel = yesterdayPanel();
      if (!panel) return;
      replaceAccountContent(panel, '<div class="pulse-briefing" id="pulse-briefing" data-phase="error">'
        + surfaceError(error, window.JournalI18n.render('journal.window.home.response.yesterday.failed'))
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
    // Pulse carries it, so the first page needn't read the list as well.
    function observeOriginalsTotal(data) {
      const total = data?.originals_total;
      if (total === undefined) return;
      originalsTotal = Number.isSafeInteger(total) && total >= 0 ? total : null;
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
          observeOriginalsTotal(data);
          renderPair(data);
          renderVitals(data);
          clearPulseRefreshError('pulse-vitals');
        })
        .catch(function (error) {
          if (read < pulseAppliedVersion) return;
          pulseAppliedVersion = read;
          pulseReadFailed = true;
          if (!lastPulse && surface) {
            surface.innerHTML = surfaceError(error, window.JournalI18n.render('journal.window.home.response.pulse.failed'));
            bindSurfaceRetry();
            reportHomeReady('error');
          }
          if (lastPulse) renderVitals(lastPulse);
          logError(error, 'home: refreshVitals failed');
          renderRefreshError('pulse-vitals', error, window.JournalI18n.render('journal.window.home.window.explanation.couldn.t.refresh.service.status.showing'));
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
          observeOriginalsTotal(data);
          renderVitals(data);
          renderPair(data);
          clearPulseRefreshError('pulse-vitals');
        })
        .catch(function (error) {
          if (read < pulseAppliedVersion) return;
          pulseAppliedVersion = read;
          pulseReadFailed = true;
          if (!lastPulse && surface) {
            surface.innerHTML = surfaceError(error, window.JournalI18n.render('journal.window.home.response.pulse.failed'));
            bindSurfaceRetry();
            reportHomeReady('error');
          }
          if (lastPulse) renderVitals(lastPulse);
          logError(error, 'home: refreshNarrative failed');
          renderRefreshError('pulse-narrative', error, window.JournalI18n.render('journal.window.home.window.explanation.couldn.t.refresh.narrative.showing.last'));
        });
    }

    // The first briefing read starts beside pulse, so the first page isn't
    // waiting on one before asking for the other.
    let earlyBriefing = null;

    function startBriefingEarly() {
      earlyBriefing = apiJson(BRIEFING_URL);
      earlyBriefing.catch(function () {});
    }

    async function refreshBriefing() {
      try {
        // A refresh follows a change, so it asks for the briefing as it is now.
        const request = earlyBriefing || ctx.read(BRIEFING_URL, { fresh: true });
        earlyBriefing = null;
        const data = await request;
        renderBriefing(data, lastPulse);
        clearPulseRefreshError('pulse-briefing');
      } catch (error) {
        lastReadFailed = true;
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
      const showAllYesterday = closest(target, '[data-yesterday-activities-show-all]');
      if (showAllYesterday) {
        event.preventDefault();
        yesterdayActivitiesExpanded = true;
        paintYesterdayActivitiesHost();
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
      const copyIdea = closest(target, '[data-copy-agent-idea]');
      if (copyIdea) {
        event.preventDefault();
        handleCopyAgentIdea(copyIdea);
        return;
      }
      const restartBtn = closest(target, '[data-security-restart-btn]');
      if (restartBtn) {
        event.preventDefault();
        handleSecurityRestart(restartBtn);
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
    // refreshed. The stream's first connect re-reads only when `firstReadFailed`
    // says the page's own read didn't succeed (it opened into an outage);
    // otherwise that read is what the connect would refresh.
    function connectionRecovery(refresh, firstReadFailed) {
      let everConnected = false;
      let lost = false;
      return function (state) {
        const connected = !!(state && state.connected);
        if (!connected) {
          if (everConnected) lost = true;
          return;
        }
        const first = !everConnected;
        everConnected = true;
        if (lost || (first && firstReadFailed && firstReadFailed())) {
          lost = false;
          refresh();
        }
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
      heldWorkSnapshot = {
        queues: msg.queues || {},
        held: msg.held || [],
        crashed: rows(msg.crashed),
        queue_hold: msg.queue_hold || null,
        observedAt: Date.now(),
      };
      heldWorkCurrent = true;
      ctx.clearTimeout(heldWorkExpiry);
      // Match Health's freshness boundary; unrelated bus activity cannot renew it.
      heldWorkExpiry = ctx.setTimeout(invalidateHeldWork, 30000);
      if (lastPulse) renderVitals(lastPulse);
      // The supervisor reports every few seconds. Pulse is read again only
      // when something it reports has changed, not on every report.
      const key = supervisorStatusKey(msg);
      if (key === lastSupervisorKey) return;
      lastSupervisorKey = key;
      refreshVitals();
    }

    // What in a supervisor report can change what pulse says: which services
    // run and how, what crashed, which tasks run, finished or are held, and
    // which heartbeats are stale. Not pids, uptimes or durations, which move on
    // every report.
    let lastSupervisorKey = null;
    const observeStatusKeys = new Map();

    function rows(value) {
      return Array.isArray(value) ? value : [];
    }

    function supervisorStatusKey(msg) {
      return JSON.stringify([
        rows(msg.services).map(function (s) { return [s?.name, s?.phase, s?.reason_code]; }),
        rows(msg.crashed).map(function (c) { return [c?.name, c?.phase, c?.reason_code]; }),
        rows(msg.tasks).map(function (t) { return [t?.reference, t?.name, !!t?.slow, !!t?.stuck]; }),
        rows(msg.recent_tasks).map(function (t) { return [t?.reference, t?.exit_status]; }),
        rows(msg.held).map(function (h) { return [h?.reference, h?.reasons]; }),
        msg.queue_hold ? [msg.queue_hold.reason] : null,
        rows(msg.stale_heartbeats).map(function (h) { return h?.source_filename ?? h?.name ?? null; })
      ]);
    }

    // An observer reports its counters every few seconds; only its mode moving
    // (idle, screencast, terminal) is news for pulse.
    function observeStatusChanged(msg) {
      const stream = String(msg.stream ?? '');
      const key = String(msg.mode ?? '');
      if (observeStatusKeys.get(stream) === key) return false;
      observeStatusKeys.set(stream, key);
      return true;
    }

    // A day's material changed. Pulse speaks for today and yesterday.
    function changeTouchesPulse(msg) {
      if (msg.event !== 'changed' || msg.kind !== 'day') return false;
      const today = lastPulse?.today;
      if (!today) return true;
      const key = String(msg.key ?? '');
      return key >= previousDayKey(today);
    }

    function previousDayKey(day) {
      const value = String(day).replace(/-/g, '');
      if (!/^\d{8}$/.test(value)) return '';
      const date = new Date(Date.UTC(+value.slice(0, 4), +value.slice(4, 6) - 1, +value.slice(6, 8)));
      date.setUTCDate(date.getUTCDate() - 1);
      return date.toISOString().slice(0, 10).replace(/-/g, '');
    }

    // Some of what pulse says moves with time alone (a device going quiet, a
    // briefing coming due), so it is read once a minute while the page is in
    // view, as well as on each change.
    const PULSE_FLOOR_MS = 60000;

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

    // New material arrived. A pulse read already in flight may have been
    // answered before it, so read once more after that read lands. Status
    // reports repeat every few seconds and are simply folded into the read in
    // flight.
    let trailingVitalsRefresh = null;
    function refreshVitalsAfterInFlight() {
      const pending = [pulseRequest, vitalsRequest].filter(Boolean);
      if (!pending.length) return refreshVitals();
      if (trailingVitalsRefresh) return trailingVitalsRefresh;
      trailingVitalsRefresh = Promise.allSettled(pending).then(function () {
        trailingVitalsRefresh = null;
        return refreshVitals();
      });
      return trailingVitalsRefresh;
    }

    // Home subscribes to the live stream before it reads anything, so on a page
    // whose stream is already connected (a move from another view) no event
    // can fall between its first read and its subscription.
    function wireRealtime() {
      if (realtimeWired) return;
      const recover = connectionRecovery(function () {
        refreshChangedPulse();
        refreshBriefing();
      }, function () { return lastReadFailed; });
      ctx.onConnectionState(function (state) {
        if (!state?.connected) {
          // The first HTTP read still needs to settle the page while the
          // event stream is unavailable. Once painted, keep the last known
          // service status until a fresh read takes over.
          if (lastPulse) pulseAppliedVersion = ++pulseReadVersion;
          pulseReadFailed = true;
          invalidateHeldWork();
        }
        recover(state);
      });
      ctx.listen('supervisor', receiveSupervisorStatus);
      ctx.listen('observe', function (msg) {
        if (msg.event === 'observed') refreshVitalsAfterInFlight();
        else if (msg.event === 'status' && observeStatusChanged(msg)) refreshVitals();
      });
      ctx.listen('change', function (msg) {
        if (changeTouchesPulse(msg)) refreshChangedPulse();
      });
      ctx.visibleInterval(refreshVitals, PULSE_FLOOR_MS);
      ctx.listen('cortex', function (msg) {
        if (msg.event === 'finish' && (msg.name === 'flow' || msg.name === 'pulse')) refreshChangedPulse();
        if (msg.event === 'finish' && msg.name === 'morning_briefing') refreshBriefing();
        if (msg.event === 'error') refreshVitals();
      });
      realtimeWired = true;
    }

    // A security update the journal's advisory check found, for this journal
    // or a paired app. The banner stays until the status says the install is
    // no longer below its floor; a failed read leaves it as it was.
    function securityLineHtml(flag) {
      const notes = typeof flag.notes === 'string' && flag.notes.startsWith('https://') ? flag.notes : null;
      const link = notes ? ' <a href="' + esc(notes) + `" target="_blank" rel="noreferrer" referrerpolicy="no-referrer">${window.JournalI18n.renderHtml('journal.window.home.action.what.it.fixes')}</a>` : '';
      if (flag.restart === true) {
        const errHtml = securityRestartError
          ? '<span class="pulse-security-error" data-press-failure>' + esc(securityRestartError) + '</span>'
          : '';
        return '<div class="pulse-security-restart" data-security-restart>'
          + '<p>' + window.JournalI18n.renderHtml('journal.window.shared.status.pane.visible.text.a.security.update.is.installed.restart') + link + '</p>'
          + '<div class="pulse-security-actions">'
          + '<button type="button" class="pulse-security-restart-btn" data-security-restart-btn' + (securityRestartInFlight ? ' disabled' : '') + '>'
          + window.JournalI18n.renderHtml('journal.window.shared.status.pane.security.restart')
          + '</button>'
          + errHtml
          + '</div>'
          + '</div>';
      }
      const fixedIn = typeof flag.fixed_in === 'string' ? flag.fixed_in : '';
      const text = flag.subject === 'journal'
        ? window.JournalI18n.render('journal.window.home.security.journal', [['version', String(fixedIn)]])
        : typeof flag.device === 'string' && flag.device
          ? window.JournalI18n.render('journal.window.home.security.device', [['device', flag.device], ['version', fixedIn]])
          : window.JournalI18n.render('journal.window.home.security.unnamed.device', [['version', fixedIn]]);
      return '<p>' + esc(text) + link + '</p>';
    }

    function renderSecurity(status) {
      if (!isPlainObject(status)) return;
      lastSecurityPayload = status;
      const nextState = typeof status.security?.state === 'string' ? status.security.state : null;
      const stateChanged = nextState !== securityState;
      securityState = nextState;
      const banner = document.getElementById?.('security-banner');
      if (banner) {
        const flags = Array.isArray(status.security?.flags) ? status.security.flags.filter(isPlainObject) : [];
        banner.innerHTML = flags.map(securityLineHtml).join('');
        banner.hidden = flags.length === 0;
      }
      if (stateChanged && lastPulse) {
        renderVitals(lastPulse);
      }
    }

    function readSecurity() {
      if (typeof ctx.read !== 'function') return;
      ctx.read('/api/system/status', { noAuthRedirect: true }).then(renderSecurity, function () {});
    }

    function applyShellBadges(data) {
      if (!isPlainObject(data)) return;
      shellBadges = data;
      if (lastPulse) renderVitals(lastPulse);
    }

    function readShellBadgesFallback() {
      if (shellBadgesFallbackRequested || typeof ctx.read !== 'function') return;
      if (isPlainObject(shellBadges) || isPlainObject(window.solShellBadges)) return;
      shellBadgesFallbackRequested = true;
      ctx.read(SHELL_BADGES_URL, { noAuthRedirect: true }).then(applyShellBadges, function () {});
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
        if (stem && window.HomeWeek) {
          window.HomeWeek.mount(ctx, stem);
        }
        return;
      }
      // The originals page is the originals list alone; it reads nothing else
      // home reads.
      if (pathname.startsWith(ORIGINALS_PAGE)) {
        surface.innerHTML = '';
        window.HomeOriginals?.mount(ctx);
        return;
      }
      isFirstPage = true;
      ctx.on(window, 'hashchange', function () {
        // The browser owns same-path fragment changes. Reload only if the
        // account clock changes, so reads from the old mount cannot win.
        if (window.location.pathname === pathname && accountTimeFromFragment() !== accountTime) {
          window.location.reload();
        }
      });
      ctx.on(window, 'sol:shell-status', syncStatusMarkIcon);
      ctx.on(window, 'sol:shell-badges', function (event) {
        applyShellBadges(event?.detail || window.solShellBadges);
      });
      wireInteractions();
      wireRealtime();
      startBriefingEarly();
      loadPulse();
      readFirstMinutes();
      readAgents();
      readSecurity();
      if (!isPlainObject(window.solShellBadges)) {
        if (typeof window.solShellChrome?.setBadge === 'function') {
          ctx.setTimeout(readShellBadgesFallback, 2500);
        } else {
          readShellBadgesFallback();
        }
      }
      ctx.visibleInterval(readSecurity, 60000);
    }

    ctx.onDispose?.(function () {
      window.solShellChrome?.setHomeBadgeDelta?.(null);
    });

    ctx.on(document, 'visibilitychange', function () {
      if (isFirstPage && !firstMinutesClosed) {
        readAgents();
      }
    });

    // Back from the originals page can restore this page from the browser's
    // cache without mounting it again, so read the total once more.
    ctx.on(window, 'pageshow', function (event) {
      if (isFirstPage && event && event.persisted) refreshVitals();
    });

    ctx.global('selectAccountSide', selectAccountSide);
    initHome();
  }
})();
