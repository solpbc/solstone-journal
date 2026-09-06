// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function(){
  const HEALTH_LOGS_COPY = {
    "LOGS_LEVEL_FILTER_LABEL": "level",
    "LOGS_LEVEL_OPTION_ALL": "all levels",
    "LOGS_LEVEL_OPTION_ERROR": "errors only",
    "LOGS_LEVEL_OPTION_INFO": "info & above",
    "LOGS_LEVEL_OPTION_WARNING": "warnings & errors",
    "LOGS_SERVICE_COLLAPSED": "── {service} ── ({n} lines, ★ {errors} errors)",
    "LOGS_SERVICE_FILTER_LABEL": "service",
    "LOGS_STREAM_FILTER_LABEL": "stream"
  };
  const HEALTH_GLANCE_COPY = {
    "HEALTH_GLANCE_CATCHING_UP": "catching up on {n} {tasks} in the background. last update {age}.",
    "HEALTH_GLANCE_CHECKING": "checking where your journal stands…",
    "HEALTH_GLANCE_CLIENT_SILENT": "one of your devices hasn't reached your journal recently.",
    "HEALTH_GLANCE_DEVICE_FAILING": "{device} isn't reaching your journal.",
    "HEALTH_GLANCE_DEVICES_FAILING": "{n} devices aren't reaching your journal: {devices}.",
    "HEALTH_GLANCE_DEVICE_SILENT": "{device} hasn't added to your journal in {age}.",
    "HEALTH_GLANCE_DEVICE_SILENT_NO_AGE": "{device} hasn't added to your journal recently.",
    "HEALTH_GLANCE_DEVICES_SILENT": "{n} devices haven't added to your journal recently: {devices}.",
    "HEALTH_GLANCE_DEVICES_UNAVAILABLE": "your devices' delivery status is unavailable right now.",
    "HEALTH_GLANCE_OK": "everything's working. the solstone app last added to your journal {age}.",
    "HEALTH_GLANCE_BRAIN_ATTENTION": "{headline}",
    "HEALTH_GLANCE_SERVICE_ATTENTION": "1 service needs attention: {service_names}.",
    "HEALTH_GLANCE_SERVICES_ATTENTION": "{n} services need attention: {service_names}.",
    "HEALTH_GLANCE_SERVICES_UNREACHABLE": "the journal's services couldn't be reached. check that your journal is running."
  };
  // Worst signal wins, and green is earned: a verdict may not read "everything's
  // working" while the device rows below it say a device has gone quiet, and a
  // failure to derive device delivery renders unavailable, never green.
  // vpx/design-system/health-verdict-glance.md
  const GLANCE_DEVICES_ACTION = { href: '#registeredClientsCard', label: 'view devices' };

  let brainSnapshot = null;
  let backlogCopy = {};

  const healthInfoReady = fetch('/app/health/api/info')
    .then(r => r.json())
    .then(info => {
      state.localHost = info.hostname;
      brainSnapshot = info.brain || brainSnapshot;
      updateObserve();
      renderBrainHealth();
      updateStatusSummary();
    })
    .catch(() => {
      if (!state.connected) {
        connectError = true;
        updateStatusSummary();
      }
    });

  fetch('/app/stats/api/usage?day=' + todayKey())
    .then(r => {
      if (!r.ok) throw new Error('token usage unavailable');
      return r.json();
    })
    .then(data => {
      const tokens = Number(data?.total?.tokens);
      if (Number.isFinite(tokens)) {
        state.todayTokens = tokens;
      }
      renderBrainHealth();
    })
    .catch(() => {
      state.todayTokens = null;
      renderBrainHealth();
    });

  // State management
  let connectError = false;

  const state = {
    supervisorSeen: false,
    supervisorStatusAt: null,
    cortexStatusAt: null,
    cortexSeen: false,
    registeredClients: null,
    registeredClientsFailed: false,
    services: new Map(),        // Running services
    connected: false,
    crashed: new Map(),         // Crashed services (separate from running)
    tasks: [],
    health: null,
    queues: {},                 // From supervisor.status
    schedules: [],              // From supervisor.status
    agents: new Map(),
    agentCount: 0,              // Quick count from cortex.status
    imports: new Map(),
    think: null,                // Think status snapshot (null when idle)
    thinkActive: false,         // Whether think is currently running
    serviceLogs: new Map(),     // service name -> array of {ts, stream, line}
    logFollow: true,            // Auto-scroll log viewport
    logsCollapsed: true,
    logLevelFilter: 'all',
    logCollapsedServices: new Map(),
    logErrorCount: 0,
    logTotalCount: 0,
    lastLogTs: null,
    lastAgentFinishTs: null,
    todayTokens: null,
    clients: new Map(),       // keyed by stream name
    recentErrors: [],
    agentErrorsOk: true,
    recentErrorsFilter: null,
    pendingRecentErrorsFocus: false,
    pendingLogAnchor: null,
    localHost: null,
    deepLinkMode: false,
    lastLogFilter: null,        // Last rendered filter state for incremental append
    lastEventTs: null,           // Timestamp of last event from WebSocket
  };

  const escapeHtml = (value) => window.AppServices.escapeHtml(value);

  // DOM elements
  const elements = {
    healthGlance: document.getElementById('healthGlance'),
    healthGlanceSentence: document.getElementById('healthGlanceSentence'),
    glanceErrorsValue: document.getElementById('glanceErrorsValue'),
    glanceErrorsLabel: document.getElementById('glanceErrorsLabel'),
    serviceDots: document.getElementById('serviceDots'),
    agentsValue: document.getElementById('agentsValue'),
    tasksValue: document.getElementById('tasksValue'),
    healthValue: document.getElementById('healthValue'),
    vitalsStatus: document.getElementById('vitalsStatus'),
    observeModeBadge: document.getElementById('observeModeBadge'),
    observeModeLabel: document.getElementById('observeModeLabel'),
    screencastStatus: document.getElementById('screencastStatus'),
    screencastDetail: document.getElementById('screencastDetail'),
    tmuxStatus: document.getElementById('tmuxStatus'),
    tmuxDetail: document.getElementById('tmuxDetail'),
    audioStatus: document.getElementById('audioStatus'),
    audioDetail: document.getElementById('audioDetail'),
    activityStatus: document.getElementById('activityStatus'),
    activityDetail: document.getElementById('activityDetail'),
    describeStatus: document.getElementById('describeStatus'),
    describeDetail: document.getElementById('describeDetail'),
    transcribeStatus: document.getElementById('transcribeStatus'),
    transcribeDetail: document.getElementById('transcribeDetail'),
    observeSourceNote: document.getElementById('observeSourceNote'),
    clientsCard: document.getElementById('clientsCard'),
    clientsGrid: document.getElementById('clientsGrid'),
    registeredClientsCard: document.getElementById('registeredClientsCard'),
    registeredClientsStrip: document.getElementById('registeredClientsStrip'),
    observeContent: document.getElementById('observeContent'),
    observeEmpty: document.getElementById('observeEmpty'),
    cortexSection: document.getElementById('cortexSection'),
    cortexGrid: document.getElementById('cortexGrid'),
    importerSection: document.getElementById('importerSection'),
    importerGrid: document.getElementById('importerGrid'),
    errorSummary: document.getElementById('errorSummary'),
    errorSummaryContent: document.getElementById('errorSummaryContent'),
    allQuietCard: document.getElementById('allQuietCard'),
    idleCardStats: document.getElementById('idleCardStats'),
    thinkCard: document.getElementById('thinkCard'),
    thinkInfo: document.getElementById('thinkInfo'),
    thinkProgress: document.getElementById('thinkProgress'),
    thinkAgents: document.getElementById('thinkAgents'),
    queuesSection: document.getElementById('queuesSection'),
    queuesValue: document.getElementById('queuesValue'),
    schedulesSection: document.getElementById('schedulesSection'),
    schedulesValue: document.getElementById('schedulesValue'),
    logsViewport: document.getElementById('logsViewport'),
    logServiceFilter: document.getElementById('logServiceFilter'),
    logLevelFilter: document.getElementById('logLevelFilter'),
    logStreamFilter: document.getElementById('logStreamFilter'),
    logFollowBtn: document.getElementById('logFollowBtn'),
    logClearBtn: document.getElementById('logClearBtn'),
    logErrorBadge: document.getElementById('logErrorBadge'),
    logsConnectionNote: document.getElementById('logsConnectionNote'),
    logsSummaryBadge: document.getElementById('logsSummaryBadge'),
    brainHealthStatus: document.getElementById('brainHealthStatus'),
    brainCheckBtn: document.getElementById('brainCheckBtn'),
    logsAnnouncer: document.getElementById('logsAnnouncer'),
    logsCollapseIndicator: document.getElementById('logsCollapseIndicator'),
    connectionIndicator: document.getElementById('connectionIndicator'),
    vitalsBar: document.querySelector('.vitals-bar'),
    vitalsCheckBtn: document.getElementById('vitalsCheckBtn'),
    logExportBtn: document.getElementById('logExportBtn'),
  };

  function logHealthError(error, context) {
    if (window.logError) {
      window.logError(error, { context });
    } else if (window.console && window.console.error) {
      window.console.error(error);
    }
  }

  function applyHealthCopy() {
    document.querySelectorAll('[data-health-copy]').forEach((element) => {
      const key = element.getAttribute('data-health-copy');
      if (!key || !Object.hasOwn(HEALTH_LOGS_COPY, key)) {
        logHealthError(new Error(`missing health copy key: ${key || ''}`), 'health copy render');
        return;
      }
      element.textContent = HEALTH_LOGS_COPY[key];
    });
  }

  async function getJson(path) {
    if (window.apiJson) {
      return window.apiJson(path);
    }
    const response = await fetch(path, { headers: { Accept: 'application/json' } });
    const payload = await response.json();
    if (!response.ok) throw payload;
    return payload;
  }

  function clearHealthStateError() {
    const errorHost = document.querySelector('[data-health-state-error]');
    if (!errorHost) return;
    errorHost.hidden = true;
    errorHost.textContent = '';
  }

  function renderHealthStateError(error) {
    const errorHost = document.querySelector('[data-health-state-error]');
    if (!errorHost) {
      logHealthError(error, 'health state fetch failed');
      return;
    }
    errorHost.hidden = false;
    if (window.SurfaceState) {
      errorHost.innerHTML = window.SurfaceState.error({
        heading: "health state couldn't be loaded",
        desc: window.CONVEY_COPY?.RELOAD_HINT || 'reload to try again.',
        retry: true,
        detail: error,
        headingLevel: 'h3'
      });
    } else {
      errorHost.innerHTML = '<button type="button" class="surface-state-retry">try again</button>';
    }
    errorHost.querySelector('.surface-state-retry')?.addEventListener('click', () => {
      loadHealthState();
    });
    logHealthError(error, 'health state fetch failed');
  }

  // "1 day is still catching up" named no day and went nowhere. When the day is
  // known and it is the only thing outstanding, name it through the shared date
  // helper and link it; otherwise keep the sentence and point at the oldest.
  function renderBacklogVerdict(line, data, stuckCount) {
    const day = typeof data.oldest_pending_day === 'string' ? data.oldest_pending_day : '';
    const pending = Number(data.pending_days) || 0;
    const label = day ? window.JournalFormat.day(day) : '';
    if (!day || !label) {
      line.textContent = data.verdict || '';
      return;
    }
    const link = document.createElement('a');
    link.className = 'backlog-verdict-day';
    link.href = `/app/transcripts/${encodeURIComponent(day)}`;
    if (pending === 1 && stuckCount === 0) {
      line.textContent = '';
      link.textContent = `${label.replace(/^(Today|Yesterday|Tomorrow)$/, (word) => word.toLowerCase())} is still catching up →`;
      line.appendChild(link);
      return;
    }
    line.textContent = `${data.verdict || ''} `;
    link.textContent = `oldest: ${label} →`;
    line.appendChild(link);
  }

  function renderBacklogState(backlogState) {
    const data = backlogState || {};
    backlogCopy = data.copy || {};
    const host = document.querySelector('[data-backlog-stuck-rows]');
    const rowsHost = host && host.querySelector('[data-backlog-rows]');
    const rows = Array.isArray(data.stuck_rows) ? data.stuck_rows : [];
    const verdictLine = document.querySelector('#backlogVerdict .backlog-verdict-line');
    if (verdictLine) renderBacklogVerdict(verdictLine, data, rows.length);
    clearHealthStateError();

    if (!host) return;
    if (!rowsHost || rows.length === 0) {
      host.hidden = true;
      if (rowsHost) rowsHost.replaceChildren();
      return;
    }

    host.hidden = false;
    const heading = host.querySelector('[data-backlog-heading]');
    const description = host.querySelector('[data-backlog-description]');
    if (heading) heading.textContent = backlogCopy.bucket_heading || '';
    if (description) description.textContent = backlogCopy.bucket_description || '';
    rowsHost.replaceChildren();
    rows.forEach((row) => {
      const rowEl = document.createElement('div');
      rowEl.className = 'backlog-row';

      const main = document.createElement('div');
      main.className = 'backlog-row-main';

      const day = document.createElement('span');
      day.className = 'backlog-row-day';
      day.textContent = window.JournalFormat.day(row.day || '');
      main.appendChild(day);

      const badge = document.createElement('span');
      badge.className = 'backlog-badge';
      badge.textContent = backlogCopy.day_badge || '';
      main.appendChild(badge);

      const reason = document.createElement('span');
      reason.className = 'backlog-row-reason';
      reason.textContent = row.reason || '';
      main.appendChild(reason);

      const detailFields = ['reason_code', 'provider', 'model'].filter(field => row[field]);
      if (detailFields.length > 0) {
        const details = document.createElement('details');
        details.className = 'backlog-row-details';
        const summary = document.createElement('summary');
        summary.textContent = 'details';
        details.appendChild(summary);
        detailFields.forEach((field) => {
          const detail = document.createElement('span');
          detail.textContent = `${field}=${row[field]}`;
          details.appendChild(detail);
        });
        main.appendChild(details);
      }

      rowEl.appendChild(main);

      if (row.depth) {
        const depth = document.createElement('span');
        depth.className = 'backlog-depth';
        depth.textContent = String(row.depth);
        rowEl.appendChild(depth);
      }

      const actions = document.createElement('div');
      actions.className = 'backlog-row-actions';

      const process = document.createElement('button');
      process.type = 'button';
      process.className = 'backlog-action';
      process.dataset.day = row.day || '';
      process.dataset.flavor = 'process-now';
      process.textContent = backlogCopy.action_process_now || '';
      actions.appendChild(process);

      const redo = document.createElement('button');
      redo.type = 'button';
      redo.className = 'backlog-action';
      redo.dataset.day = row.day || '';
      redo.dataset.flavor = 'from-scratch';
      redo.dataset.confirm = backlogCopy.confirm_redo_scratch || '';
      redo.textContent = backlogCopy.action_redo_scratch || '';
      actions.appendChild(redo);

      const status = document.createElement('span');
      status.className = 'backlog-action-status';
      actions.appendChild(status);

      rowEl.appendChild(actions);
      rowsHost.appendChild(rowEl);
    });
    wireBacklogReprocessActions();
  }

  function renderAgentErrorsState(agentErrors) {
    const data = agentErrors || {};
    state.agentErrorsOk = data.ok !== false;
    seedAgentErrors(Array.isArray(data.items) ? data.items : []);
    if (elements.glanceErrorsValue) {
      elements.glanceErrorsValue.textContent = state.agentErrorsOk ? String(data.count || 0) : '—';
    }
    if (elements.glanceErrorsLabel) {
      elements.glanceErrorsLabel.textContent = data.label || 'errors today';
    }
    updateStatusSummary();
  }

  async function loadHealthState() {
    clearHealthStateError();
    try {
      const payload = await getJson('/app/health/api/state');
      renderBacklogState(payload.backlog);
      renderAgentErrorsState(payload.agent_errors);
    } catch (error) {
      renderHealthStateError(error);
    }
  }

  let timeoutFired = false;
  let programmaticScroll = false;
  const STALE_MS = 30000;
  // Human-readable service names
  // Owner words for every service the supervisor can report. A service with no
  // entry here is a codename on the trust page, so the map covers the ones that
  // ship (spl, parakeet) rather than letting them through raw (X-18).
  const SERVICE_NAMES = {
    supervisor: 'system manager',
    convey: 'web interface',
    cortex: 'ai engine',
    sense: 'media processor',
    observe: 'screen & audio',
    think: 'background analysis',
    importer: 'file importer',
    schedule: 'task scheduler',
    spl: 'private link',
    parakeet: 'transcription',
  };

  function serviceName(internal) {
    return SERVICE_NAMES[internal] || String(internal || '').replace(/[_:-]+/g, ' ');
  }

  // Readable names for talent ids that do not humanize cleanly; everything else
  // falls back to the humanized id. Mirrors thinking.js talentLabel so the two
  // surfaces name the same talent the same way (X-18).
  const TALENT_NAMES = {
    'entities:detection': 'entity detection',
  };

  function talentName(name) {
    const id = String(name || '');
    if (!id) return '';
    return TALENT_NAMES[id] || id.replace(/[_:]+/g, ' ');
  }

  // Owner words for the schedule keys the server ships. Returns null for an
  // unmapped key so callers can fall back to a generic phrase instead of
  // printing a colon-delimited task identifier (G3-102).
  const SCHEDULE_NAMES = {
    'brain': 'processing check',
    'cadence': 'processing schedule',
    'heartbeat': 'journal review',
    'facet-candidates': 'facet suggestions',
    'maintenance:backup:run': 'backup',
    'maintenance:backup:prune': 'backup cleanup',
    'maintenance:backup:verify': 'backup check',
    'maintenance:backup:offload': 'original media cleanup',
    'maintenance:health:mark-raw': 'original media review',
    'maintenance:health:prune-logs': 'log cleanup',
    'maintenance:speakers:discover-voices': 'voice discovery',
    'maintenance:speakers:candidate-pair-suggestions': 'speaker suggestions',
    'maintenance:speakers:name-variants': 'speaker name suggestions',
    'maintenance:speakers:consolidate-pool': 'speaker cleanup',
  };

  function scheduleName(name) {
    return SCHEDULE_NAMES[String(name || '')] || null;
  }

  function sweepUnresolvedSkeletons() {
    if (timeoutFired) return;
    timeoutFired = true;
    const targets = [
      elements.vitalsStatus,
      elements.serviceDots,
      elements.healthValue,
    ];
    for (const el of targets) {
      if (el && el.querySelector('.skeleton, .skeleton-dark')) {
        el.textContent = 'unavailable';
        el.classList.add('unavailable');
      }
    }
    updateVitalsA11y();
  }

  function armSkeletonTimeout() {
    setTimeout(sweepUnresolvedSkeletons, 7000);
  }

  // Utility functions
  function formatElapsed(seconds) {
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${minutes}m ${secs}s`;
  }

  function formatDuration(ms) {
    return formatElapsed(Math.floor(ms / 1000));
  }

  // One relative-time ladder for the whole page: the shared helper in
  // /static/relative-time.js (loaded by the shell), never a compact shadow.
  function relativeTime(ms) {
    if (typeof window.relativeTime === 'function') return window.relativeTime(ms);
    return formatElapsed(Math.floor(ms / 1000));
  }

  // Every owner-visible age on this page goes through here: spelled units from
  // the shared ladder, "just now" under a minute, and the "ago" suffix built in
  // so no caller invents its own. Abbreviated units, "a few seconds" and
  // "0 seconds ago" are all the same defect (G3-104).
  function ageAgo(ms) {
    const value = Number.isFinite(ms) && ms > 0 ? ms : 0;
    if (value < 60000) return 'just now';
    return relativeTime(value) + ' ago';
  }

	  function truncate(str, len) {
	    if (!str) return '';
	    return str.length > len ? str.substring(0, len).replace(/\s+\S*$/, '') + '…' : str;
	  }

	  function dayKeyFromTimestamp(ts) {
	    const date = new Date(ts);
	    if (Number.isNaN(date.getTime())) return '';
	    const year = String(date.getFullYear());
	    const month = String(date.getMonth() + 1).padStart(2, '0');
	    const day = String(date.getDate()).padStart(2, '0');
	    return year + month + day;
	  }

	  function todayKey() {
	    return dayKeyFromTimestamp(Date.now());
	  }

	  function resolveRecentErrorsDay(day) {
	    if (!day || day === 'today') return todayKey();
	    return /^\d{8}$/.test(day) ? day : null;
	  }

	  function recentErrorMatchesFilter(entry) {
	    const filter = state.recentErrorsFilter;
	    if (!filter) return true;
	    const requestedDay = resolveRecentErrorsDay(filter.day);
	    if (requestedDay && dayKeyFromTimestamp(entry.ts) !== requestedDay) return false;
	    if (filter.talent && entry.name !== filter.talent) return false;
	    return true;
	  }

  function getAgentId(id) {
    return String(id).slice(-4);
  }

  function formatNextRun(epochMs) {
    if (!epochMs) return '';
    const delta = epochMs - Date.now();
    if (delta < 0) return 'due now';
    // Spelled units, same ladder as every other age on the page (G3-104).
    if (delta < 60000) return 'in under a minute';
    return `in ${relativeTime(delta)}`;
  }

  function renderInfoItems(parent, items) {
    const filtered = items.filter(item => item.value != null);
    while (parent.children.length > filtered.length) {
      parent.removeChild(parent.lastChild);
    }
    filtered.forEach((item, i) => {
      let el = parent.children[i];
      if (!el) {
        el = document.createElement('div');
        el.className = 'info-item';
        const label = document.createElement('div');
        label.className = 'info-label';
        el.appendChild(label);
        el.appendChild(document.createElement('div'));
        parent.appendChild(el);
      }
      el.children[0].textContent = item.label;
      el.children[1].textContent = item.value;
    });
  }

  function registeredClientName(client) {
    return client.display_label || client.device_label || client.cid_short || client.cid || 'an unnamed device';
  }

  function registeredClientAge(client) {
    return Number.isFinite(client.capture_elapsed_ms) ? relativeTime(client.capture_elapsed_ms) : null;
  }

  function describeRegisteredClient(client) {
    const age = registeredClientAge(client);
    return age ? `${registeredClientName(client)} (${age})` : registeredClientName(client);
  }

  function byQuietestFirst(a, b) {
    return (Number.isFinite(b.capture_elapsed_ms) ? b.capture_elapsed_ms : -1)
      - (Number.isFinite(a.capture_elapsed_ms) ? a.capture_elapsed_ms : -1);
  }

  // The device-delivery half of the verdict. Returns null when device delivery
  // has nothing to say, so the caller falls through to the other signals.
  function selectDeviceVerdict() {
    const clients = state.registeredClients;
    if (!Array.isArray(clients)) {
      // No derived status is its own honest state — it is never an upgrade to green.
      return {
        key: state.registeredClientsFailed ? 'HEALTH_GLANCE_DEVICES_UNAVAILABLE' : 'HEALTH_GLANCE_CHECKING',
        vars: {},
        action: state.registeredClientsFailed ? GLANCE_DEVICES_ACTION : null,
      };
    }
    const failing = clients.filter(client => client.failing === true).sort(byQuietestFirst);
    if (failing.length === 1) {
      return {
        key: 'HEALTH_GLANCE_DEVICE_FAILING',
        vars: { device: registeredClientName(failing[0]) },
        action: GLANCE_DEVICES_ACTION,
      };
    }
    if (failing.length > 1) {
      return {
        key: 'HEALTH_GLANCE_DEVICES_FAILING',
        vars: { n: String(failing.length), devices: failing.map(registeredClientName).join(', ') },
        action: GLANCE_DEVICES_ACTION,
      };
    }
    const silent = clients
      .filter(client => client.capture_state === 'offline' || client.capture_state === 'stale')
      .sort(byQuietestFirst);
    if (silent.length === 1) {
      const age = registeredClientAge(silent[0]);
      return {
        key: age ? 'HEALTH_GLANCE_DEVICE_SILENT' : 'HEALTH_GLANCE_DEVICE_SILENT_NO_AGE',
        vars: { device: registeredClientName(silent[0]), age: age || '' },
        action: GLANCE_DEVICES_ACTION,
      };
    }
    if (silent.length > 1) {
      return {
        key: 'HEALTH_GLANCE_DEVICES_SILENT',
        vars: { n: String(silent.length), devices: silent.map(describeRegisteredClient).join(', ') },
        action: GLANCE_DEVICES_ACTION,
      };
    }
    return null;
  }

	  function selectGlanceSentence(state, now) {
    const activeAgents = Array.from(state.agents.values()).filter(agent => agent.event !== 'finish' && agent.event !== 'error').length;
    const activeImports = Array.from(state.imports.values()).filter(imp => imp.event !== 'completed' && imp.event !== 'error').length;
    const staleHeartbeats = state.health?.stale_heartbeats || [];

    if (state.connected === false && connectError === true) {
      return { key: 'HEALTH_GLANCE_SERVICES_UNREACHABLE', vars: {} };
    }

    if (state.crashed.size > 0 || staleHeartbeats.length > 0) {
      const names = Array.from(new Set([
        ...Array.from(state.crashed.keys()),
        ...staleHeartbeats,
      ])).sort();
      return {
        key: names.length === 1 ? 'HEALTH_GLANCE_SERVICE_ATTENTION' : 'HEALTH_GLANCE_SERVICES_ATTENTION',
        vars: {
          n: String(names.length),
          service_names: names.map(serviceName).join(', '),
        },
      };
    }

    if (brainSnapshot && ['blocked', 'unhealthy', 'unknown'].includes(brainSnapshot.state)) {
      return {
        key: 'HEALTH_GLANCE_BRAIN_ATTENTION',
        vars: { headline: brainSnapshot.headline || '' },
      };
    }

    // Device delivery is part of the verdict, not only of the rows below it.
    const deviceVerdict = selectDeviceVerdict();
    if (deviceVerdict) return deviceVerdict;

    const clients = Array.from(state.clients.values());
    if (clients.length > 0 && clients.every(client => (now - client.lastSeen) >= STALE_MS)) {
      const ageMs = Math.min(...clients.map(client => now - client.lastSeen));
      return {
        key: 'HEALTH_GLANCE_CLIENT_SILENT',
        vars: { age: relativeTime(ageMs) },
        action: GLANCE_DEVICES_ACTION,
      };
    }

	    if (activeAgents > 0 || activeImports > 0) {
      const catchingUp = activeAgents + activeImports;
      return {
        key: 'HEALTH_GLANCE_CATCHING_UP',
        vars: {
          n: String(catchingUp),
          tasks: catchingUp === 1 ? 'task' : 'tasks',
          age: ageAgo(now - (state.lastEventTs || now)),
        },
      };
    }

    if (state.services.size > 0 || state.crashed.size > 0) {
      return {
        key: 'HEALTH_GLANCE_OK',
        vars: { age: ageAgo(now - (state.lastEventTs || now)) },
      };
    }

    return null;
	  }

  function formatGlanceSentence(selection) {
    if (!selection) return '';
    let text = HEALTH_GLANCE_COPY[selection.key] || '';
    for (const [key, value] of Object.entries(selection.vars || {})) {
      text = text.replaceAll('{' + key + '}', value);
    }
    return text;
  }

  const GLANCE_ATTENTION_KEYS = new Set([
    'HEALTH_GLANCE_SERVICES_UNREACHABLE',
    'HEALTH_GLANCE_SERVICE_ATTENTION',
    'HEALTH_GLANCE_SERVICES_ATTENTION',
    'HEALTH_GLANCE_BRAIN_ATTENTION',
    'HEALTH_GLANCE_DEVICE_FAILING',
    'HEALTH_GLANCE_DEVICES_FAILING',
    'HEALTH_GLANCE_DEVICE_SILENT',
    'HEALTH_GLANCE_DEVICE_SILENT_NO_AGE',
    'HEALTH_GLANCE_DEVICES_SILENT',
    'HEALTH_GLANCE_DEVICES_UNAVAILABLE',
    'HEALTH_GLANCE_CLIENT_SILENT',
  ]);

  // Anything the verdict calls out has to be somewhere the owner can go.
  function renderGlanceVerdict(selection) {
    const line = elements.healthGlanceSentence;
    line.textContent = formatGlanceSentence(selection);
    const action = selection && selection.action;
    if (action) {
      line.appendChild(document.createTextNode(' '));
      const link = document.createElement('a');
      link.className = 'glance-action';
      link.href = action.href;
      link.textContent = `${action.label} →`;
      line.appendChild(link);
    }
    elements.healthGlance?.classList.toggle(
      'glance-attention',
      Boolean(selection && GLANCE_ATTENTION_KEYS.has(selection.key))
    );
  }

  function updateStatusSummary() {
    const now = Date.now();
    const selection = selectGlanceSentence(state, now);
    renderGlanceVerdict(selection);

    const today = todayKey();
    if (!state.agentErrorsOk) {
      elements.glanceErrorsValue.textContent = '—';
      elements.glanceErrorsLabel.textContent = 'errors today';
    } else {
      const errorsToday = state.recentErrors.filter(error => dayKeyFromTimestamp(error.ts) === today).length;
      elements.glanceErrorsValue.textContent = String(errorsToday);
      elements.glanceErrorsLabel.textContent = (errorsToday === 1 ? 'error today' : 'errors today');
    }
  }

  function renderBrainHealth() {
    const box = elements.brainHealthStatus;
    if (!box) return;
    const brain = brainSnapshot || {};
    const identity = brain.identity || {};
    const evidence = brain.evidence || {};
    const component = brain.failing_component ? ` (${brain.failing_component})` : '';
    const lines = [];
    if (brain.headline) lines.push(brain.headline);
    if (identity.lane && identity.provider && identity.model) {
      if (brain.state === 'ready') {
        // age_text is the server's compact form ("10h"); the page speaks one
        // vocabulary, so derive from age_seconds and fall back only if absent.
        const checkedAge = Number.isFinite(evidence.age_seconds)
          ? ageAgo(evidence.age_seconds * 1000)
          : (evidence.age_text ? evidence.age_text + ' ago' : '');
        const checked = checkedAge ? `, checked ${checkedAge}` : '';
        lines.push(`${window.JournalFormat.processingLane(identity.lane)}${checked}`);
      } else {
        lines.push(`${window.JournalFormat.processingLane(identity.lane)}: ${brain.reason_text || ''}${component}`);
      }
    } else if (identity.lane || identity.provider || identity.model) {
      lines.push(`${brain.reason_text || ''}${component}`);
    }
    const identityOpen = box.querySelector('details')?.open || false;
    const identityFocused = document.activeElement === box.querySelector('summary');
    box.innerHTML = '';
    lines.forEach((line) => {
      const p = document.createElement('p');
      p.textContent = line;
      box.appendChild(p);
    });
    const identityText = [identity.lane, identity.provider, identity.model].filter(Boolean).join(' · ');
    // Token counts are provider internals: real, but not something the owner
    // acts on, so they sit in the processing disclosure rather than leading the
    // page as a headline figure (G3-106).
    const tokensText = Number.isFinite(state.todayTokens)
      ? `${window.JournalFormat.compactTokens(state.todayTokens)} tokens used today`
      : '';
    if (identityText || tokensText) {
      const details = document.createElement('details');
      details.open = identityOpen;
      const summary = document.createElement('summary');
      summary.textContent = 'processing details';
      details.append(summary);
      if (identityText) {
        const value = document.createElement('p');
        value.textContent = identityText;
        details.append(value);
      }
      if (tokensText) {
        const tokens = document.createElement('p');
        const link = document.createElement('a');
        link.href = '/app/stats/#tokens';
        link.textContent = tokensText;
        tokens.append(link);
        details.append(tokens);
      }
      box.appendChild(details);
      if (identityFocused) summary.focus({preventScroll: true});
    }
    const action = brain.action || null;
    const button = elements.brainCheckBtn;
    if (!button) return;
    if (!action?.label) {
      button.hidden = true;
      button.onclick = null;
      return;
    }
    button.textContent = action.label;
    button.hidden = false;
    if (action.href) {
      button.onclick = () => {
        window.location.href = action.href;
      };
    } else if (action.refresh) {
      button.onclick = () => requestBrainCheck();
    } else {
      button.onclick = null;
    }
  }

  function requestBrainCheck() {
    const button = elements.brainCheckBtn;
    if (button) button.disabled = true;
    return fetch('/app/health/api/brain/check', {method: 'POST'})
      .then(r => r.json())
      .then(payload => {
        brainSnapshot = payload.brain || brainSnapshot;
        renderBrainHealth();
        updateStatusSummary();
      })
      .catch(() => {
        renderBrainHealth();
      })
      .finally(() => {
        if (button) button.disabled = false;
      });
  }

  function updateAllQuiet() {
    const allHidden = state.supervisorSeen && state.cortexSeen && state.tasks.length === 0 &&
      elements.cortexSection.classList.contains('hidden') &&
      elements.importerSection.classList.contains('hidden') &&
      elements.thinkCard.classList.contains('hidden');
    if (allHidden) updateAllQuietContent();
    elements.allQuietCard.classList.toggle('hidden', !allHidden);
  }

  function updateAllQuietContent() {
    const container = elements.idleCardStats;
    let idx = 0;

    function ensureChild(i) {
      let el = container.children[i];
      if (!el) {
        el = document.createElement('div');
        el.className = 'idle-stat';
        container.appendChild(el);
      }
      return el;
    }

    if (state.lastAgentFinishTs) {
      const el = ensureChild(idx++);
      el.textContent = 'last talent finished ' + ageAgo(Date.now() - state.lastAgentFinishTs);
      el.style.color = '';
    }

    const errCount = state.recentErrors.length;
    const errEl = ensureChild(idx++);
    if (!state.agentErrorsOk) {
      errEl.textContent = "couldn't check talent errors today.";
      errEl.style.color = '#92400e';
    } else if (errCount > 0) {
      errEl.textContent = errCount + ' recent error' + (errCount !== 1 ? 's' : '');
      errEl.style.color = '#dc2626';
    } else {
      errEl.textContent = 'no recent errors';
      errEl.style.color = '';
    }

    const now = Date.now();
    const nextSchedule = state.schedules
      .filter(s => s.next_run && s.next_run > now)
      .sort((a, b) => a.next_run - b.next_run)[0];
    if (nextSchedule) {
      const el = ensureChild(idx++);
      // The raw schedule key is a task identifier, not a name the owner chose.
      // Unmapped keys get the generic phrase; the exact key stays in the
      // schedules disclosure in the vitals bar (G3-102).
      const label = scheduleName(nextSchedule.name);
      el.textContent = label
        ? 'next: ' + label + ' ' + formatNextRun(nextSchedule.next_run)
        : 'next scheduled run ' + formatNextRun(nextSchedule.next_run);
      el.style.color = '';
    }

    while (container.children.length > idx) {
      container.removeChild(container.lastChild);
    }
  }

  function updateLogsBadge() {
    if (state.deepLinkMode) return;
    let text = state.logTotalCount + ' line' + (state.logTotalCount !== 1 ? 's' : '');
    if (state.lastLogTs) {
      const d = new Date(state.lastLogTs);
      const hh = String(d.getHours()).padStart(2, '0');
      const mm = String(d.getMinutes()).padStart(2, '0');
      const ss = String(d.getSeconds()).padStart(2, '0');
      text += ' · ' + hh + ':' + mm + ':' + ss;
    }
    elements.logsSummaryBadge.textContent = text;
  }
  updateLogsBadge();

  function isAtBottom(viewport, tol = 50) {
    return viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight <= tol;
  }

  function scrollLogsToBottom(viewport = elements.logsViewport) {
    programmaticScroll = true;
    viewport.scrollTop = viewport.scrollHeight;
    requestAnimationFrame(() => {
      programmaticScroll = false;
    });
  }

  function recentErrorGroupKey(entry) {
    if (entry.key) return entry.key;
    if (entry.reason_code) {
      return [
        entry.reason_code || '',
        entry.provider || '',
        entry.model || '',
      ].join(':');
    }
    return [
      'fallback',
      entry.type || '',
      entry.service || '',
      entry.name || '',
      entry.stage || '',
      entry.error || '',
    ].join(':');
  }

  // The row summary is interface copy, not the engine's exception string: a
  // hook name, an internal failure code and a sentence cut at one letter are
  // not something the owner can act on. The raw message keeps its place in the
  // row's disclosure panel and in the service logs (G3-103).
  function recentErrorOwnerPhrase(entry) {
    if (entry.type === 'agent') return "didn't finish";
    if (entry.type === 'import') return "didn't finish importing";
    return 'ran into a problem';
  }

  // The name an owner recognises for whatever produced the error.
  function recentErrorName(entry) {
    if (entry.type === 'agent') return talentName(entry.name);
    if (entry.type === 'import') return String(entry.name || 'import');
    return serviceName(entry.name);
  }

  function recentErrorOwnerMessage(entry) {
    if (entry.summary) return entry.summary;
    if (entry.reason_code && window.renderChatReason) {
      return window.renderChatReason(entry.reason_code, entry.provider || '').message;
    }
    return entry.error || window.CONVEY_COPY.UNKNOWN_ERROR;
  }

  function recentErrorDetailText(entry) {
    const parts = [];
    if (entry.reason_code) parts.push(`reason_code=${entry.reason_code}`);
    if (entry.provider) parts.push(`provider=${entry.provider}`);
    if (entry.model) parts.push(`model=${entry.model}`);
    if (entry.service) parts.push(`service=${entry.service}`);
    if (entry.stage) parts.push(`stage=${entry.stage}`);
    return parts.join(' · ');
  }

  function groupedRecentErrors() {
    const groups = new Map();
    state.recentErrors.forEach((entry, realIdx) => {
      if (!recentErrorMatchesFilter(entry)) return;
      const key = recentErrorGroupKey(entry);
      const existing = groups.get(key);
      if (!existing) {
        groups.set(key, {
          key,
          e: entry,
          realIdx,
          count: 1,
          lastTs: entry.ts || 0,
        });
        return;
      }
      existing.count += 1;
      if ((entry.ts || 0) >= existing.lastTs) {
        existing.e = entry;
        existing.realIdx = realIdx;
        existing.lastTs = entry.ts || 0;
      }
    });
    return Array.from(groups.values()).sort((a, b) => a.lastTs - b.lastTs);
  }

  function appendRecentError(entry) {
    if (entry?.id) {
      const dedupeIdx = state.recentErrors.findIndex(existing =>
        existing?.id === entry.id && (existing.type || '') === (entry.type || '')
      );
      if (dedupeIdx !== -1) {
        state.recentErrors[dedupeIdx] = { ...state.recentErrors[dedupeIdx], ...entry };
        updateErrorSummary();
        return;
      }
    }
    state.recentErrors.push(entry);
    if (state.recentErrors.length > 50) state.recentErrors.shift();
    updateErrorSummary();
  }

  let recentErrorPanelSeq = 0;

  function updateErrorSummary() {
    if (state.recentErrors.length === 0 && !state.recentErrorsFilter && state.agentErrorsOk) {
      elements.errorSummary.classList.add('hidden');
      return;
    }

    elements.errorSummary.classList.remove('hidden');
    const container = elements.errorSummaryContent;
    const recent = groupedRecentErrors().slice(-10).reverse();

    const newKeys = new Set();
    const entries = recent.map(group => {
      const { e, realIdx, key, count } = group;
      newKeys.add(key);
      return { e, realIdx, key, count };
    });

    const existingByKey = new Map();
    for (const child of Array.from(container.children)) {
      const k = child.getAttribute('data-key');
      if (k) existingByKey.set(k, child);
    }

    for (const [k, child] of existingByKey) {
      if (!newKeys.has(k)) container.removeChild(child);
    }

    for (const child of Array.from(container.children)) {
      if (!child.getAttribute('data-key')) container.removeChild(child);
    }

    if (!state.agentErrorsOk && entries.length > 0) {
      const degraded = document.createElement('div');
      degraded.style.cssText = 'padding: 0.3em 0; font-size: 0.85em; color: #92400e;';
      degraded.textContent = "couldn't check talent errors today.";
      container.appendChild(degraded);
    }

    if (state.recentErrorsFilter) {
      const filter = state.recentErrorsFilter;
      const label = document.createElement('div');
      label.style.cssText = 'padding: 0.2em 0 0.45em; font-size: 0.8em; color: #6b7280; display: flex; gap: 0.5em; align-items: center;';
      const dayText = filter.day === 'today' ? "today's" : filter.day;
      label.appendChild(document.createTextNode(`showing ${dayText} errors`));
      if (filter.talent) {
        label.appendChild(document.createTextNode(` for ${filter.talent}`));
      }
      const clearBtn = document.createElement('button');
      clearBtn.type = 'button';
      clearBtn.setAttribute('data-action', 'clear-error-filter');
      clearBtn.className = 'error-advice-link';
      clearBtn.textContent = 'clear';
      label.appendChild(clearBtn);
      container.appendChild(label);
    }

    if (entries.length === 0) {
      const empty = document.createElement('div');
      empty.style.cssText = 'padding: 0.3em 0; font-size: 0.85em; color: #6b7280;';
      empty.textContent = !state.agentErrorsOk
        ? "couldn't check talent errors today."
        : (state.recentErrorsFilter ? 'no matching recent errors yet.' : 'no recent errors.');
      container.appendChild(empty);
      if (state.pendingRecentErrorsFocus) {
        state.pendingRecentErrorsFocus = false;
        requestAnimationFrame(() => elements.errorSummary.scrollIntoView({ behavior: 'smooth', block: 'start' }));
      }
      return;
    }

    for (const { e, realIdx, key, count } of entries) {
      let row = existingByKey.get(key);
      if (!row) {
        row = document.createElement('div');
        row.setAttribute('data-key', key);
        row.style.cssText = 'padding: 0.3em 0; font-size: 0.85em; color: #374151; border-bottom: 1px solid #e5e7eb; display: flex; align-items: baseline; gap: 0.4em; flex-wrap: wrap;';

        const panelId = 'recent-error-panel-' + (++recentErrorPanelSeq);

        const summaryBtn = document.createElement('button');
        summaryBtn.type = 'button';
        summaryBtn.setAttribute('data-action', 'toggle-error');
        summaryBtn.setAttribute('aria-expanded', 'false');
        summaryBtn.setAttribute('aria-controls', panelId);
        summaryBtn.style.cssText = 'flex: 1 1 auto; min-width: 0; text-align: left; background: none; border: none; padding: 0; margin: 0; font: inherit; color: inherit; cursor: pointer;';
        row.appendChild(summaryBtn);

        const actions = document.createElement('span');
        actions.style.cssText = 'margin-left: auto; display: flex; gap: 0.3em;';
        const viewBtn = document.createElement('button');
        viewBtn.setAttribute('data-action', 'view-logs');
        viewBtn.className = 'error-action-btn';
        viewBtn.textContent = 'view logs';
        actions.appendChild(viewBtn);
        const dismissBtn = document.createElement('button');
        dismissBtn.setAttribute('data-action', 'dismiss');
        dismissBtn.className = 'error-dismiss-btn';
        dismissBtn.textContent = 'dismiss';
        actions.appendChild(dismissBtn);
        row.appendChild(actions);

        const panel = document.createElement('div');
        panel.id = panelId;
        panel.hidden = true;
        panel.setAttribute('data-error-panel', 'true');
        panel.style.cssText = 'flex-basis: 100%; width: 100%; padding: 0.35em 0 0.1em; color: #374151;';
        row.appendChild(panel);
      }

      const icon = e.type === 'agent' ? '⚙' : e.type === 'import' ? '↓' : '⚠';
      const ago = ageAgo(Date.now() - (e.ts || Date.now()));
      const summaryBtn = row.querySelector('[data-action="toggle-error"]');
      summaryBtn.textContent = '';
      summaryBtn.appendChild(document.createTextNode(icon + ' '));
      const strong = document.createElement('strong');
      strong.textContent = recentErrorName(e);
      summaryBtn.appendChild(strong);
      summaryBtn.appendChild(document.createTextNode(' — ' + recentErrorOwnerPhrase(e) + ' '));
      if (count > 1) {
        const countSpan = document.createElement('span');
        countSpan.style.cssText = 'color: var(--ink-faint); font-size: 0.85em; font-weight: 600;';
        countSpan.textContent = `×${count} `;
        summaryBtn.appendChild(countSpan);
      }
      const timeSpan = document.createElement('span');
      timeSpan.style.cssText = 'color: var(--ink-faint); font-size: 0.85em;';
      timeSpan.textContent = ago;
      summaryBtn.appendChild(timeSpan);

      const panel = row.querySelector('[data-error-panel]');
      panel.textContent = '';
      const fullMsg = document.createElement('div');
      fullMsg.textContent = recentErrorOwnerMessage(e);
      panel.appendChild(fullMsg);
      const detailText = recentErrorDetailText(e);
      if (detailText) {
        const tech = document.createElement('div');
        tech.style.cssText = 'margin-top: 0.3em; font-size: 0.85em; color: #6b7280;';
        tech.textContent = detailText;
        panel.appendChild(tech);
      }

      const viewBtn = row.querySelector('[data-action="view-logs"]');
      viewBtn.setAttribute('data-error-index', realIdx);
      const dismissBtn = row.querySelector('[data-action="dismiss"]');
      dismissBtn.setAttribute('data-error-index', realIdx);

      container.appendChild(row);
    }

    if (state.pendingRecentErrorsFocus) {
      state.pendingRecentErrorsFocus = false;
      requestAnimationFrame(() => elements.errorSummary.scrollIntoView({ behavior: 'smooth', block: 'start' }));
    }

    // Error-type-specific advice templates
    const types = new Set(recent.map(({ e }) => e.type));
    if (types.has('agent')) {
      const advice = document.createElement('div');
      advice.style.cssText = 'padding: 0.25em 0; font-size: 0.8em; color: #9ca3af;';
      advice.appendChild(document.createTextNode('Talent errors usually resolve on the next run. '));
      const btn = document.createElement('button');
      btn.setAttribute('data-action', 'view-logs');
      btn.setAttribute('data-service', 'cortex');
      btn.className = 'error-advice-link';
      btn.textContent = 'view ai engine logs';
      advice.appendChild(btn);
      advice.appendChild(document.createTextNode(' for details.'));
      container.appendChild(advice);
    }
    if (types.has('import')) {
      const importErr = recent.find(({ e }) => e.type === 'import')?.e;
      const stageText = importErr?.stage && importErr.stage !== 'unknown' ? ' at ' + importErr.stage : '';
      const advice = document.createElement('div');
      advice.style.cssText = 'padding: 0.25em 0; font-size: 0.8em; color: #9ca3af;';
      advice.appendChild(document.createTextNode('import failed' + stageText + '. check the source file and retry, or '));
      const btn = document.createElement('button');
      btn.setAttribute('data-action', 'view-logs');
      btn.setAttribute('data-service', 'importer');
      btn.className = 'error-advice-link';
      btn.textContent = 'view file importer logs';
      advice.appendChild(btn);
      advice.appendChild(document.createTextNode('.'));
      container.appendChild(advice);
    }
    // Default fallback for any unrecognized error types (connection/service errors)
    if (Array.from(types).some(t => t !== 'agent' && t !== 'import')) {
      const advice = document.createElement('div');
      advice.style.cssText = 'padding: 0.25em 0; font-size: 0.8em; color: #9ca3af;';
      advice.appendChild(document.createTextNode('service error detected. '));
      const btn = document.createElement('button');
      btn.setAttribute('data-action', 'view-logs');
      btn.setAttribute('data-service', 'supervisor');
      btn.className = 'error-advice-link';
      btn.textContent = 'view system manager logs';
      advice.appendChild(btn);
      advice.appendChild(document.createTextNode(' for details.'));
      container.appendChild(advice);
    }
  }

	  function viewServiceLogs(service, ts) {
	    // Set filter to the relevant service
	    const option = Array.from(elements.logServiceFilter.options).find(o => o.value === service);
	    if (option) {
	      elements.logServiceFilter.value = service;
	    }
	    if (ts) state.pendingLogAnchor = ts;
	    // Expand logs if collapsed
	    if (state.logsCollapsed) {
      state.logsCollapsed = false;
      document.querySelector('.logs-card').classList.remove('logs-collapsed');
      elements.logsCollapseIndicator.textContent = '▼ hide';
      document.querySelector('.logs-header').setAttribute('aria-expanded', 'true');
    }
    renderLogs();
    document.querySelector('.logs-card').scrollIntoView({ behavior: 'smooth' });
  }

  elements.errorSummaryContent.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-action]');
    if (!btn) return;
    const action = btn.dataset.action;
    if (action === 'toggle-error') {
      const expanded = btn.getAttribute('aria-expanded') === 'true';
      btn.setAttribute('aria-expanded', expanded ? 'false' : 'true');
      const panel = document.getElementById(btn.getAttribute('aria-controls'));
      if (panel) panel.hidden = expanded;
      return;
    }
    if (action === 'dismiss') {
      const idx = parseInt(btn.dataset.errorIndex, 10);
      if (!isNaN(idx) && idx >= 0 && idx < state.recentErrors.length) {
        state.recentErrors.splice(idx, 1);
        updateErrorSummary();
        updateStatusSummary();
      }
	    } else if (action === 'view-logs') {
      // If data-service is set (from advice template), use it directly
      const service = btn.dataset.service;
      if (service) {
        viewServiceLogs(service);
        return;
      }
      // Otherwise derive from the error entry
      const idx = parseInt(btn.dataset.errorIndex, 10);
	      if (!isNaN(idx) && idx >= 0 && idx < state.recentErrors.length) {
	        const err = state.recentErrors[idx];
	        const svc = err.service || (err.type === 'agent' ? 'cortex' : err.type === 'import' ? 'importer' : 'supervisor');
	        viewServiceLogs(svc, err.ts);
	      }
	    } else if (action === 'clear-error-filter') {
	      state.recentErrorsFilter = null;
	      updateErrorSummary();
	    }
	  });

  elements.importerGrid.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-action="retry"]');
    if (!btn) return;
    const importId = btn.dataset.importId;
    if (!importId) return;
    const card = btn.closest('[data-key]');
    const errorEl = card?.querySelector('.activity-card-error');
    btn.disabled = true;
    btn.textContent = 'retrying…';
    window.apiJson('/app/health/api/retry-import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ import_id: importId })
    })
      .then(() => {
        btn.textContent = 'retry sent';
        btn.style.color = '#9ca3af';
        if (errorEl) {
          errorEl.textContent = '';
        }
      })
      .catch((err) => {
        window.logError(err, { context: 'health: retry-import failed' });
        btn.disabled = false;
        btn.textContent = 'retry';
        btn.style.color = '';
        if (errorEl) {
          errorEl.textContent = err.serverMessage || 'retry failed';
        }
      });
  });

  // Client-side elapsed time updater
  let elapsedTimer = null;
  function updateElapsed() {
    // Update agent elapsed times based on start timestamp
    let needsUpdate = false;
    const now = Date.now();
    state.agents.forEach((agent, id) => {
      if (agent.startTs && agent.event !== 'finish' && agent.event !== 'error') {
        agent.elapsed_seconds = Math.floor((now - agent.startTs) / 1000);
        needsUpdate = true;
      }
    });
    // Stop timer when no agents remain
    if (state.agents.size === 0) {
      clearInterval(elapsedTimer);
      elapsedTimer = null;
      return;
    }
    if (needsUpdate) {
      updateCortexGrid();
    }
  }

  function startElapsedTimer() {
    if (elapsedTimer || document.hidden) return;
    elapsedTimer = setInterval(updateElapsed, 1000);
  }

  function updateVitalsA11y() {
    const sections = document.querySelectorAll('.vitals-content > .vitals-section');
    const runningCount = state.services.size;
    const crashedCount = Array.from(state.crashed.values()).filter(info => info.phase !== 'backoff').length;
    const retryingCount = state.crashed.size - crashedCount;
    const serviceParts = [];
    if (runningCount > 0) serviceParts.push(runningCount + ' active');
    if (retryingCount > 0) serviceParts.push(retryingCount + ' retrying');
    if (crashedCount > 0) serviceParts.push(crashedCount + ' needs attention');
    sections[0]?.setAttribute('aria-label', 'Services: ' + (state.supervisorSeen ? serviceParts.join(', ') || 'none' : 'unavailable'));

    sections[1]?.setAttribute('aria-label', 'Talents: ' + (state.cortexSeen ? state.agentCount + ' running' : 'unavailable'));
    sections[2]?.setAttribute('aria-label', 'Tasks: ' + (state.supervisorSeen ? state.tasks.length + ' active' : 'unavailable'));

    const staleCount = state.health?.stale_heartbeats?.length || 0;
    let healthLabel = timeoutFired ? 'unavailable' : 'loading';
    if (staleCount > 0) {
      healthLabel = 'warning, ' + staleCount + ' service' + (staleCount === 1 ? '' : 's') + ' not responding';
    } else if (crashedCount > 0) {
      healthLabel = 'error, services need attention';
    } else if (retryingCount > 0) {
      healthLabel = 'warning, services retrying';
    } else if (state.health) {
      healthLabel = 'ok';
    }
    sections[3]?.setAttribute('aria-label', 'Health: ' + healthLabel);

    const queueEntries = Object.entries(state.queues).filter(([, count]) => count > 0);
    sections[4]?.setAttribute(
      'aria-label',
      'Queues: ' + (queueEntries.map(([cmd, count]) => cmd + ' ' + count).join(', ') || (state.supervisorSeen ? 'none' : 'unavailable'))
    );

    sections[5]?.setAttribute(
      'aria-label',
      'Schedules: ' + (state.schedules.length ? state.schedules.length + ' scheduled' : state.supervisorSeen ? 'none' : 'unavailable')
    );
  }

  // Update vitals bar
  function updateVitals() {
    // Combine running and crashed services
    const allServices = [];

    // Running services first
    state.services.forEach((info, name) => {
      allServices.push({ name, info, crashed: false });
    });

    // Then crashed services
    state.crashed.forEach((info, name) => {
      allServices.push({ name, info, crashed: true });
    });

    if (allServices.length > 0) {
      const container = elements.serviceDots;
      if (!container.children.length) container.textContent = '';
      const existingByKey = new Map();
      for (const child of Array.from(container.children)) {
        const key = child.getAttribute('data-service');
        if (key) {
          existingByKey.set(key, child);
        } else {
          container.removeChild(child);
        }
      }

      const newKeys = new Set(allServices.map(s => s.name));

      for (const [key, child] of existingByKey) {
        if (!newKeys.has(key)) container.removeChild(child);
      }

      for (const { name, info, crashed } of allServices) {
        let dot = existingByKey.get(name);
        if (!dot) {
          dot = document.createElement('div');
          dot.setAttribute('data-service', name);
          const indicator = document.createElement('span');
          dot.appendChild(indicator);
          const label = document.createElement('span');
          dot.appendChild(label);
          container.appendChild(dot);
        }
        const retrying = crashed && info.phase === 'backoff';
        const statusClass = retrying ? 'restarting' : (crashed ? 'crashed' : 'active');
        const statusLabel = retrying ? 'retrying' : statusClass;
        const restartInfo = retrying ? ' (retrying)' : '';
        dot.className = retrying ? 'service-dot restarting' : (crashed ? 'service-dot crashed' : 'service-dot');
        dot.children[0].className = 'status-indicator ' + statusClass;
        dot.children[0].setAttribute('aria-label', serviceName(name) + ': ' + statusLabel + restartInfo);
        dot.children[1].setAttribute('title', name);
        dot.children[1].textContent = serviceName(name) + restartInfo;
      }
    }

    if (!state.supervisorSeen) {
      elements.serviceDots.textContent = 'unavailable';
      elements.healthValue.textContent = 'unavailable';
      updateVitalsStatus('unavailable');
    }

    // Agents count
    elements.agentsValue.firstElementChild.textContent = state.cortexSeen ? state.agentCount + ' running' : 'unavailable';

    // Tasks
    const taskCount = state.tasks.length;
    elements.tasksValue.firstElementChild.textContent = state.supervisorSeen ? taskCount + ' active' : 'unavailable';

    // Health with stale heartbeat names
    if (state.health) {
      const staleHeartbeats = state.health.stale_heartbeats || [];
      const staleCount = staleHeartbeats.length;
      const hasCrashed = Array.from(state.crashed.values()).some(info => info.phase !== 'backoff');
      const hasRetrying = state.crashed.size > 0 && !hasCrashed;

      const hv = elements.healthValue;
      if (!hv.querySelector('.health-main')) {
        hv.textContent = '';
        const main = document.createElement('span');
        main.className = 'health-main';
        hv.appendChild(main);
        const staleList = document.createElement('span');
        staleList.className = 'stale-list';
        hv.appendChild(staleList);
      }
      const mainSpan = hv.children[0];
      const staleListSpan = hv.children[1];

      if (staleCount > 0) {
        const staleNames = staleHeartbeats.map(s => serviceName(s)).join(', ');
        mainSpan.textContent = staleCount + ' service' + (staleCount === 1 ? '' : 's') + ' not responding';
        mainSpan.style.color = '#f59e0b';
        staleListSpan.textContent = '(' + staleNames + ')';
        staleListSpan.style.display = '';
        updateVitalsStatus('warning');
      } else if (hasCrashed) {
        mainSpan.textContent = 'services need attention';
        mainSpan.style.color = '#f87171';
        staleListSpan.textContent = '';
        staleListSpan.style.display = 'none';
        updateVitalsStatus('error');
      } else if (hasRetrying) {
        mainSpan.textContent = 'services retrying';
        mainSpan.style.color = '#f59e0b';
        staleListSpan.textContent = '';
        staleListSpan.style.display = 'none';
        updateVitalsStatus('warning');
      } else {
        mainSpan.textContent = 'ok';
        mainSpan.style.color = '';
        staleListSpan.textContent = '';
        staleListSpan.style.display = 'none';
        updateVitalsStatus('ok');
      }
    }

    // Queues (always visible)
    const queueEntries = Object.entries(state.queues).filter(([, count]) => count > 0);
    if (queueEntries.length > 0) {
      let wrapper = elements.queuesValue.querySelector('.vitals-chips');
      if (!wrapper) {
        elements.queuesValue.textContent = '';
        wrapper = document.createElement('div');
        wrapper.className = 'vitals-chips';
        elements.queuesValue.appendChild(wrapper);
      }
      const existingByKey = new Map();
      for (const child of Array.from(wrapper.children)) {
        existingByKey.set(child.getAttribute('data-key'), child);
      }
      const newKeys = new Set(queueEntries.map(([cmd]) => cmd));
      for (const [key, child] of existingByKey) {
        if (!newKeys.has(key)) wrapper.removeChild(child);
      }
      for (const [cmd, count] of queueEntries) {
        let chip = existingByKey.get(cmd);
        if (!chip) {
          chip = document.createElement('span');
          chip.className = 'vitals-chip';
          chip.setAttribute('data-key', cmd);
          wrapper.appendChild(chip);
        }
        chip.textContent = cmd + ': ' + count;
      }
    } else {
      elements.queuesValue.textContent = state.supervisorSeen ? 'none' : 'unavailable';
    }

    // Keep the inventory available without turning the glance into a long list.
    if (state.schedules.length > 0) {
      let details = elements.schedulesValue.querySelector('details');
      if (!details) {
        details = document.createElement('details');
        details.append(document.createElement('summary'), document.createElement('ul'));
        elements.schedulesValue.replaceChildren(details);
      }
      const schedules = [...state.schedules].sort((a, b) => Number(a.next_run || Infinity) - Number(b.next_run || Infinity));
      // "next overdue" is not a sentence: when something is already due, say so
      // and count it; otherwise say when the soonest one runs.
      const dueNow = schedules.filter(schedule => schedule.due
        || (schedule.next_run && Number(schedule.next_run) <= Date.now())).length;
      const next = formatNextRun(schedules[0].next_run);
      const tail = dueNow === 1
        ? ' · one is due now'
        : dueNow > 1
          ? ` · ${dueNow} are due now`
          : next ? ' · next ' + next : '';
      details.querySelector('summary').textContent = `${schedules.length} scheduled${tail}`;
      details.querySelector('ul').innerHTML = schedules.map(schedule => {
        const name = schedule.name || 'unnamed';
        const label = scheduleName(name) || name.replace(/^maintenance:/, '').replace(/[:_-]+/g, ' ');
        const next = schedule.due ? 'due now' : formatNextRun(schedule.next_run);
        return `<li>${escapeHtml(label)}${next ? ' · ' + escapeHtml(next) : ''}<code>${escapeHtml(name)}</code></li>`;
      }).join('');
    } else {
      elements.schedulesValue.textContent = state.supervisorSeen ? 'none' : 'unavailable';
    }

    updateVitalsA11y();
    updateStatusSummary();
  }

  function updateVitalsStatus(status) {
    const el = elements.vitalsStatus;
    el.classList.remove('warning', 'error', 'unavailable');

    // Ensure stable child structure: [indicator, text, severity-label]
    if (!el.querySelector('.status-indicator') || el.children.length < 3 || !el.querySelector('.severity-label')) {
      el.textContent = '';
      const indicator = document.createElement('span');
      indicator.className = 'status-indicator';
      el.appendChild(indicator);
      const text = document.createElement('span');
      el.appendChild(text);
      const severity = document.createElement('span');
      severity.className = 'severity-label';
      el.appendChild(severity);
    }

    const indicator = el.children[0];
    const text = el.children[1];
    const severity = el.children[2];

    // This chip reads service and process state only. It is NOT the page
    // verdict: device delivery, processing and the backlog do not reach it, so
    // its words stay scoped to services and the glance sentence above carries
    // the worst signal for the whole page (G3-101).
    if (status === 'ok') {
      indicator.className = 'status-indicator active';
      indicator.setAttribute('aria-label', 'services: all running');
      text.textContent = 'all services running';
      severity.textContent = '';
    } else if (status === 'warning') {
      indicator.className = 'status-indicator restarting';
      indicator.setAttribute('aria-label', 'services: warning');
      text.textContent = 'some services slow to respond';
      severity.textContent = 'warning';
      el.classList.add('warning');
    } else if (status === 'error') {
      indicator.className = 'status-indicator crashed';
      indicator.setAttribute('aria-label', 'services: need attention');
      text.textContent = 'services need attention';
      severity.textContent = 'error';
      el.classList.add('error');
    } else {
      indicator.className = 'status-indicator';
      indicator.setAttribute('aria-label', 'services: status unavailable');
      text.textContent = 'service status unavailable';
      severity.textContent = '';
      el.classList.add('unavailable');
    }
  }

  // What the observe card can honestly say when no stream is reporting live.
  // The registered-device list is a separate population from the observe
  // stream, so "unavailable" is only true when that list is unreadable too.
  function observeQuietState() {
    if (!Array.isArray(state.registeredClients)) {
      return state.registeredClientsFailed
        ? { badge: 'unavailable', unavailable: true, heading: 'device activity is unavailable right now.' }
        : { badge: 'checking', unavailable: false, heading: 'checking device activity…' };
    }
    if (state.registeredClients.length === 0) {
      return { badge: 'no devices', unavailable: false, heading: 'no devices are linked to your journal yet.' };
    }
    const adding = state.registeredClients
      .filter(client => client.capture_state === 'active')
      .sort((a, b) => (Number.isFinite(a.capture_elapsed_ms) ? a.capture_elapsed_ms : Infinity)
        - (Number.isFinite(b.capture_elapsed_ms) ? b.capture_elapsed_ms : Infinity));
    if (adding.length > 0) {
      const age = registeredClientAge(adding[0]);
      const name = registeredClientName(adding[0]);
      return {
        badge: 'adding',
        unavailable: false,
        heading: age
          ? `${name} added to your journal ${ageAgo(adding[0].capture_elapsed_ms)}. live detail isn't being reported right now.`
          : `${name} is adding to your journal. live detail isn't being reported right now.`,
      };
    }
    return { badge: 'quiet', unavailable: false, heading: 'no device is reporting live activity right now.' };
  }

  // Update observe mode badge
  function updateObserveMode(displayedClient = null) {
    if (state.clients.size === 0) {
      elements.observeModeBadge.className = 'health-badge idle';
      elements.observeModeLabel.textContent = observeQuietState().badge;
      return;
    }

    const mode = (displayedClient || state.clients.get(state.localHost))?.mode;
    const badge = elements.observeModeBadge;
    const label = elements.observeModeLabel;

    if (mode === 'screencast') {
      badge.className = 'health-badge recording';
      label.textContent = 'taking in your screen';
    } else if (mode === 'tmux') {
      badge.className = 'health-badge tmux';
      label.textContent = 'terminal sessions';
    } else if (mode === 'idle') {
      badge.className = 'health-badge idle';
      label.textContent = 'idle';
    } else {
      badge.className = 'health-badge idle';
      label.textContent = 'idle';
    }
  }

  // Update observe card
  function updateObserve() {
    const quiet = state.clients.size === 0 ? observeQuietState() : null;
    document.querySelector('.observe-card').dataset.unavailable = String(Boolean(quiet && quiet.unavailable));
    if (quiet) {
      elements.observeEmpty.classList.remove('hidden');
      const heading = elements.observeEmpty.querySelector('.surface-state-heading');
      heading.textContent = quiet.heading;
      elements.observeContent.classList.add('hidden');
      elements.observeSourceNote.classList.add('hidden');
      elements.observeSourceNote.textContent = '';
      updateObserveMode();
      updateStatusSummary();
      return;
    }
    elements.observeEmpty.classList.add('hidden');
    elements.observeContent.classList.remove('hidden');

    const confirmedPrimary = state.localHost ? state.clients.get(state.localHost) : null;
    const fallbackEntry = Array.from(state.clients.entries())
      .filter(([stream]) => !stream.endsWith('.tmux'))
      .sort((a, b) => (b[1].lastSeen || 0) - (a[1].lastSeen || 0))[0] || null;
    const displayedStream = confirmedPrimary ? state.localHost : (fallbackEntry ? fallbackEntry[0] : null);
    const primary = confirmedPrimary || (fallbackEntry ? fallbackEntry[1] : null);
    const tmux = displayedStream ? state.clients.get(displayedStream + '.tmux') : null;
    const confirmedLocal = Boolean(state.localHost && confirmedPrimary && displayedStream === state.localHost);
    if (!confirmedLocal && displayedStream) {
      elements.observeSourceNote.textContent = state.localHost
        ? `this host's stream isn't reporting yet — showing ${displayedStream}`
        : `this host is unknown — showing ${displayedStream}`;
      elements.observeSourceNote.classList.remove('hidden');
    } else {
      elements.observeSourceNote.textContent = '';
      elements.observeSourceNote.classList.add('hidden');
    }
    const channels = [
      {
        statusEl: elements.screencastStatus,
        detailEl: elements.screencastDetail,
        idleText: 'idle',
        extract: () => {
          if (!primary?.screencast) return null;
          const recording = primary.screencast.recording;
          if (!recording) return { status: 'idle' };
          const streams = primary.screencast.streams || [];
          const elapsed = primary.screencast.window_elapsed_seconds || 0;
          const streamCount = streams.length;
          const displayLabel = streamCount === 1 ? 'display' : 'displays';
          const mins = Math.max(1, Math.round(elapsed / 60));
          return {
            status: `taking in ${streamCount} ${displayLabel}, ~${mins} min`,
            detail: streamCount > 0
              ? streams.map(s => `${s.position || 'unknown'} ${s.connector || 'unknown'}`).join(', ')
              : '',
          };
        },
      },
      {
        statusEl: elements.tmuxStatus,
        detailEl: elements.tmuxDetail,
        idleText: 'idle',
        extract: () => {
          if (!tmux?.tmux) return null;
          if (!tmux.tmux.capturing) return { status: 'idle' };
          const captures = tmux.tmux.captures || 0;
          const sessions = tmux.tmux.sessions || [];
          const elapsed = tmux.tmux.window_elapsed_seconds || 0;
          const mins = Math.max(1, Math.round(elapsed / 60));
          return {
            status: `adding ${captures} ${captures === 1 ? 'snapshot' : 'snapshots'}, ~${mins} min`,
            detail: sessions.length > 0 ? sessions.join(', ') : '',
          };
        },
      },
      {
        statusEl: elements.audioStatus,
        detailEl: elements.audioDetail,
        idleText: 'quiet',
        extract: () => {
          if (!primary?.audio) return null;
          const hits = primary.audio.threshold_hits || 0;
          const willSave = primary.audio.will_save ? ' · saving' : '';
          return {
            status: hits > 0
              ? `${hits} sound${hits === 1 ? '' : 's'} detected${willSave}`
              : 'quiet',
          };
        },
      },
      {
        statusEl: elements.activityStatus,
        detailEl: elements.activityDetail,
        idleText: 'idle',
        extract: () => {
          if (!primary?.activity) return null;
          const idleMs = primary.activity.idle_time_ms || 0;
          if (primary.activity.power_save) return { status: 'power saving' };
          if (primary.activity.screen_locked) return { status: 'screen locked' };
          if (primary.activity.sink_muted) return { status: 'audio muted' };
          return { status: `idle: ${Math.floor(idleMs/1000)}s` };
        },
      },
      {
        statusEl: elements.describeStatus,
        detailEl: elements.describeDetail,
        idleText: 'idle',
        extract: () => {
          if (!primary?.describe) return null;
          return { processor: primary.describe };
        },
      },
      {
        statusEl: elements.transcribeStatus,
        detailEl: elements.transcribeDetail,
        idleText: 'idle',
        extract: () => {
          if (!primary?.transcribe) return null;
          return { processor: primary.transcribe };
        },
      },
    ];

    updateObserveMode(primary);

    channels.forEach(ch => {
      const result = ch.extract();
      if (!result) {
        ch.statusEl.textContent = ch.idleText;
        ch.detailEl.textContent = '';
      } else if (result.processor) {
        // Describe/transcribe processor logic
        const p = result.processor;
        const running = p.running || [];
        const isRunning = running.length > 0;
        const queued = p.queued?.length || 0;
        if (isRunning && queued > 0) {
          ch.statusEl.textContent = running.length > 1
            ? `running (${running.length}, +${queued} queued)`
            : `running (+${queued} queued)`;
        } else if (isRunning) {
          ch.statusEl.textContent = running.length > 1 ? `running (${running.length})` : 'running';
        } else if (queued > 0) {
          ch.statusEl.textContent = `queued: ${queued}`;
        } else {
          ch.statusEl.textContent = 'idle';
        }
        if (isRunning && running[0]?.file) {
          ch.detailEl.textContent = truncate(running[0].file.split('/').pop(), 30);
        } else {
          ch.detailEl.textContent = '';
        }
      } else {
        ch.statusEl.textContent = result.status;
        ch.detailEl.textContent = result.detail || '';
      }
      ch.statusEl.classList.toggle('idle', ch.statusEl.textContent === ch.idleText);
    });

    updateStatusSummary();
  }

  // Update clients
  function updateClients() {
    if (state.clients.size === 0) {
      elements.clientsCard.classList.add('hidden');
      return;
    }

    elements.clientsCard.classList.remove('hidden');

    const byHost = new Map();
    for (const [stream, data] of state.clients) {
      const host = data.host || stream;
      if (!byHost.has(host)) byHost.set(host, []);
      byHost.get(host).push({ stream, data });
    }

    const now = Date.now();

    elements.clientsGrid.innerHTML = '';
    for (const [host, streams] of byHost) {
      const card = document.createElement('div');
      card.className = 'client-host-card';

      const anyActive = streams.some(({ data }) => (now - data.lastSeen) < STALE_MS);
      if (anyActive) card.classList.add('active');
      if (!anyActive) card.classList.add('stale');

      const nameEl = document.createElement('div');
      nameEl.className = 'client-host-name';
      nameEl.textContent = host;
      const statusIcon = document.createElement('span');
      statusIcon.className = 'status-indicator ' + (anyActive ? 'active' : 'stale');
      statusIcon.setAttribute('aria-label', anyActive ? 'active' : 'stale');
      nameEl.prepend(statusIcon);
      nameEl.style.display = 'flex';
      nameEl.style.alignItems = 'center';
      nameEl.style.gap = '0.4em';
      card.appendChild(nameEl);

      const platform = streams[0]?.data?.platform || '';
      if (platform) {
        const platEl = document.createElement('div');
        platEl.className = 'client-host-platform';
        platEl.textContent = platform;
        card.appendChild(platEl);
      }

      for (const { stream, data } of streams) {
        const row = document.createElement('div');
        row.className = 'client-stream-row';

        const dotIdx = stream.indexOf('.');
        const qualifier = dotIdx >= 0 ? stream.slice(dotIdx + 1) : 'desktop';

        const stale = (now - data.lastSeen) >= STALE_MS;
        if (stale) row.classList.add('stale');

        const qualEl = document.createElement('div');
        qualEl.className = 'client-stream-qualifier';
        qualEl.textContent = qualifier;
        row.appendChild(qualEl);

        const modeEl = document.createElement('div');
        modeEl.className = 'client-stream-mode';
        modeEl.textContent = data.mode || '—';
        row.appendChild(modeEl);

        if (stale) {
          const badgeEl = document.createElement('span');
          badgeEl.className = 'stale-badge';
          badgeEl.setAttribute('aria-label', 'stale — not responding');
          badgeEl.textContent = 'stale';
          row.appendChild(badgeEl);
        }

        card.appendChild(row);
      }

      elements.clientsGrid.appendChild(card);
    }
  }

	  function monthDay(ms) {
	    return new Date(ms).toLocaleDateString('en-US', { month: 'short', day: 'numeric' }).toLowerCase();
	  }

	  function registeredClientMeta(client) {
	    const lastCapture = client.last_accepted_ingest_at && Date.parse(client.last_accepted_ingest_at);
	    if (Number.isFinite(lastCapture)) {
	      const deltaMs = Date.now() - lastCapture;
	      if (deltaMs < 0) return 'last added from future';
	      return `last added ${ageAgo(deltaMs)}`;
	    }
	    const lastSeen = client.last_seen_at && Date.parse(client.last_seen_at);
	    if (!Number.isFinite(lastSeen)) return 'no material yet';
	    const deltaMs = Date.now() - lastSeen;
	    if (deltaMs < 0) return 'last seen from future';
	    return `last reported ${ageAgo(deltaMs)}`;
	  }

	  function requestBacklogReprocess(button) {
	    const row = button.closest('.backlog-row');
	    if (!row) return;
	    const day = button.dataset.day;
	    const flavor = button.dataset.flavor;
	    if (flavor === 'from-scratch' && !window.confirm(button.dataset.confirm)) {
	      return;
	    }
	    const buttons = Array.from(row.querySelectorAll('.backlog-action'));
	    const statusEl = row.querySelector('.backlog-action-status');
	    buttons.forEach((rowButton) => { rowButton.disabled = true; });
	    if (statusEl) statusEl.textContent = '';
	    window.apiJson('/app/health/api/reprocess', {
	      method: 'POST',
	      headers: { 'Content-Type': 'application/json' },
	      body: JSON.stringify({ day, flavor })
	    })
	      .then((result) => {
	        if (!statusEl) return;
	        if (result && (result.status === 'already_complete' || result.status === 'held_by_backoff')) {
	          statusEl.textContent = result.message || '';
	          buttons.forEach((rowButton) => { rowButton.disabled = false; });
	          return;
	        }
	        statusEl.textContent = backlogCopy.queued_feedback || '';
	      })
	      .catch((err) => {
	        buttons.forEach((rowButton) => { rowButton.disabled = false; });
	        window.logError(err, { context: 'health: reprocess failed' });
	        if (statusEl) {
	          statusEl.textContent = err && err.serverMessage ? err.serverMessage : 'try again';
	        }
	      });
	  }

	  function wireBacklogReprocessActions() {
	    const host = document.getElementById('backlogNeedsHand');
	    if (!host) return;
	    host.querySelectorAll('.backlog-action').forEach((button) => {
	      if (button.dataset.reprocessBound === 'true') return;
	      button.dataset.reprocessBound = 'true';
	      button.addEventListener('click', () => requestBacklogReprocess(button));
	    });
	  }

  function renderRegisteredClients(clients) {
    if (!clients || clients.length === 0) {
      elements.registeredClientsCard.classList.add('hidden');
      elements.registeredClientsStrip.innerHTML = '';
      return;
    }

    elements.registeredClientsCard.classList.remove('hidden');
    const wasOpen = elements.registeredClientsStrip.querySelector('details')?.open || false;
    elements.registeredClientsStrip.innerHTML = '';
    const unstarted = document.createElement('details');
    unstarted.open = wasOpen;
    const summary = document.createElement('summary');
    const unused = clients.filter(client => client.capture_state === 'no_capture' && !client.failing);
    summary.textContent = `devices with no material yet (${unused.length})`;
    unstarted.appendChild(summary);
    const activityRank = client => client.failing ? 0 : client.capture_state === 'active' ? 1 : 2;
    const sorted = [...clients].sort((a, b) => activityRank(a) - activityRank(b)
      || (Date.parse(b.last_accepted_ingest_at) || 0) - (Date.parse(a.last_accepted_ingest_at) || 0));
    for (const client of sorted) {
      let stateClass = ['connected', 'stale', 'disconnected'].includes(client.state)
        ? client.state
        : 'disconnected';
      let labelText = client.capture_state || client.label || 'unknown';
      if (client.capture_state === 'active') stateClass = 'connected';
      if (client.capture_state === 'stale') stateClass = 'stale';
      if (client.capture_state === 'offline') stateClass = 'disconnected';
      if (client.failing) {
        stateClass = 'failing';
        labelText = 'failing';
      } else if (client.capture_state === 'no_capture') {
        labelText = 'no material yet';
      } else if (client.capture_state === 'unknown') {
        labelText = 'delivery unknown';
      }
      const row = document.createElement('div');
      row.className = 'registered-client-row';

      const nameEl = document.createElement('span');
      nameEl.className = 'registered-client-name';
      nameEl.textContent = client.display_label || client.device_label || client.cid_short || client.cid || 'unnamed device';
      row.appendChild(nameEl);

      const labelEl = document.createElement('span');
      labelEl.className = `registered-client-label ${stateClass}`;
      labelEl.textContent = labelText;
      row.appendChild(labelEl);

      if (client.failing && client.ingest_rejection) {
        const rej = client.ingest_rejection;
        const parts = [];
        if (typeof rej.active_count === 'number' && isFinite(rej.active_count)
            && typeof rej.first === 'string' && Number.isFinite(Date.parse(rej.first))) {
          parts.push(rej.active_count + ' rejected since ' + monthDay(Date.parse(rej.first)));
        }
        if (rej.reason_code) parts.push(rej.reason_code);
        if (parts.length) {
          const detailEl = document.createElement('span');
          detailEl.className = 'registered-client-detail';
          detailEl.textContent = parts.join(' · ');
          row.appendChild(detailEl);
        }
        const recoveryEl = document.createElement('span');
        recoveryEl.className = 'registered-client-recovery';
        recoveryEl.textContent = 'update or restart the solstone app on ' + (client.display_label || client.device_label || 'that device');
        row.appendChild(recoveryEl);
      }

      // The chip already carries this state for a device with no delivery yet;
      // printing it again on the right of the same row said nothing new.
      const metaText = registeredClientMeta(client);
      if (metaText && metaText !== labelText) {
        const metaEl = document.createElement('span');
        metaEl.className = 'registered-client-meta';
        metaEl.textContent = metaText;
        row.appendChild(metaEl);
      }

      const skewEl = document.createElement('span');
      skewEl.className = 'registered-client-skew' + (client.clock_skew ? '' : ' hidden');
      skewEl.textContent = 'clock skew';
	      row.appendChild(skewEl);

      (client.capture_state === 'no_capture' && !client.failing ? unstarted : elements.registeredClientsStrip).appendChild(row);
    }
    if (unused.length) elements.registeredClientsStrip.appendChild(unstarted);
  }

  async function loadRegisteredClients() {
    try {
      const response = await fetch('/app/network/api/clients');
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const payload = await response.json();
      if (!Array.isArray(payload.clients)) throw new Error('Invalid device list');
      state.registeredClientsFailed = false;
      state.registeredClients = payload.clients;
      renderRegisteredClients(state.registeredClients);
      updateObserve();
      document.dispatchEvent(new CustomEvent('health:devices-loaded'));
    } catch (err) {
      state.registeredClientsFailed = true;
      updateObserve();
      console.warn('Failed to load registered clients:', err);
    }
  }

  // Update cortex grid
  function updateCortexGrid() {
    const activeAgents = Array.from(state.agents.values()).filter(a => a.event !== 'finish' && a.event !== 'error');

    if (activeAgents.length === 0) {
      elements.cortexSection.classList.add('hidden');
      updateAllQuiet();
      updateStatusSummary();
      return;
    }

    elements.cortexSection.classList.remove('hidden');

    const container = elements.cortexGrid;
    const existingByKey = new Map();
    for (const child of Array.from(container.children)) {
      const k = child.getAttribute('data-key');
      if (k) existingByKey.set(k, child);
    }
    const newKeys = new Set(activeAgents.map(a => a.use_id));
    for (const [k, child] of existingByKey) {
      if (!newKeys.has(k)) container.removeChild(child);
    }
    for (const agent of activeAgents) {
      const key = agent.use_id;
      let card = existingByKey.get(key);
      if (!card) {
        card = document.createElement('div');
        card.className = 'activity-card agent-active';
        card.setAttribute('data-key', key);
        // The run id is an exact identifier, so it lives behind a disclosure
        // rather than on the face of the card. It stays at index 0 so the
        // field indexing below is unchanged, and because the card is built
        // once and only its text is updated afterwards, an open disclosure
        // survives a live refresh. G3 health leftovers.
        const idEl = document.createElement('details');
        idEl.className = 'activity-card-id';
        const idSummary = document.createElement('summary');
        idSummary.textContent = 'run id';
        idEl.appendChild(idSummary);
        const idValue = document.createElement('code');
        idValue.className = 'activity-card-id-value';
        idEl.appendChild(idValue);
        card.appendChild(idEl);
        const nameEl = document.createElement('div');
        nameEl.className = 'activity-card-name';
        card.appendChild(nameEl);
        const stateEl = document.createElement('div');
        stateEl.className = 'activity-card-state';
        card.appendChild(stateEl);
        const elapsedEl = document.createElement('div');
        elapsedEl.className = 'activity-card-elapsed';
        card.appendChild(elapsedEl);
        const providerEl = document.createElement('div');
        providerEl.className = 'activity-card-provider';
        card.appendChild(providerEl);
        container.appendChild(card);
      }
      const stateLabel = agent.event === 'thinking' ? 'thinking…' :
                        (agent.event === 'tool_start' || agent.event === 'tool_end') ? 'working…' : 'running…';
      const elapsed = agent.elapsed_seconds ? formatElapsed(agent.elapsed_seconds) : '0s';
      card.children[0].querySelector('.activity-card-id-value').textContent = '…' + getAgentId(agent.use_id);
      card.children[1].textContent = talentName(agent.name) || 'default';
      card.children[2].textContent = stateLabel;
      card.children[3].textContent = elapsed;
      card.children[4].textContent = agent.provider || 'unknown';
    }
    updateAllQuiet();
    updateStatusSummary();
  }

  // Update importer grid
  function updateImporterGrid() {
    const activeImports = Array.from(state.imports.values()).filter(i => i.event !== 'completed');

    if (activeImports.length === 0) {
      elements.importerSection.classList.add('hidden');
      updateAllQuiet();
      updateStatusSummary();
      return;
    }

    elements.importerSection.classList.remove('hidden');

    const container = elements.importerGrid;
    const existingByKey = new Map();
    for (const child of Array.from(container.children)) {
      const k = child.getAttribute('data-key');
      if (k) existingByKey.set(k, child);
    }
    const newKeys = new Set(activeImports.map(i => i.import_id));
    for (const [k, child] of existingByKey) {
      if (!newKeys.has(k)) container.removeChild(child);
    }
    for (const imp of activeImports) {
      const key = imp.import_id;
      const isError = imp.event === 'error';
      let card = existingByKey.get(key);
      if (!card) {
        card = document.createElement('div');
        card.setAttribute('data-key', key);
        const idEl = document.createElement('div');
        idEl.className = 'activity-card-id';
        card.appendChild(idEl);
        const nameEl = document.createElement('div');
        nameEl.className = 'activity-card-name';
        card.appendChild(nameEl);
        const stateEl = document.createElement('div');
        stateEl.className = 'activity-card-state';
        card.appendChild(stateEl);
        const errorEl = document.createElement('div');
        errorEl.className = 'activity-card-error';
        card.appendChild(errorEl);
        const retryWrap = document.createElement('div');
        retryWrap.style.marginTop = '0.3em';
        const retryBtn = document.createElement('button');
        retryBtn.setAttribute('data-action', 'retry');
        retryBtn.className = 'import-retry-btn';
        retryBtn.textContent = 'retry';
        retryWrap.appendChild(retryBtn);
        card.appendChild(retryWrap);
        const progressWrap = document.createElement('div');
        progressWrap.className = 'activity-card-progress';
        const progressBar = document.createElement('div');
        progressBar.className = 'activity-card-progress-bar';
        progressWrap.appendChild(progressBar);
        card.appendChild(progressWrap);
        const elapsedEl = document.createElement('div');
        elapsedEl.className = 'activity-card-elapsed';
        card.appendChild(elapsedEl);
        const providerEl = document.createElement('div');
        providerEl.className = 'activity-card-provider';
        card.appendChild(providerEl);
        container.appendChild(card);
      }

      const cardClass = isError ? 'import-error' : 'import-active';
      card.className = 'activity-card ' + cardClass;
      const progress = imp.stage === 'initialization' ? 25 :
                      imp.stage === 'transcribing' ? 50 :
                      imp.stage === 'segmenting' ? 75 : 90;
      const elapsed = imp.elapsed_ms ? formatDuration(imp.elapsed_ms) : '0s';
      const humanStage = imp.stage === 'initialization' ? 'starting…' :
                         imp.stage === 'transcribing' ? 'transcribing audio…' :
                         imp.stage === 'segmenting' ? 'organizing segments…' : 'processing…';

      if (isError && imp.error) {
        card.children[3].textContent = truncate(imp.error, 40);
        card.children[3].style.display = '';
        card.children[4].style.display = '';
        card.children[4].querySelector('button').setAttribute('data-import-id', imp.import_id);
      } else {
        card.children[3].textContent = '';
        card.children[3].style.display = 'none';
        card.children[4].style.display = 'none';
      }

      card.children[0].textContent = '...' + getAgentId(imp.import_id);
      card.children[1].textContent = truncate(imp.input_file || 'import', 20);
      card.children[2].textContent = (isError ? '! ' : '') + humanStage;
      card.children[2].setAttribute('data-internal-stage', imp.stage || 'processing');
      card.children[5].children[0].style.width = progress + '%';
      card.children[6].textContent = elapsed;
      card.children[7].textContent = imp.file_type || 'unknown';
    }
    updateAllQuiet();
    updateStatusSummary();
  }

  function handleThinkEvent(msg) {
    if (msg.event === 'started') {
      state.thinkActive = true;
      state.think = { mode: msg.mode, day: msg.day };
      updateThinkCard();
    } else if (msg.event === 'status') {
      state.thinkActive = true;
      state.think = { ...state.think, ...msg };
      updateThinkCard();
    } else if (msg.event === 'completed') {
      state.thinkActive = false;
      state.think = null;
      updateThinkCard();
    }
  }

  function updateThinkCard() {
    if (!state.thinkActive || !state.think) {
      elements.thinkCard.classList.add('hidden');
      updateAllQuiet();
      updateStatusSummary();
      return;
    }

    elements.thinkCard.classList.remove('hidden');
    const d = state.think;

    // Info fields
    renderInfoItems(elements.thinkInfo, [
      { label: 'mode', value: d.mode || null },
      { label: 'day', value: d.day || null },
      { label: 'facet', value: d.facet || null },
      { label: 'segment', value: d.segment || null },
    ]);

    // Progress bars
    const progressItems = [];
    if (d.agents_total > 0) {
      progressItems.push({ label: 'Talents: ' + (d.agents_completed || 0) + ' / ' + d.agents_total, pct: Math.round((d.agents_completed || 0) / d.agents_total * 100) });
    }
    if (d.segments_total > 0) {
      progressItems.push({ label: 'Segments: ' + (d.segments_completed || 0) + ' / ' + d.segments_total, pct: Math.round((d.segments_completed || 0) / d.segments_total * 100) });
    }
    const progContainer = elements.thinkProgress;
    while (progContainer.children.length > progressItems.length) {
      progContainer.removeChild(progContainer.lastChild);
    }
    progressItems.forEach((item, i) => {
      let wrap = progContainer.children[i];
      if (!wrap) {
        wrap = document.createElement('div');
        wrap.className = 'think-progress';
        const label = document.createElement('div');
        label.className = 'think-progress-label';
        wrap.appendChild(label);
        const bar = document.createElement('div');
        bar.className = 'think-progress-bar';
        const fill = document.createElement('div');
        fill.className = 'think-progress-fill';
        bar.appendChild(fill);
        wrap.appendChild(bar);
        progContainer.appendChild(wrap);
      }
      wrap.children[0].textContent = item.label;
      wrap.children[1].children[0].style.width = item.pct + '%';
    });

    // Current agents
    if (d.current_agents && d.current_agents.length > 0) {
      elements.thinkAgents.textContent = 'running: ' + d.current_agents.join(', ');
    } else {
      elements.thinkAgents.textContent = '';
    }

    updateAllQuiet();
    updateStatusSummary();
  }

  const LOG_BUFFER_SIZE = 50;

  // mirror of solstone/apps/health/log_classifier.py — keep in sync
  function classifyLogLevel(stream, line) {
    const normalizedStream = (stream || '').trim().toLowerCase();
    const text = line || '';

    if (normalizedStream === 'stderr' && /\b(?:ERROR|CRITICAL|FATAL)\b/.test(text)) return 'error';
    if (/^Traceback \(most recent call last\):/.test(text)) return 'error';
    if (/: error while loading shared libraries:/.test(text)) return 'error';
    if (/\bsolstone isn't running\b/.test(text)) return 'error';
    if (/\b(?:ERROR|CRITICAL|FATAL)\b/.test(text)) return 'error';

    if (/\b(?:WARNING|WARN)\b/.test(text)) return 'warning';
    if (/\bUserWarning\b/.test(text)) return 'warning';
    if (/^\S+\s+W\s+/.test(text)) return 'warning';
    if (/\b(?:not reachable|Connection refused)\b/.test(text)) return 'warning';

    if (/\bDEBUG\b/.test(text)) return 'debug';

    if (/\bINFO\b/.test(text)) return 'info';
    if (/^\S+\s+I\s+/.test(text)) return 'info';

    return 'info';
  }

  function formatTemplate(template, values) {
    return template.replace(/\{(service|n|errors)\}/g, (_, key) => String(values[key]));
  }

  function stripLogPrefix(line) {
    return line.replace(/^\S+\s+\[[^\]]+\]\s+/, '');
  }

  function logLevelMatchesFilter(level) {
    if (state.logLevelFilter === 'error') return level === 'error';
    if (state.logLevelFilter === 'warning') return level === 'error' || level === 'warning';
    if (state.logLevelFilter === 'info') return level === 'error' || level === 'warning' || level === 'info';
    return true;
  }

  function formatLogTime(ts) {
    const date = new Date(ts);
    if (Number.isNaN(date.getTime())) return '';
    return [date.getHours(), date.getMinutes(), date.getSeconds()]
      .map(part => String(part).padStart(2, '0'))
      .join(':');
  }

  function filteredLogRecords(records) {
    const streamFilter = elements.logStreamFilter.value;
    return records.filter((record) => {
      const streamMatch = streamFilter === 'all' || streamFilter === record.stream;
      if (!streamMatch) return false;
      return logLevelMatchesFilter(classifyLogLevel(record.stream, record.line));
    });
  }

  function countErrorRecords(records) {
    return records.filter(record => classifyLogLevel(record.stream, record.line) === 'error').length;
  }

  function toggleLogServiceCollapse(service) {
    const collapsed = state.logCollapsedServices.get(service) === true;
    state.logCollapsedServices.set(service, !collapsed);
    renderLogs();
  }

  function makeLogServiceHeader(service, records) {
    const collapsed = state.logCollapsedServices.get(service) === true;
    const header = document.createElement('div');
    header.className = 'logs-service-header';
    header.setAttribute('data-svc', service);
    header.setAttribute('role', 'button');
    header.setAttribute('tabindex', '0');
    header.setAttribute('aria-expanded', String(!collapsed));
    if (collapsed) {
      header.textContent = formatTemplate(HEALTH_LOGS_COPY.LOGS_SERVICE_COLLAPSED, {
        service: serviceName(service),
        n: records.length,
        errors: countErrorRecords(records),
      });
    } else {
      header.textContent = '── ' + serviceName(service) + ' ──';
    }
    header.addEventListener('click', () => toggleLogServiceCollapse(service));
    header.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        toggleLogServiceCollapse(service);
      }
    });
    return header;
  }

  function appendLogLine(parent, service, record) {
    const level = classifyLogLevel(record.stream, record.line);
    if (!logLevelMatchesFilter(level)) return false;

    const line = document.createElement('div');
    line.className = 'logs-line logs-level-' + level;
    if (state.logCollapsedServices.get(service) === true) {
      line.classList.add('logs-svc-collapsed');
    }
    if (record.ts) {
      line.dataset.ts = String(record.ts);
      line.dataset.hhmmss = formatLogTime(record.ts);
    }
    line.appendChild(document.createTextNode(record.line));
    parent.appendChild(line);
    return true;
  }

  function applyPendingLogAnchor() {
    const anchor = state.pendingLogAnchor;
    if (!anchor) return;
    state.pendingLogAnchor = null;
    const lines = Array.from(elements.logsViewport.querySelectorAll('.logs-line[data-ts]'));
    if (lines.length === 0) return;
    let best = lines[0];
    let bestDelta = Math.abs(Number(best.dataset.ts) - anchor);
    for (const line of lines.slice(1)) {
      const delta = Math.abs(Number(line.dataset.ts) - anchor);
      if (delta < bestDelta) {
        best = line;
        bestDelta = delta;
      }
    }
    best.classList.add('log-row-anchored');
    best.scrollIntoView({ behavior: 'smooth', block: 'center' });
    setTimeout(() => best.classList.remove('log-row-anchored'), 1600);
  }

  function handleLogsEvent(msg) {
    if (msg.event !== 'line') return;

    const name = msg.name || 'unknown';
    const record = { ts: msg.ts || Date.now(), stream: msg.stream || 'stdout', line: msg.line || '' };
    state.logTotalCount++;
    state.lastLogTs = record.ts;

    // Check if this is a new service before adding to buffer
    const isNew = !state.serviceLogs.has(name);

    // Buffer per service
    if (isNew) {
      state.serviceLogs.set(name, []);
    }
    const buf = state.serviceLogs.get(name);
    buf.push(record);
    if (buf.length > LOG_BUFFER_SIZE) buf.shift();

    if (classifyLogLevel(record.stream, record.line) === 'error') {
      window.AppServices?.quietNotifs?.add({
        source: name,
        message: stripLogPrefix(record.line),
        ts: record.ts
      });
      elements.logsAnnouncer.textContent = stripLogPrefix(record.line);
      state.logErrorCount++;
      elements.logErrorBadge.textContent = state.logErrorCount + ' error' + (state.logErrorCount > 1 ? 's' : '');
      elements.logErrorBadge.classList.remove('hidden');
      if (state.logsCollapsed) {
        state.logsCollapsed = false;
        document.querySelector('.logs-card').classList.remove('logs-collapsed');
        elements.logsCollapseIndicator.textContent = '▼ hide';
        document.querySelector('.logs-header').setAttribute('aria-expanded', 'true');
        renderLogs();
        state.lastLogFilter = null;
      }
    }

    // Update service filter dropdown if new service
    if (isNew) {
      const opt = document.createElement('option');
      opt.value = name;
      opt.textContent = serviceName(name);
      elements.logServiceFilter.appendChild(opt);
    }

    renderLogs(name, record);
    updateLogsBadge();
  }

  function renderLogs(newService, newRecord) {
    if (state.deepLinkMode) return;
    const serviceFilter = elements.logServiceFilter.value;
    const viewport = elements.logsViewport;
    const filterKey = serviceFilter + ':' + elements.logStreamFilter.value + ':' + state.logLevelFilter;
    const atBottom = isAtBottom(viewport);

    // Incremental append: when following, filters unchanged, and we have a new record
    if (newRecord && state.lastLogFilter === filterKey && viewport.children.length > 0) {
      const serviceMatch = serviceFilter === 'all' || serviceFilter === newService;
      const streamMatch = elements.logStreamFilter.value === 'all' || elements.logStreamFilter.value === newRecord.stream;
      const levelMatch = logLevelMatchesFilter(classifyLogLevel(newRecord.stream, newRecord.line));
      if (serviceMatch && streamMatch && levelMatch) {
        if (state.logCollapsedServices.get(newService) === true) {
          renderLogs();
          return;
        }
        if (serviceFilter === 'all' && !viewport.querySelector(`[data-svc="${CSS.escape(newService)}"]`)) {
          viewport.appendChild(makeLogServiceHeader(newService, filteredLogRecords(state.serviceLogs.get(newService) || [])));
        }
        appendLogLine(viewport, newService, newRecord);
        if (state.logFollow && atBottom) {
          scrollLogsToBottom(viewport);
        }
        return;
      }
    }

    state.lastLogFilter = filterKey;

    // Check if user has scrolled away from bottom before updating
    const wasAtBottom = isAtBottom(viewport);

    const fragment = document.createDocumentFragment();
    const services = serviceFilter === 'all'
      ? Array.from(state.serviceLogs.keys()).sort()
      : (state.serviceLogs.has(serviceFilter) ? [serviceFilter] : []);

    for (const svc of services) {
      const lines = state.serviceLogs.get(svc) || [];
      const filtered = filteredLogRecords(lines);
      if (filtered.length === 0) continue;

      fragment.appendChild(makeLogServiceHeader(svc, filtered));

      for (const rec of filtered) {
        appendLogLine(fragment, svc, rec);
      }
    }

    viewport.textContent = '';
    viewport.appendChild(fragment);

    // Auto-scroll if following
    if (state.logFollow && wasAtBottom) {
      scrollLogsToBottom(viewport);
    }
    applyPendingLogAnchor();
  }

  // Event handlers by tract
  function handleSupervisorEvent(msg) {
    if (msg.event === 'status-error') {
      invalidateRealtime();
      return;
    }
    if (msg.event === 'status') {
      state.supervisorSeen = true;
      state.supervisorStatusAt = Date.now();
      if (!state.connected) {
        state.connected = true;
        connectError = false;
      }

      // Update running services
      if (msg.services) {
        state.services.clear();
        msg.services.forEach(svc => {
          state.services.set(svc.name, svc);
        });
      }

      // Update crashed services (separate array)
      if (msg.crashed) {
        state.crashed.clear();
        msg.crashed.forEach(svc => {
          state.crashed.set(svc.name, svc);
        });
      } else {
        state.crashed.clear();
      }

      // Update tasks
      state.tasks = msg.tasks || [];

      // Update health
      state.health = {
        stale_heartbeats: msg.stale_heartbeats || []
      };

      // Update queues
      state.queues = msg.queues || {};
      state.schedules = msg.schedules || [];

      updateVitals();
    }
  }

  function handleCortexEvent(msg) {
    // Handle status event first (no use_id at top level)
    if (msg.event === 'status') {
      // Update agent count for vitals
      state.cortexSeen = true;
      state.cortexStatusAt = Date.now();
      state.agentCount = msg.running_uses || 0;

      // Status event contains array of uses
      if (msg.uses) {
        // Clear uses not in status (they finished)
        const activeIds = new Set(msg.uses.map(a => a.use_id));
        state.agents.forEach((_, id) => {
          if (!activeIds.has(id)) {
            state.agents.delete(id);
          }
        });

        // Update/add uses from status
        msg.uses.forEach(agent => {
          const existing = state.agents.get(agent.use_id) || {};
          state.agents.set(agent.use_id, {
            ...existing,
            use_id: agent.use_id,
            name: agent.name,
            provider: agent.provider,
            elapsed_seconds: agent.elapsed_seconds,
            event: existing.event || 'thinking'
          });
        });
      }

      updateVitals();
      updateCortexGrid();
      return;
    }

    // Individual agent events require use_id
    const agentId = msg.use_id;
    if (!agentId) return;

    // Track start time for client-side elapsed updates
    const existing = state.agents.get(agentId) || {};
    const startTs = msg.event === 'start' ? msg.ts : existing.startTs;

    state.agents.set(agentId, {
      ...existing,
      use_id: agentId,
      name: msg.name || existing.name,
      provider: msg.provider || existing.provider,
      model: msg.model || existing.model,
      event: msg.event,
      ts: msg.ts,
      startTs: startTs
    });

    // Start elapsed timer when first agent appears
    if (state.agents.size > 0) {
      startElapsedTimer();
    }

    // Remove finished/error agents after delay
    if (msg.event === 'finish' || msg.event === 'error') {
      state.lastAgentFinishTs = msg.ts || Date.now();
      if (msg.event === 'error') {
        appendRecentError({
          type: 'agent',
          id: agentId,
          name: msg.name || existing.name || 'unknown',
          error: msg.error || window.CONVEY_COPY.UNKNOWN_ERROR,
          summary: msg.summary || msg.message || null,
          key: msg.key || msg.semantic_key || null,
          reason_code: msg.reason_code || null,
          provider: msg.provider || existing.provider || null,
          model: msg.model || existing.model || null,
          service: 'cortex',
          ts: Date.now()
        });
      }
      setTimeout(() => {
        state.agents.delete(agentId);
        updateCortexGrid();
      }, 5000);
    }

    updateCortexGrid();
  }

  function handleObserveEvent(msg) {
    if (!msg.stream) return;
    const existing = state.clients.get(msg.stream) || {};
    state.clients.set(msg.stream, { ...existing, ...msg, lastSeen: Date.now() });
    updateObserve();
    updateClients();
  }

  function handleImporterEvent(msg) {
    const importId = msg.import_id;
    if (!importId) return;

    if (msg.event === 'started') {
      state.imports.set(importId, {
        import_id: importId,
        input_file: msg.input_file,
        file_type: msg.file_type,
        stage: msg.stage,
        event: 'started',
        elapsed_ms: 0,
        lastSeen: Date.now()
      });
    } else if (msg.event === 'status') {
      const existing = state.imports.get(importId) || {};
      state.imports.set(importId, {
        ...existing,
        stage: msg.stage,
        elapsed_ms: msg.elapsed_ms,
        event: 'status',
        lastSeen: Date.now()
      });
    } else if (msg.event === 'completed') {
      const existing = state.imports.get(importId) || {};
      state.imports.set(importId, {
        ...existing,
        event: msg.event
      });

      // Remove after delay
      setTimeout(() => {
        state.imports.delete(importId);
        updateImporterGrid();
      }, 5000);
    } else if (msg.event === 'error') {
      const existing = state.imports.get(importId) || {};
      state.imports.set(importId, {
        ...existing,
        event: 'error',
        error: msg.error || window.CONVEY_COPY.UNKNOWN_ERROR,
        lastSeen: Date.now()
      });

      appendRecentError({
        type: 'import',
        id: importId,
        name: msg.input_file || existing.input_file || 'unknown',
        error: msg.error || window.CONVEY_COPY.UNKNOWN_ERROR,
        summary: msg.summary || msg.message || null,
        key: msg.key || msg.semantic_key || null,
        reason_code: msg.reason_code || null,
        provider: msg.provider || null,
        model: msg.model || null,
        service: 'importer',
        stage: existing.stage || msg.stage || 'unknown',
        ts: Date.now()
      });

      // Keep error visible longer
      setTimeout(() => {
        state.imports.delete(importId);
        updateImporterGrid();
      }, 15000);
    }

    updateImporterGrid();
  }

  // Main event handler
  function handleEvent(msg) {
    const eventTs = Date.now();
    state.lastEventTs = eventTs;
    const tract = msg.tract;
    if (tract === 'supervisor') handleSupervisorEvent(msg);
    else if (tract === 'cortex') handleCortexEvent(msg);
    else if (tract === 'observe') handleObserveEvent(msg);
    else if (tract === 'importer') handleImporterEvent(msg);
    else if (tract === 'think') handleThinkEvent(msg);
    else if (tract === 'logs') handleLogsEvent(msg);
    updateConnectionHealth();
    updateStatusSummary();
  }

  // Log controls
  elements.logServiceFilter.addEventListener('change', () => {
    renderLogs();
  });
  elements.logLevelFilter.addEventListener('change', () => {
    state.logLevelFilter = elements.logLevelFilter.value;
    renderLogs();
  });
  elements.logStreamFilter.addEventListener('change', () => {
    renderLogs();
  });
  elements.logsViewport.addEventListener('scroll', () => {
    if (programmaticScroll) return;
    if (!isAtBottom(elements.logsViewport)) {
      state.logFollow = false;
      elements.logFollowBtn.classList.remove('active');
    }
  });
  function toggleLogsCollapse() {
    state.logsCollapsed = !state.logsCollapsed;
    const card = document.querySelector('.logs-card');
    const header = document.querySelector('.logs-header');
    card.classList.toggle('logs-collapsed', state.logsCollapsed);
    header.setAttribute('aria-expanded', String(!state.logsCollapsed));
    elements.logsCollapseIndicator.textContent = state.logsCollapsed ? '▶ show' : '▼ hide';
    if (!state.logsCollapsed) renderLogs();
  }
  document.querySelector('.logs-header').addEventListener('click', (e) => {
    if (state.deepLinkMode || e.target.closest('.logs-controls')) return;
    toggleLogsCollapse();
  });
  document.querySelector('.logs-header').addEventListener('keydown', (e) => {
    if (state.deepLinkMode || e.target.closest('.logs-controls')) return;
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      toggleLogsCollapse();
    }
  });
  elements.logFollowBtn.addEventListener('click', () => {
    state.logFollow = !state.logFollow;
    elements.logFollowBtn.classList.toggle('active', state.logFollow);
    if (state.logFollow) {
      scrollLogsToBottom(elements.logsViewport);
    }
  });
  elements.logClearBtn.addEventListener('click', () => {
    state.serviceLogs.clear();
    state.logTotalCount = 0;
    state.logErrorCount = 0;
    state.lastLogTs = null;
    state.lastLogFilter = null;
    state.logCollapsedServices.clear();
    elements.logErrorBadge.textContent = '';
    elements.logErrorBadge.classList.add('hidden');
    elements.logServiceFilter.innerHTML = '<option value="all">all services</option>';
    updateLogsBadge();
    renderLogs();
  });
  elements.vitalsCheckBtn.addEventListener('click', () => {
    elements.vitalsCheckBtn.textContent = 'checking…';
    elements.vitalsCheckBtn.disabled = true;
    fetch('/app/health/api/info')
      .then(r => r.json())
      .then(info => {
        state.localHost = info.hostname;
        brainSnapshot = info.brain || brainSnapshot;
        updateObserve();
        renderBrainHealth();
        updateStatusSummary();
        elements.vitalsCheckBtn.textContent = 'check now';
        elements.vitalsCheckBtn.disabled = false;
      })
      .catch(() => {
        elements.vitalsCheckBtn.textContent = 'check now';
        elements.vitalsCheckBtn.disabled = false;
      });
  });
  elements.logExportBtn.addEventListener('click', () => {
    const content = elements.logsViewport.textContent;
    const blob = new Blob([content], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'solstone-logs-' + new Date().toISOString().slice(0, 19).replace(/:/g, '') + '.txt';
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  });

  // Deep-link: display log file content if ?log= param is present
  const deepLinkLog = new URLSearchParams(window.location.search).get('log');
  if (deepLinkLog) {
    state.logsCollapsed = false;
    document.querySelector('.logs-card').classList.remove('logs-collapsed');
    elements.logsCollapseIndicator.textContent = '▼ hide';
    document.querySelector('.logs-header').setAttribute('aria-expanded', 'true');
    const viewport = elements.logsViewport;
    const logsCard = viewport.closest('.logs-card');
    const logsTitle = logsCard.querySelector('.logs-title');
    const logsControls = logsCard.querySelector('.logs-controls');

    // Hide dashboard cards and suppress live log rendering
    const dashboard = document.querySelector('.health-dashboard');
    dashboard.querySelectorAll('.vitals-bar, .observe-card, .clients-card, .registered-clients-card, .activity-grids, .think-card').forEach(el => el.style.display = 'none');
    state.deepLinkMode = true;
    elements.logsSummaryBadge.style.display = 'none';

    // Replace header with log file context
    logsTitle.textContent = 'log file';
    logsControls.innerHTML = '<button id="logBackBtn">← back to dashboard</button>';
    function parseDeepLinkLogLine(rawLine) {
      const prefixed = rawLine.match(/^\S+\s+\[([^\]]+)\]\s?(.*)$/) || rawLine.match(/^\[([^\]]+)\]\s?(.*)$/);
      if (!prefixed) return { stream: 'log', classifierLine: rawLine };
      const label = prefixed[1];
      const splitAt = label.lastIndexOf(':');
      if (splitAt === -1) return { stream: 'log', classifierLine: rawLine };
      return { stream: label.slice(splitAt + 1), classifierLine: prefixed[2] || '' };
    }

    function appendDeepLinkLogLine(parent, rawLine, streamOverride, classifierOverride) {
      const parsed = parseDeepLinkLogLine(rawLine);
      const stream = streamOverride || parsed.stream;
      const classifierLine = classifierOverride || (streamOverride ? rawLine : parsed.classifierLine);
      const level = classifyLogLevel(stream, classifierLine);
      const line = document.createElement('div');
      line.className = 'logs-line logs-level-' + level;
      line.textContent = rawLine;
      parent.appendChild(line);
    }

    viewport.textContent = '';
    appendDeepLinkLogLine(viewport, 'loading...', 'stdout');

    fetch('/app/health/api/log?path=' + encodeURIComponent(deepLinkLog))
      .then(r => r.json().then(data => ({ok: r.ok, data})))
      .then(({ok, data}) => {
        viewport.textContent = '';
        if (!ok) {
          const message = data.error || window.CONVEY_COPY.LOG_READ_FAILED;
          appendDeepLinkLogLine(viewport, message, 'stderr', 'ERROR: ' + message);
          return;
        }
        const pathHeader = document.createElement('div');
        pathHeader.className = 'logs-service-header';
        pathHeader.textContent = '── ' + data.path + ' ──';
        viewport.appendChild(pathHeader);
        data.content.split('\n').forEach((rawLine) => {
          appendDeepLinkLogLine(viewport, rawLine);
        });
      })
      .catch(() => {
        viewport.textContent = '';
        appendDeepLinkLogLine(viewport, 'network error loading log file', 'stderr', 'ERROR: network error loading log file');
      });

    document.addEventListener('click', function(e) {
      if (e.target && e.target.id === 'logBackBtn') {
        window.location.href = '/app/health';
      }
    });

	    // Scroll logs card into view
	    logsCard.scrollIntoView({behavior: 'smooth'});
	  }

	  function runRecentErrorsFocus(day, talent) {
	    state.recentErrorsFilter = { day, talent };
	    state.pendingRecentErrorsFocus = true;
	    updateErrorSummary();
	  }
	  function focusRecentErrors() {
	    const hashParams = new URLSearchParams(window.location.hash.replace(/^#/, ''));
	    if (deepLinkLog || hashParams.get('focus') !== 'recent-errors') return;
	    const day = hashParams.get('day') || 'today';
	    const talent = hashParams.get('talent') || '';
	    runRecentErrorsFocus(day, talent);
	  }
		  function seedAgentErrors(seed) {
		    const entries = Array.isArray(seed) ? seed : [];
		    entries.forEach(entry => appendRecentError(entry));
		    if (entries.length === 0) updateErrorSummary();
		    updateStatusSummary();
		  }

			  // Listen to all Callosum events
  const registeredClientsReady = loadRegisteredClients();
	  let healthInitialized = false;

  function invalidateRealtime() {
    state.connected = false;
    state.supervisorSeen = false;
    state.cortexSeen = false;
    state.supervisorStatusAt = null;
    state.cortexStatusAt = null;
    state.lastEventTs = null;
    state.services.clear();
    state.crashed.clear();
    state.tasks = [];
    state.health = null;
    state.queues = {};
    state.schedules = [];
    state.agents.clear();
    state.imports.clear();
    state.clients.clear();
    state.think = null;
    state.thinkActive = false;
    updateVitals();
    updateCortexGrid();
    updateImporterGrid();
    updateThinkCard();
    updateObserve();
    updateConnectionHealth();
  }

	  function initHealthRealtime() {
	    wireBacklogReprocessActions();
	    if (window.appEvents) {
	      window.appEvents.listen('*', handleEvent);
          window.appEvents.onConnectionState?.(({ connected }) => {
            if (!connected) invalidateRealtime();
          });
    }

	    armSkeletonTimeout();
	  }

	  function initHealth() {
	    if (healthInitialized) return;
	    healthInitialized = true;
	    applyHealthCopy();
    Promise.all([healthInfoReady, registeredClientsReady, loadHealthState()]).then(() => {
      document.dispatchEvent(new CustomEvent('health:initial-ready'));
    });
	    focusRecentErrors();
	    window.addEventListener('hashchange', focusRecentErrors);
	    document.getElementById('glanceErrors')?.addEventListener('click', (e) => {
	      e.preventDefault();
	      runRecentErrorsFocus('today', '');
	    });
	    initHealthRealtime();
	  }

	  document.addEventListener('workspace:mounted', (event) => {
	    if (!event.detail || event.detail.appName === 'health') {
	      initHealth();
	    }
	  });
	  if (document.readyState === 'complete') {
	    initHealth();
	  }

  function sweepStale() {
    const cutoff = Date.now() - 5 * 60 * 1000;
    let changed = false;
    state.agents.forEach((agent, id) => {
      if ((agent.ts || 0) < cutoff) {
        state.agents.delete(id);
        changed = true;
      }
    });
    if (changed) updateCortexGrid();

    changed = false;
    state.imports.forEach((imp, id) => {
      if ((imp.lastSeen || 0) < cutoff) {
        state.imports.delete(id);
        changed = true;
      }
    });
    if (changed) updateImporterGrid();
  }

  function updateConnectionHealth() {
    // A busy event bus cannot keep an old supervisor or cortex snapshot fresh.
    const now = Date.now();
    if (state.supervisorStatusAt && now - state.supervisorStatusAt >= 30000) {
      invalidateRealtime();
      return;
    }
    if (state.cortexStatusAt && now - state.cortexStatusAt >= 30000) {
      state.cortexStatusAt = null;
      state.cortexSeen = false;
      state.agents.clear();
      updateVitals();
      updateCortexGrid();
    }
    const el = elements.connectionIndicator;
    if (!state.lastEventTs) {
      el.textContent = 'waiting for live updates';
      el.className = 'connection-indicator';
      elements.logsConnectionNote.textContent = 'log updates may be delayed';
      elements.logsConnectionNote.classList.remove('hidden');
      return;
    }
    const ageMs = Date.now() - state.lastEventTs;
    const ago = Math.floor(ageMs / 1000);
    if (ago >= 60) {
      el.textContent = `no updates in ${relativeTime(ageMs)}`;
      el.className = 'connection-indicator disconnected';
      elements.logsConnectionNote.textContent = 'log updates may be delayed';
      elements.logsConnectionNote.classList.remove('hidden');
    } else if (ago >= 30) {
      el.textContent = 'updates are slow';
      el.className = 'connection-indicator stale';
      elements.logsConnectionNote.textContent = 'log updates may be delayed';
      elements.logsConnectionNote.classList.remove('hidden');
    } else {
      el.textContent = `updated ${ageAgo(ageMs)}`;
      el.className = 'connection-indicator';
      elements.logsConnectionNote.textContent = '';
      elements.logsConnectionNote.classList.add('hidden');
    }
  }

  // Sweep stale agents and imports every 60s
  let staleSweepTimer = setInterval(sweepStale, 60000);

  let registeredClientsTimer = setInterval(loadRegisteredClients, 60000);

  // Connection health indicator — updated every 5s
  let connectionHealthTimer = setInterval(updateConnectionHealth, 5000);

  // Pause intervals when tab is hidden
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      clearInterval(staleSweepTimer);
      staleSweepTimer = null;
      clearInterval(connectionHealthTimer);
      connectionHealthTimer = null;
      clearInterval(registeredClientsTimer);
      registeredClientsTimer = null;
      if (elapsedTimer) {
        clearInterval(elapsedTimer);
        elapsedTimer = null;
      }
    } else {
      sweepStale();
      updateConnectionHealth();
      if (state.agents.size > 0) {
        updateElapsed();
        startElapsedTimer();
      }
      staleSweepTimer = setInterval(sweepStale, 60000);
      loadRegisteredClients();
      registeredClientsTimer = setInterval(loadRegisteredClients, 60000);
      connectionHealthTimer = setInterval(updateConnectionHealth, 5000);
    }
  });
})();
