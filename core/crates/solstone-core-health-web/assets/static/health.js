// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function(){
  const reportHealthReady = window.createViewReady('health');

  const HEALTH_LOGS_COPY = {
    "LOGS_LEVEL_FILTER_LABEL": window.JournalI18n.render('journal.window.health.logs.labels.logs.level.filter.level'),
    "LOGS_LEVEL_OPTION_ALL": window.JournalI18n.render('journal.window.health.logs.labels.logs.level.option.all.levels'),
    "LOGS_LEVEL_OPTION_ERROR": window.JournalI18n.render('journal.window.health.logs.labels.logs.level.option.errors.only'),
    "LOGS_LEVEL_OPTION_INFO": window.JournalI18n.render('journal.window.health.logs.labels.logs.level.option.info.above'),
    "LOGS_LEVEL_OPTION_WARNING": window.JournalI18n.render('journal.window.health.logs.labels.logs.level.option.warnings.errors'),
    "LOGS_SERVICE_FILTER_LABEL": window.JournalI18n.render('journal.window.health.logs.labels.logs.service.filter.service'),
    "LOGS_STREAM_FILTER_LABEL": window.JournalI18n.render('journal.window.health.logs.labels.logs.stream.filter.stream')
  };
  const HEALTH_GLANCE_COPY = {
    HEALTH_OBSERVE_LOCAL_SILENT: window.JournalI18n.render('journal.window.health.glance.observe.local.your.journal.s.computer.isn.t'),
    HEALTH_OBSERVE_UNKNOWN_SILENT: window.JournalI18n.render('journal.window.health.glance.observe.unknown.can.t.identify.your.journal.s'),
    HEALTH_DEVICES_EMPTY: window.JournalI18n.render('journal.window.health.glance.devices.empty.no.devices.are.paired.yet'),
    HEALTH_DEVICES_PAIR_LINK: window.JournalI18n.render('journal.window.health.glance.devices.pair.pair.a.device'),
    HEALTH_DEVICES_SEE_LINK: window.JournalI18n.render('journal.window.health.glance.devices.see.see.your.devices.in.network'),
    HEALTH_DEVICES_MANAGE_LINK: window.JournalI18n.render('journal.window.health.glance.devices.manage.manage.in.network'),
  };
  const HEALTH_GLANCE_MESSAGES = {
    HEALTH_GLANCE_CATCHING_UP: 'journal.window.health.glance.catching.up',
    HEALTH_GLANCE_CHECKING: 'journal.window.health.glance.glance.checking.checking.where.your.journal.stands',
    HEALTH_GLANCE_CLIENT_SILENT: 'journal.window.health.glance.glance.client.one.of.your.devices.hasn.t',
    HEALTH_GLANCE_DEVICE_FAILING: 'journal.window.health.glance.device.failing',
    HEALTH_GLANCE_DEVICES_FAILING: 'journal.window.health.glance.devices.failing',
    HEALTH_GLANCE_DEVICE_SILENT: 'journal.window.health.glance.device.silent',
    HEALTH_GLANCE_DEVICE_SILENT_NO_AGE: 'journal.window.health.glance.device.silent.no.age',
    HEALTH_GLANCE_DEVICES_SILENT: 'journal.window.health.glance.devices.silent',
    HEALTH_GLANCE_DEVICES_UNAVAILABLE: 'journal.window.health.glance.glance.devices.your.devices.delivery.status.is.unavailable',
    HEALTH_GLANCE_DEVICES_UNKNOWN: 'journal.window.health.glance.glance.devices.delivery.from.some.of.your.devices',
    HEALTH_GLANCE_WORK_HELD: 'journal.window.health.glance.glance.work.some.queued.work.is.on.hold',
    HEALTH_GLANCE_WORK_UNAVAILABLE: 'journal.window.health.glance.glance.work.queued.work.status.is.unavailable.right',
    HEALTH_GLANCE_OK: 'journal.window.health.glance.glance.ok.the.journal.s.live.services.and',
    HEALTH_GLANCE_CHECKIN_STALE: 'journal.window.health.glance.checkin.stale',
    HEALTH_GLANCE_CHECKINS_STALE: 'journal.window.health.glance.checkins.stale',
    HEALTH_GLANCE_SERVICE_ATTENTION: 'journal.window.health.glance.service.attention',
    HEALTH_GLANCE_SERVICES_ATTENTION: 'journal.window.health.glance.services.attention',
    HEALTH_GLANCE_SERVICES_UNREACHABLE: 'journal.window.health.glance.glance.services.the.journal.s.services.couldn.t',
    HEALTH_DEVICES_COUNT_MANY: 'journal.window.health.glance.devices.count.many',
  };
  // Worst signal wins, and green is earned: a verdict may not read "everything's
  // working" while the device rows below it say a device has gone quiet, and a
  // failure to derive device delivery renders unavailable, never green.
  const GLANCE_DEVICES_ACTION = { href: '#registeredClientsCard', label: window.JournalI18n.render('journal.window.health.label.view.devices') };

  let brainSnapshot = null;
  let backlogCopy = {};

  const healthInfoReady = fetch('/app/health/api/info')
    .then(r => r.json())
    .then(info => {
      state.localHost = info.hostname;
      state.localProgramsAvailable = info.local_tcp_programs === true;
      state.hostCapabilities = info.host_capabilities || null;
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
    loadedAt: Date.now(),
    queuedWorkAuthoritySeen: false,
    backupView: { tone: 'unavailable', text: window.JournalI18n.render('journal.window.health.text.checking.backup.status') },
    heldWorkSnapshot: null,
    heldWorkCurrent: false,
    supervisorSeen: false,
    supervisorStatusAt: null,
    cortexStatusAt: null,
    cortexSeen: false,
    registeredClients: null,
    registeredClientsFailed: false,
    registeredClientsObservedAt: null,
    services: new Map(),        // Running services
    connected: false,
    crashed: new Map(),         // Crashed services (separate from running)
    tasks: [],
    health: null,
    searchIndex: null,
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
    // G3-202: until /app/health/api/state answers, the error count is unknown.
    // Neither path below may paint a confident zero from an empty list.
    agentErrorsLoaded: false,
    recentErrorsFilter: null,
    pendingRecentErrorsFocus: false,
    pendingLogAnchor: null,
    localHost: null,
    hostCapabilities: null,
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
    logExportBtn: document.getElementById('logExportBtn'),
  };

  function renderCount(id, count, displayed = String(count)) {
    return window.JournalI18n.render(id, [
      ['count', displayed], ['quantity', window.JournalI18n.cardinalQuantity(count)]
    ]);
  }

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

  async function getJson(path, options = {}) {
    if (window.apiJson) {
      return window.apiJson(path, options);
    }
    const response = await fetch(path, { headers: { Accept: 'application/json' }, ...options });
    const payload = await response.json();
    if (!response.ok) throw payload;
    return payload;
  }

  const backupSection = document.getElementById('backup');
  let backupReadPending = false;
  async function loadBackupStatus() {
    const section = backupSection;
    if (!section?.isConnected || document.hidden || backupReadPending) return;
    backupReadPending = true;
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10000);
    let snapshot = null;
    try {
      const payload = await getJson('/app/backup/status', { signal: controller.signal });
      if (payload?.success === true) snapshot = payload;
    } catch (error) {
      logHealthError(error, 'backup status fetch failed');
    } finally {
      clearTimeout(timeout);
      backupReadPending = false;
    }
    // A read started on a removed workspace cannot repaint its replacement.
    if (!section.isConnected) return;
    state.backupView = window.BackupStatus.renderSummary(section.querySelector('[data-backup-summary]'), snapshot);
    window.BackupStatus.renderFacts(section.querySelector('[data-backup-facts]'), snapshot);
    updateStatusSummary();
  }

  function focusBackupDetails() {
    if (window.location.hash === '#backup' && backupSection?.isConnected) {
      backupSection.scrollIntoView({ block: 'start' });
    }
  }

  const SPEECH_SETUP_COPY = {
    waiting: {
      state: window.JournalI18n.render('journal.window.health.speech.setup.copy.state.waiting.for.setup'),
      explanation: window.JournalI18n.render('journal.window.health.speech.setup.copy.explanation.setup.hasn.t.started.yet'),
      summary: window.JournalI18n.render('journal.window.health.speech.setup.copy.summary.setup.details'),
    },
    downloading: {
      state: window.JournalI18n.render('journal.window.health.speech.setup.copy.state.downloading'),
      explanation: null,
      summary: window.JournalI18n.render('journal.window.health.speech.setup.copy.summary.setup.details'),
    },
    verifying: {
      state: window.JournalI18n.render('journal.window.health.speech.setup.copy.state.checking.the.download'),
      explanation: null,
      summary: window.JournalI18n.render('journal.window.health.speech.setup.copy.summary.setup.details'),
    },
    installing: {
      state: window.JournalI18n.render('journal.window.health.speech.setup.copy.state.installing'),
      explanation: null,
      summary: window.JournalI18n.render('journal.window.health.speech.setup.copy.summary.setup.details'),
    },
    paused: {
      state: window.JournalI18n.render('journal.window.health.speech.setup.copy.state.setup.paused'),
      explanation: window.JournalI18n.render('journal.window.health.speech.setup.copy.explanation.setup.was.cancelled'),
      summary: window.JournalI18n.render('journal.window.health.speech.setup.copy.summary.show.recovery.steps'),
    },
    cooling: {
      state: window.JournalI18n.render('journal.window.health.speech.setup.copy.state.setup.couldn.t.finish'),
      explanation: window.JournalI18n.render('journal.window.health.speech.setup.copy.explanation.automatic.setup.is.waiting.to.try'),
      summary: window.JournalI18n.render('journal.window.health.speech.setup.copy.summary.show.recovery.steps'),
    },
    attention: {
      state: window.JournalI18n.render('journal.window.health.speech.setup.copy.state.setup.needs.attention'),
      explanation: window.JournalI18n.render('journal.window.health.speech.setup.copy.explanation.local.speech.recognition.isn.t.ready'),
      summary: window.JournalI18n.render('journal.window.health.speech.setup.copy.summary.show.recovery.steps'),
    },
    unavailable: {
      state: window.JournalI18n.render('journal.window.health.speech.setup.copy.state.status.unavailable'),
      explanation: window.JournalI18n.render('journal.window.health.speech.setup.copy.explanation.speech.recognition.setup.couldn.t.be'),
      summary: window.JournalI18n.render('journal.window.health.speech.setup.copy.summary.setup.details'),
    },
  };

  function validSpeechSetup(payload) {
    if (!payload || typeof payload !== 'object' || Array.isArray(payload)
        || typeof payload.applicable !== 'boolean' || typeof payload.visible !== 'boolean') return false;
    if (!payload.visible) return payload.state == null && payload.tone == null;
    const nullableText = value => value == null || typeof value === 'string';
    const nullableBytes = value => value == null || (Number.isSafeInteger(value) && value >= 0);
    return payload.applicable && typeof payload.state === 'string'
      && Object.hasOwn(SPEECH_SETUP_COPY, payload.state)
      && ['neutral', 'calm', 'attention', 'unavailable'].includes(payload.tone)
      && nullableBytes(payload.bytes_received) && nullableBytes(payload.bytes_total)
      && nullableText(payload.cause) && nullableText(payload.command)
      && typeof payload.recovery_representable === 'boolean'
      && (payload.command == null || (payload.recovery_representable && payload.state !== 'unavailable'));
  }

  function formatSpeechBytes(bytes) {
    if (typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes < 0) return '';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let val = bytes;
    let idx = 0;
    while (val >= 1024 && idx < units.length - 1) {
      val /= 1024;
      idx += 1;
    }
    return idx === 0 ? `${val} B` : `${val.toFixed(1)} ${units[idx]}`;
  }

  function clearSpeechBytes() {
    const bytesEl = document.getElementById('speechSetupBytes');
    if (bytesEl) {
      bytesEl.hidden = true;
      bytesEl.textContent = '';
      bytesEl.removeAttribute('data-received');
      bytesEl.removeAttribute('data-total');
    }
  }

  function hideSpeechSetup() {
    const setupEl = document.getElementById('speechSetup');
    if (!setupEl) return;
    if (document.activeElement?.closest('#speechSetup') === setupEl) {
      const target = document.querySelector('#brain .card-title') || document.getElementById('brainHealthStatus');
      if (target) {
        target.setAttribute('tabindex', '-1');
        target.focus({ preventScroll: true });
      }
    }
    setupEl.hidden = true;
    setupEl.removeAttribute('data-state');
    setupEl.classList.remove(
      'speech-setup-tone-neutral',
      'speech-setup-tone-calm',
      'speech-setup-tone-attention',
      'speech-setup-tone-unavailable'
    );
    clearSpeechBytes();
  }

  function renderSpeechSetup(payload) {
    const brainCard = document.getElementById('brain');
    const setupEl = document.getElementById('speechSetup');
    if (!brainCard || !setupEl) return;

    const mediaBacklog = document.getElementById('mediaBacklogLine');
    if (mediaBacklog && mediaBacklog.parentNode === brainCard) {
      if (setupEl.nextSibling !== mediaBacklog) {
        brainCard.insertBefore(setupEl, mediaBacklog);
      }
    }

    setupEl.hidden = false;
    if (payload.state) {
      setupEl.setAttribute('data-state', payload.state);
    } else {
      setupEl.removeAttribute('data-state');
    }

    setupEl.classList.remove(
      'speech-setup-tone-neutral',
      'speech-setup-tone-calm',
      'speech-setup-tone-attention',
      'speech-setup-tone-unavailable'
    );
    if (payload.tone) {
      setupEl.classList.add(`speech-setup-tone-${payload.tone}`);
    }

    const copy = SPEECH_SETUP_COPY[payload.state] || {
      state: payload.state || '',
      explanation: null,
      summary: window.JournalI18n.render('journal.window.health.status.setup.details'),
    };

    const stateEl = document.getElementById('speechSetupState');
    if (stateEl && stateEl.textContent !== copy.state) {
      stateEl.textContent = copy.state;
    }

    const expEl = document.getElementById('speechSetupExplanation');
    if (expEl) {
      if (copy.explanation) {
        expEl.hidden = false;
        expEl.textContent = copy.explanation;
      } else {
        expEl.hidden = true;
        expEl.textContent = '';
      }
    }

    const bytesEl = document.getElementById('speechSetupBytes');
    if (bytesEl) {
      const hasReceived = typeof payload.bytes_received === 'number';
      const hasTotal = typeof payload.bytes_total === 'number';
      if (hasReceived || hasTotal) {
        bytesEl.hidden = false;
        if (hasReceived) bytesEl.setAttribute('data-received', String(payload.bytes_received));
        else bytesEl.removeAttribute('data-received');
        if (hasTotal) bytesEl.setAttribute('data-total', String(payload.bytes_total));
        else bytesEl.removeAttribute('data-total');

        if (hasReceived && hasTotal) {
          bytesEl.textContent = window.JournalI18n.render('journal.window.health.speech.setup.download.bytes', [['received', String(formatSpeechBytes(payload.bytes_received))], ['total', String(formatSpeechBytes(payload.bytes_total))]]);
        } else if (hasReceived) {
          bytesEl.textContent = formatSpeechBytes(payload.bytes_received);
        } else {
          bytesEl.textContent = formatSpeechBytes(payload.bytes_total);
        }
      } else {
        bytesEl.hidden = true;
        bytesEl.textContent = '';
        bytesEl.removeAttribute('data-received');
        bytesEl.removeAttribute('data-total');
      }
    }

    const summaryEl = document.getElementById('speechSetupSummary');
    if (summaryEl && summaryEl.textContent !== copy.summary) {
      summaryEl.textContent = copy.summary;
    }

    const causeEl = document.getElementById('speechSetupCause');
    const hasCause = typeof payload.cause === 'string' && payload.cause.trim().length > 0;
    if (causeEl) {
      if (hasCause) {
        causeEl.hidden = false;
        causeEl.textContent = payload.cause;
      } else {
        causeEl.hidden = true;
        causeEl.textContent = '';
      }
    }

    const instructionEl = document.getElementById('speechSetupInstruction');
    const commandWrapper = document.getElementById('speechSetupCommandWrapper');
    const commandEl = document.getElementById('speechSetupCommand');
    const hasCommand = payload.recovery_representable && payload.state !== 'unavailable'
      && typeof payload.command === 'string' && payload.command.trim().length > 0;
    if (hasCommand) {
      if (instructionEl) instructionEl.hidden = false;
      if (commandWrapper) commandWrapper.hidden = false;
      if (commandEl) {
        commandEl.hidden = false;
        commandEl.textContent = payload.command;
      }
    } else {
      if (instructionEl) instructionEl.hidden = true;
      if (commandWrapper) commandWrapper.hidden = true;
      if (commandEl) {
        commandEl.hidden = true;
        commandEl.textContent = '';
      }
    }
    const detailsEl = document.getElementById('speechSetupDetails');
    if (detailsEl) {
      if (!hasCause && !hasCommand && document.activeElement?.closest('#speechSetupDetails') === detailsEl && stateEl) {
        stateEl.setAttribute('tabindex', '-1');
        stateEl.focus({ preventScroll: true });
      }
      detailsEl.hidden = !hasCause && !hasCommand;
    }
  }

  let speechReadPending = false;
  let speechApplicableLatched = document.getElementById('speechSetup')?.dataset.applicable === 'true';
  let speechAttempt = 0;

  async function loadSpeechSetup() {
    if (state.hostCapabilities?.terminal_speech_setup === false) {
      speechApplicableLatched = false;
      hideSpeechSetup();
      return;
    }
    const card = document.getElementById('brain');
    if (!card?.isConnected || document.hidden || speechReadPending) return;
    speechReadPending = true;
    const attempt = ++speechAttempt;
    let settled = false;
    const controller = new AbortController();
    const timeout = setTimeout(() => {
      if (settled || attempt !== speechAttempt) return;
      settled = true;
      controller.abort();
      clearSpeechBytes();
      speechReadPending = false;
      if (!card.isConnected) return;
      if (speechApplicableLatched) {
        renderSpeechSetup({
          applicable: true,
          visible: true,
          state: 'unavailable',
          tone: 'unavailable',
          bytes_received: null,
          bytes_total: null,
          cause: null,
          recovery_representable: false,
          command: null,
        });
      }
    }, 10000);

    try {
      const payload = await getJson('/app/health/api/speech-setup', { signal: controller.signal });
      if (settled || attempt !== speechAttempt) return;
      if (!card.isConnected) return;

      if (!validSpeechSetup(payload)) {
        throw new Error('invalid speech-setup payload');
      }

      settled = true;
      clearTimeout(timeout);
      speechReadPending = false;

      if (payload.applicable === false) {
        speechApplicableLatched = false;
        hideSpeechSetup();
        return;
      }

      speechApplicableLatched = true;

      if (payload.visible === false) {
        hideSpeechSetup();
        return;
      }

      renderSpeechSetup(payload);
    } catch (error) {
      if (settled || attempt !== speechAttempt) return;
      settled = true;
      clearTimeout(timeout);
      clearSpeechBytes();
      speechReadPending = false;
      if (!card.isConnected) return;
      logHealthError(error, 'speech setup fetch failed');
      if (speechApplicableLatched) {
        renderSpeechSetup({
          applicable: true,
          visible: true,
          state: 'unavailable',
          tone: 'unavailable',
          bytes_received: null,
          bytes_total: null,
          cause: null,
          recovery_representable: false,
          command: null,
        });
      } else {
        hideSpeechSetup();
      }
    } finally {
      clearTimeout(timeout);
      if (attempt === speechAttempt && !settled) {
        speechReadPending = false;
      }
    }
  }

  async function loadLocalPrograms() {
    const section = document.getElementById('localPrograms');
    if (!section) return;
    await healthInfoReady;
    if (!state.localProgramsAvailable) { section.hidden = true; return; }
    try {
      const data = await window.apiJson('/app/health/api/local-programs');
      const host = section.querySelector('[data-local-programs]');
      host.replaceChildren();
      for (const item of data.items) {
        const row = document.createElement('p');
        row.textContent = window.JournalI18n.render('journal.window.health.local.program.requests', [['program', item.program], ['count', String(item.count)], ['last_seen', new Date(item.last_seen * 1000).toLocaleString()], ['quantity', window.JournalI18n.cardinalQuantity(item.count)]]);
        host.appendChild(row);
      }
      if (!data.items.length) host.textContent = window.JournalI18n.render('journal.window.health.visible.text.none.since.this.journal.started');
      section.hidden = false;
    } catch (_) {
      section.querySelector('[data-local-programs]').textContent = window.JournalI18n.render('journal.window.health.visible.text.this.list.could.not.be.read');
      section.hidden = false;
    }
  }

  function refreshStatusReads() {
    loadLocalPrograms();
    loadRegisteredClients();
    loadBackupStatus();
    loadSpeechSetup();
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
        heading: window.JournalI18n.render('journal.window.health.heading.health.state.couldn.t.be.loaded'),
        desc: window.CONVEY_COPY?.RELOAD_HINT || window.JournalI18n.render('journal.window.health.window.explanation.reload.to.try.again'),
        retry: true,
        retryLabel: window.JournalI18n.render('journal.window.health.button.label.try.again'),
        detail: error,
        headingLevel: 'h3'
      });
    } else {
      errorHost.innerHTML = `<button type="button" class="surface-state-retry">${window.JournalI18n.renderHtml('journal.window.health.button.label.try.again')}</button>`;
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
      line.textContent = data.scope ? window.JournalI18n.render('journal.window.health.backlog.verdict.scoped', [['scope', data.scope], ['verdict', data.verdict || '']]) : data.verdict || '';
      return;
    }
    const link = document.createElement('a');
    link.className = 'backlog-verdict-day';
    link.href = `/app/transcripts/${encodeURIComponent(day)}`;
    if (pending === 1 && stuckCount === 0) {
      line.textContent = '';
      link.textContent = window.JournalI18n.render('journal.window.health.backlog.catching.up.day', [['day', String(label.replace(/^(Today|Yesterday|Tomorrow)$/, (word) => word.toLowerCase()))]]);
      line.appendChild(link);
      return;
    }
    line.replaceChildren(...window.JournalI18n.renderMarkup('journal.window.health.backlog.verdict.with.link', [['verdict', data.verdict || ''], ['day', label]], {elements: { oldest_link: link }}));
  }

  function renderBacklogState(backlogState, searchIndex) {
    const data = backlogState || {};
    backlogCopy = data.copy || {};
    const host = document.querySelector('[data-backlog-stuck-rows]');
    const rowsHost = host && host.querySelector('[data-backlog-rows]');
    const rows = Array.isArray(data.stuck_rows) ? data.stuck_rows : [];
    state.legacyHolds = rows;
    renderHeldWork();
    const verdictLine = document.querySelector('#backlogVerdict .backlog-verdict-line');
    if (verdictLine) renderBacklogVerdict(verdictLine, data, rows.length);
    // Before the nightly run's first chance: calm, not a warning (the day-0 rule).
    const verdictCard = document.getElementById('backlogVerdict');
    if (verdictCard) verdictCard.classList.toggle('is-not-yet', typeof data.not_yet === 'string' && data.not_yet.length > 0);

    const unfinishedLine = document.querySelector('#backlogVerdict .backlog-unfinished-line');
    if (unfinishedLine) {
      const unfinished = data.unfinished_activities || {};
      const activities = Number(unfinished.activities) || 0;
      const dayCount = Number(unfinished.day_count) || 0;
      const oldestDay = typeof unfinished.oldest_day === 'string' ? unfinished.oldest_day : '';
      if (activities > 0 && oldestDay) {
        const label = window.JournalFormat ? window.JournalFormat.day(oldestDay) : oldestDay;
        const formattedDay = label.replace(/^(Today|Yesterday|Tomorrow)$/, (word) => word.toLowerCase());
        const available = dayCount > 1 ? backlogCopy.unfinished_template_many_days
          : dayCount === 1 && (activities === 1
            ? backlogCopy.unfinished_template_one : backlogCopy.unfinished_template_many_one_day);
        if (available) {
          const id = dayCount > 1 ? 'journal.window.stats.backlog.unfinished.multiple.days'
            : 'journal.window.stats.backlog.unfinished.one.day';
          const text = window.JournalI18n.render(id, [
            ['oldest_day', formattedDay], ['count', String(activities)],
            ['quantity', window.JournalI18n.cardinalQuantity(activities)]
          ]);
          const link = document.createElement('a');
          link.className = 'backlog-verdict-day';
          link.href = `/app/transcripts/${encodeURIComponent(oldestDay)}`;
          link.textContent = text;
          unfinishedLine.replaceChildren(link);
          unfinishedLine.hidden = false;
        } else {
          unfinishedLine.hidden = true;
          unfinishedLine.replaceChildren();
        }
      } else {
        unfinishedLine.hidden = true;
        unfinishedLine.replaceChildren();
      }
    }

    const searchLine = document.querySelector('#backlogVerdict .backlog-search-line');
    if (searchLine) {
      if (searchIndex && typeof searchIndex.text === 'string' && searchIndex.text.length > 0) {
        searchLine.textContent = searchIndex.text;
        const index = searchIndex.index;
        if (index) {
          const details = [];
          if (index.queued > 0) details.push(renderCount('journal.window.health.search.queued', index.queued));
          if (index.failed_deletions > 0) details.push(renderCount('journal.window.health.search.removals', index.failed_deletions));
          if (Number.isFinite(index.last_finished_check_at_ms)) {
            const hours = Math.max(0, Math.floor((Date.now() - index.last_finished_check_at_ms) / 3600000));
            const args = [['updated', String(index.check_indexed)], ['removed', String(index.check_removed)],
              ['updated_quantity', window.JournalI18n.cardinalQuantity(index.check_indexed)], ['removed_quantity', window.JournalI18n.cardinalQuantity(index.check_removed)]];
            if (hours >= 1) args.push(['hours', String(hours)], ['hour_quantity', window.JournalI18n.cardinalQuantity(hours)]);
            details.push(window.JournalI18n.render(hours < 1 ? 'journal.window.health.search.last.check.recent'
              : 'journal.window.health.search.last.check.elapsed', args));
          } else {
            details.push(window.JournalI18n.render('journal.window.health.window.explanation.no.finished.search.check.yet'));
          }
          searchLine.textContent = window.JournalI18n.render('journal.window.health.search.summary.details', [['status', searchIndex.text], ['details', details.join('; ')]]);
        }
        searchLine.hidden = false;
      } else {
        searchLine.hidden = true;
        searchLine.textContent = '';
      }
    }

    clearHealthStateError();

    if (host) host.hidden = true;
    if (rowsHost) rowsHost.replaceChildren();
  }

  function renderAgentErrorsState(agentErrors) {
    const data = agentErrors || {};
    state.agentErrorsOk = data.ok !== false;
    state.agentErrorsLoaded = true;
    seedAgentErrors(Array.isArray(data.items) ? data.items : []);
    if (elements.glanceErrorsValue) {
      // G3-202: a missing count is not zero. The glance may never paint a
      // confident '0 errors' before the real number lands.
      const reported = Number(data.count);
      elements.glanceErrorsValue.textContent = state.agentErrorsOk && Number.isFinite(reported)
        ? String(reported)
        : '—';
      elements.glanceErrorsValue.classList.toggle('has-errors', state.agentErrorsOk && reported > 0);
    }
    if (elements.glanceErrorsLabel) {
      elements.glanceErrorsLabel.textContent = data.label || window.JournalI18n.render('journal.window.health.visible.text.errors.today');
    }
    updateStatusSummary();
  }

  function renderMediaBacklog(notSensed) {
    const card = document.getElementById('brain');
    const existing = document.getElementById('mediaBacklogLine');
    const count = Number(notSensed);
    if (!card) return;
    if (Number.isFinite(count) && count > 0) {
      const el = existing || document.createElement('p');
      el.id = 'mediaBacklogLine';
      el.className = 'brain-health-pending';
      el.textContent = renderCount('journal.window.health.backlog.media.segments', count);
      if (!existing) {
        card.appendChild(el);
      }
    } else if (existing) {
      existing.remove();
    }
  }

  async function loadMediaBacklog() {
    try {
      let payload;
      if (window.apiJson) {
        payload = await window.apiJson('/app/health/api/media-backlog', { noAuthRedirect: true });
      } else {
        const response = await fetch('/app/health/api/media-backlog', { headers: { Accept: 'application/json' } });
        if (!response.ok) throw new Error('summary fetch failed');
        payload = await response.json();
      }
      renderMediaBacklog(payload?.segment_backlog?.not_sensed);
    } catch {
      renderMediaBacklog(0);
    }
  }

  function renderWorkProgress(work) {
    const section = document.getElementById('workProgress');
    if (!section) return;
    state.work = work;
    renderHeldWork();
    const inventory = work?.inventory;
    const reach = section.querySelector('[data-work-reach]');
    const rows = section.querySelector('[data-work-bands]');
    rows.replaceChildren();
    // A finished pass stays current until its next one is due, as on the journal.
    const fresh = inventory && Date.now() >= inventory.as_of_ms
      && Date.now() - Math.max(inventory.as_of_ms, inventory.next_pass_ms ?? inventory.as_of_ms) < 180000;
    if (!inventory) {
      reach.textContent = window.JournalI18n.render('journal.window.health.visible.text.the.journal.has.not.finished.checking');
    } else {
      const range = inventory.oldest_day ? inventory.oldest_day + '–' + inventory.newest_day : window.JournalI18n.render('journal.window.health.status.no.recorded.days');
      const args = [['checked', String(inventory.days_inspected)], ['total', String(inventory.days_total)],
        ['range', range], ['quantity', window.JournalI18n.cardinalQuantity(inventory.days_total)],
        ['checked_quantity', window.JournalI18n.cardinalQuantity(inventory.days_inspected)]];
      const freshness = !fresh ? 'stale' : inventory.complete ? 'complete' : 'incomplete';
      if (!fresh) args.push(['checked_at', new Date(inventory.as_of_ms).toLocaleString()]);
      reach.textContent = window.JournalI18n.render(`journal.window.health.work.inspection.${freshness}`, args);
      const names = [window.JournalI18n.render('journal.window.health.status.today'), window.JournalI18n.render('journal.window.health.window.explanation.your.requests.and.yesterday'), window.JournalI18n.render('journal.window.health.status.recent.days'), window.JournalI18n.render('journal.window.health.status.imports'), window.JournalI18n.render('journal.window.health.status.finishing.older.days'), window.JournalI18n.render('journal.window.health.status.updating.older.days')];
      inventory.bands.forEach((band, i) => {
        const row = document.createElement('li');
        const args = [['band', names[i]], ['recordings_ready', String(band.segments_complete)],
          ['recordings_left', String(band.segments_owed)], ['outputs_ready', String(band.daily_units_complete)],
          ['outputs_left', String(band.daily_units_owed)],
          ['recordings_ready_quantity', window.JournalI18n.cardinalQuantity(band.segments_complete)],
          ['recordings_left_quantity', window.JournalI18n.cardinalQuantity(band.segments_owed)],
          ['outputs_ready_quantity', window.JournalI18n.cardinalQuantity(band.daily_units_complete)],
          ['outputs_left_quantity', window.JournalI18n.cardinalQuantity(band.daily_units_owed)]];
        const details = [];
        if (band.activities_owed) details.push(renderCount('journal.window.health.work.band.activities', band.activities_owed));
        if (band.owner_requests) details.push(renderCount('journal.window.health.work.band.owner.requests', band.owner_requests));
        if (band.permission_held) details.push(renderCount('journal.window.health.work.band.permission', band.permission_held));
        if (band.prerequisite_held) details.push(renderCount('journal.window.health.work.band.recordings', band.prerequisite_held));
        if (details.length) args.push(['details', details.join('; ')]);
        row.textContent = window.JournalI18n.render(details.length
          ? 'journal.window.health.work.band.progress.details' : 'journal.window.health.work.band.progress', args);
        rows.appendChild(row);
      });
    }
    const history = section.querySelector('[data-work-history]');
    const progress = inventory?.history;
    history.hidden = !progress;
    history.textContent = '';
    if (progress) {
      const count = value => Number(value).toLocaleString();
      const day = key => new Date(Number(key.slice(0, 4)), Number(key.slice(4, 6)) - 1, Number(key.slice(6, 8)))
        .toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
      const range = progress.caught_up_since
        ? window.JournalI18n.render('journal.window.health.history.caught.up.since', [['day', String(day(progress.caught_up_since))]])
        : inventory.oldest_day && inventory.oldest_day <= progress.through_day
          ? window.JournalI18n.render('journal.window.health.window.explanation.older.history.is.still.catching.up') : window.JournalI18n.render('journal.window.health.window.explanation.no.older.history.to.check');
      const counts = window.JournalI18n.render('journal.window.health.history.counts', [
        ['recordings', count(progress.segments_owed)], ['outputs', count(progress.daily_units_owed)],
        ['recording_quantity', window.JournalI18n.cardinalQuantity(progress.segments_owed)],
        ['output_quantity', window.JournalI18n.cardinalQuantity(progress.daily_units_owed)]
      ]);
      const parts = [range, counts];
      if (fresh && progress.net_per_hour > 0) parts.push(window.JournalI18n.render('journal.window.health.history.rate.hourly', [['rate', String(progress.net_per_hour.toLocaleString(undefined, { maximumSignificantDigits: 3 }))]]));
      if (progress.set_aside_units) parts.push(renderCount('journal.window.health.history.held.count', progress.set_aside_units, count(progress.set_aside_units)));
      if (progress.unreadable_days) parts.push(renderCount('journal.window.health.history.unreadable.count', progress.unreadable_days, count(progress.unreadable_days)));
      history.textContent = window.JournalI18n.render(fresh ? 'journal.window.health.history.summary.current'
        : 'journal.window.health.history.summary.stale', [['metrics', parts.join(' · ')], ['through_day', day(progress.through_day)]]);
    }
    const cost = section.querySelector('[data-work-cost]');
    const older = inventory?.bands?.slice(4);
    const count = (key) => older?.reduce((total, band) => total + (band[key] || 0), 0);
    cost.textContent = older && fresh && inventory.complete
      ? window.JournalI18n.renderEnglish('journal.window.health.history.cost.consent', [['recordings', String(count('segments_owed'))], ['day_outputs', String(count('daily_units_owed'))], ['activities', String(count('activities_owed'))]])
      : window.JournalI18n.renderEnglish('journal.window.health.visible.text.finishing.older.recordings.and.day.updates');
    section.querySelector('[data-work-confirm]').hidden = !work?.confirmation_available || work.history_confirmed;
    const details = section.querySelector('[data-work-details]');
    const errors = [...(work?.error ? [work.error] : inventory?.unreadable || []), ...(work?.retry_errors || [])];
    details.hidden = errors.length === 0;
    details.querySelector('pre').textContent = errors.join('\n');
    const button = section.querySelector('[data-work-allow]');
    button.onclick = async () => {
      const result = section.querySelector('[data-work-action-result]');
      result.hidden = false;
      result.textContent = window.JournalI18n.render('journal.window.health.visible.text.saving.your.choice');
      try {
        const response = await fetch('/app/settings/api/config', {
          method: 'PUT', headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ section: 'processing', data: { history_confirmed: true } }),
        });
        if (!response.ok) throw new Error('save failed');
        result.textContent = window.JournalI18n.render('journal.window.health.visible.text.older.work.is.allowed.to.use');
        await loadHealthState();
      } catch {
        result.textContent = window.JournalI18n.render('journal.window.health.visible.text.your.choice.could.not.be.saved');
      }
    };
  }

  let healthStateRequest = null;
  function loadHealthState() {
    if (!healthStateRequest) {
      healthStateRequest = readHealthState().finally(() => { healthStateRequest = null; });
    }
    return healthStateRequest;
  }

  async function readHealthState() {
    clearHealthStateError();
    try {
      const payload = await getJson('/app/health/api/state');
      renderStoragePrivacy(payload.storage_privacy);
      renderWorkProgress(payload.work);
      state.searchIndex = payload.search_index || null;
      renderBacklogState(payload.backlog, payload.search_index);
      renderAgentErrorsState(payload.agent_errors);
      reportHealthReady('content');
    } catch (error) {
      renderHealthStateError(error);
      reportHealthReady('error');
    } finally {
      // This independent detail must not compete with the initial index read.
      loadMediaBacklog();
    }
  }

  function renderStoragePrivacy(privacy) {
    state.storagePrivacy = privacy || null;
    const section = document.getElementById('storagePrivacy');
    if (!section) return;
    section.hidden = !privacy || privacy.private;
    if (!privacy || privacy.private) return;
    section.querySelector('[data-storage-privacy-message]').textContent = privacy.message;
    const button = section.querySelector('[data-storage-privacy-consent]');
    button.hidden = privacy.consented || privacy.unavailable;
    section.style.background = privacy.consented ? '' : 'var(--warn-wash)';
    button.onclick = async () => {
      const result = section.querySelector('[data-storage-privacy-result]');
      button.disabled = true;
      result.hidden = true;
      try {
        const saved = await getJson('/app/health/api/storage-privacy/consent', {
          method: 'POST', headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ filesystem_type: privacy.filesystem_type }),
        });
        renderStoragePrivacy(saved);
        updateStatusSummary();
      } catch (error) {
        result.textContent = error.error || window.JournalI18n.render('journal.window.health.visible.text.your.choice.could.not.be.saved.variant2');
        result.hidden = false;
      } finally { button.disabled = false; }
    };
  }

  let timeoutFired = false;
  let programmaticScroll = false;
  const STALE_MS = 30000;
  // Human-readable service names
  // Owner words for every service the supervisor can report. A service with no
  // entry here is a codename on the trust page, so the map covers the ones that
  // ship (spl, parakeet) rather than letting them through raw (X-18).
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

  function serviceName(internal) {
    return SERVICE_NAMES[internal] || String(internal || '').replace(/[_:-]+/g, ' ');
  }

  // Readable names for talent ids that do not humanize cleanly; everything else
  // falls back to the humanized id. Mirrors thinking.js talentLabel so the two
  // surfaces name the same talent the same way (X-18).
  const TALENT_NAMES = {
    'entities:detection': window.JournalI18n.render('journal.window.health.talent.names.entities.detection.entity.detection'),
    'entities:entity_observer': window.JournalI18n.render('journal.window.health.talent.names.entities.entity.observer.entity.facts'),
    'entities:possible_names': window.JournalI18n.render('journal.window.health.talent.names.entities.possible.names.possible.names'),
    screen: window.JournalI18n.render('journal.window.health.talent.names.screen.screen.descriptions'),
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
    'brain': window.JournalI18n.render('journal.window.health.schedule.names.brain.processing.check'),
    'cadence': window.JournalI18n.render('journal.window.health.schedule.names.cadence.processing.schedule'),
    'heartbeat': window.JournalI18n.render('journal.window.health.schedule.names.heartbeat.journal.review'),
    'facet-candidates': window.JournalI18n.render('journal.window.health.schedule.names.facet.candidates.facet.suggestions'),
    'maintenance:backup:run': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.backup.run.backup'),
    'maintenance:backup:prune': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.backup.prune.backup.cleanup'),
    'maintenance:backup:verify': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.backup.verify.backup.check'),
    'maintenance:backup:offload': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.backup.offload.original.media.cleanup'),
    'maintenance:health:mark-raw': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.health.mark.raw.original.media.review'),
    'maintenance:health:prune-logs': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.health.prune.logs.log.cleanup'),
    'maintenance:retention:owner-voice-originals': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.retention.owner.voice.audio.deletion.catch.up'),
    'maintenance:trash:erase': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.trash.erase.trash.erase'),
    'maintenance:speakers:discover-voices': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.speakers.discover.voices.voice.discovery'),
    'maintenance:speakers:candidate-pair-suggestions': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.speakers.candidate.pair.speaker.suggestions'),
    'maintenance:speakers:name-variants': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.speakers.name.variants.speaker.name.suggestions'),
    'maintenance:speakers:consolidate-pool': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.speakers.consolidate.pool.speaker.cleanup'),
    'maintenance:speakers:backfill-pool': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.speakers.backfill.pool.voice.list.catch.up'),
    'maintenance:speakers:backfill-labels': window.JournalI18n.render('journal.window.health.schedule.names.maintenance.speakers.backfill.labels.speaker.naming.catch.up'),
  };

  function scheduleName(name) {
    return SCHEDULE_NAMES[String(name || '')] || null;
  }

  // G3-201: a vital that has not answered yet is loading, not unavailable.
  // "unavailable" is a verdict about a service and the worst one to guess, so
  // every vitals cell keeps the same pulsing placeholder services and health
  // already use until either its value lands or the sweep below gives up.
  function vitalPending(el) {
    return Boolean(el && el.querySelector('.skeleton, .skeleton-dark'));
  }

  function writeVital(el, seen, value) {
    if (!el) return;
    if (seen) {
      el.replaceChildren(document.createTextNode(value));
      return;
    }
    if (vitalPending(el)) return;
    el.replaceChildren(document.createTextNode(window.JournalI18n.render('journal.window.health.status.unavailable')));
  }

  // The word a vitals group reads out before its value lands. Screen readers
  // heard "unavailable" permanently because the static aria-label was never
  // rewritten; it is written from this path now, alongside the value.
  function pendingVitalLabel() {
    return timeoutFired ? window.JournalI18n.render('journal.window.health.status.unavailable') : window.JournalI18n.render('journal.window.health.status.loading');
  }

  function sweepUnresolvedSkeletons() {
    if (timeoutFired) return;
    timeoutFired = true;

    const targets = [
      elements.vitalsStatus,
      elements.serviceDots,
      elements.healthValue,
      elements.agentsValue,
      elements.tasksValue,
      elements.queuesValue,
      elements.schedulesValue,
    ];
    for (const el of targets) {
      if (el && el.querySelector('.skeleton, .skeleton-dark')) {
        el.textContent = window.JournalI18n.render('journal.window.health.visible.text.unavailable');
        el.classList.add('unavailable');
      }
    }
    updateVitalsA11y();
    // The brain card narrates its own pending state, so it has to hear the
    // same timeout the vitals do (F-16).
    renderBrainHealth();
  }

  // The supervisor reports status every 5 s, so a vital still pending 7 s
  // after the live stream connects is unavailable. The clock starts at the
  // connection, not at page load: on a slow link the stream can take longer
  // than that to open, and a value that is still on its way is "loading".
  let skeletonTimer = null;
  function armSkeletonTimeout() {
    if (timeoutFired || skeletonTimer) return;
    skeletonTimer = setTimeout(sweepUnresolvedSkeletons, 7000);
  }

  // Utility functions
  function formatElapsed(seconds) {
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${minutes}m ${secs}s`;
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
    if (value < 60000) return window.JournalI18n.render('journal.window.health.status.just.now');
    return window.JournalI18n.render('journal.window.health.time.legacy.age', [['duration', relativeTime(value)]]);
  }

  // A duration that is still running is not an age, so it gets the ladder
  // without the "ago" and with the floor said as a length of time rather than
  // "just now". A bare "8s" is the same defect as "0 seconds ago" (G3-104).
  function runningFor(ms) {
    const value = Number.isFinite(ms) && ms > 0 ? ms : 0;
    if (value < 60000) return window.JournalI18n.render('journal.window.health.status.under.a.minute');
    return relativeTime(value);
  }

	  function truncate(str, len) {
	    if (!str) return '';
	    return str.length > len ? str.substring(0, len).replace(/\s+\S*$/, '') + '…' : str;
	  }

	  function dayKeyFromTimestamp(ts) {
	    if (window.JournalClock) {
	      return window.JournalClock.dayKey(ts);
	    }
	    const date = new Date(ts);
	    if (Number.isNaN(date.getTime())) return '';
	    const year = String(date.getFullYear());
	    const month = String(date.getMonth() + 1).padStart(2, '0');
	    const day = String(date.getDate()).padStart(2, '0');
	    return year + month + day;
	  }

	  function todayKey() {
	    if (window.JournalClock) {
	      return window.JournalClock.today();
	    }
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
    if (delta < 0) return window.JournalI18n.render('journal.window.health.status.due.now');
    // Spelled units, same ladder as every other age on the page (G3-104).
    if (delta < 60000) return window.JournalI18n.render('journal.window.health.window.explanation.in.under.a.minute');
    return window.JournalI18n.render('journal.window.health.schedule.next.duration', [['duration', String(relativeTime(delta))]]);
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
    return client.display_label || client.device_label || client.cid_short || client.cid || window.JournalI18n.render('journal.window.health.status.an.unnamed.device');
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

  function rejectedClientSources(client) {
    return Object.entries(client.source_delivery || {})
      .filter(([, source]) => source.state === 'needs_attention');
  }

  function registeredClientNeedsAttention(client) {
    return client.failing === true || rejectedClientSources(client).length > 0;
  }

  function describeRejectedClient(client) {
    const sources = rejectedClientSources(client).map(([name]) => name || window.JournalI18n.render('journal.window.health.status.default'));
    return registeredClientName(client) + (sources.length ? ` (${sources.join(', ')})` : '');
  }

  // The device-delivery half of the verdict. Returns null when device delivery
  // has nothing to say, so the caller falls through to the other signals.
  function selectDeviceVerdict() {
    const clients = state.registeredClients;
    if (state.registeredClientsFailed) {
      return { key: 'HEALTH_GLANCE_DEVICES_UNAVAILABLE', vars: {}, action: GLANCE_DEVICES_ACTION };
    }
    if (!Array.isArray(clients)) {
      // No derived status is its own honest state — it is never an upgrade to green.
      return {
        key: 'HEALTH_GLANCE_CHECKING',
        vars: {},
        action: null,
      };
    }
    const failing = clients.filter(registeredClientNeedsAttention).sort(byQuietestFirst);
    if (failing.length === 1) {
      return {
        key: 'HEALTH_GLANCE_DEVICE_FAILING',
        vars: { device: describeRejectedClient(failing[0]) },
        action: GLANCE_DEVICES_ACTION,
      };
    }
    if (failing.length > 1) {
      return {
        key: 'HEALTH_GLANCE_DEVICES_FAILING',
        vars: { count: failing.length, devices: failing.map(describeRejectedClient).join(', ') },
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
        vars: age ? { device: registeredClientName(silent[0]), age }
          : { device: registeredClientName(silent[0]) },
        action: GLANCE_DEVICES_ACTION,
      };
    }
    if (silent.length > 1) {
      return {
        key: 'HEALTH_GLANCE_DEVICES_SILENT',
        vars: { count: silent.length, devices: silent.map(describeRegisteredClient).join(', ') },
        action: GLANCE_DEVICES_ACTION,
      };
    }
    if (clients.some(client => client.capture_state === 'unknown')) {
      return { key: 'HEALTH_GLANCE_DEVICES_UNKNOWN', vars: {}, action: GLANCE_DEVICES_ACTION };
    }
    return null;
  }

  // Only the complete status snapshot joins a hold to queued intended work.
  // The one-off held event does not establish that consequence.
  function queuedWorkHolds(snapshot) {
    if (!snapshot) return [];
    const queues = Object.entries(snapshot.queues).filter(([, count]) => count > 0);
    const holds = snapshot.held.filter(hold => snapshot.queues[hold.partition] > 0)
      .map(hold => ({ key: 'partition:' + hold.partition, partition: hold.partition,
        count: snapshot.queues[hold.partition], evidence: hold }));
    if (snapshot.queueHold && queues.length) {
      holds.unshift({ key: 'queue', partition: null,
        count: queues.reduce((sum, [, count]) => sum + count, 0),
        evidence: { ...snapshot.queueHold, queues: Object.fromEntries(queues) } });
    }
    return holds;
  }

  function setAsideHolds() {
    const inventory = state.work?.inventory;
    const records = new Map((state.work?.retry_records || []).map(record => [record.id, record]));
    const holds = (inventory?.holds || []).map((hold, index) => ({
      key: hold.retry_unit ? 'unit:' + hold.retry_unit : 'work:' + [hold.day, hold.stream, hold.segment, hold.reason].join(':'),
      unit: hold.retry_unit, day: hold.day, reason: hold.reason, count: hold.count || 1,
      label: [hold.day, hold.segment, talentName(records.get(hold.retry_unit)?.unit?.talent)].filter(Boolean).join(' · '),
      evidence: records.get(hold.retry_unit) || hold,
    }));
    const days = new Set(holds.map(hold => hold.day));
    for (const row of state.legacyHolds || []) {
      if (!days.has(row.day)) holds.push({ key: 'day:' + row.day, day: row.day,
        displayReason: row.reason, label: row.day, evidence: row });
    }
    return [...new Map(holds.map(hold => [hold.key, hold])).values()];
  }

  function heldReason(reason) {
    const reasons = {
      corrupt_input: window.JournalI18n.render('journal.window.health.window.explanation.the.recording.could.not.be.read'),
      no_audio_stream: window.JournalI18n.render('journal.window.health.window.explanation.the.recording.has.no.audio.stream'),
      'embedded-duration-budget': window.JournalI18n.render('journal.window.health.window.explanation.this.recording.exceeds.the.audio.processing'),
      'embedded-input-budget': window.JournalI18n.render('journal.window.health.window.explanation.this.recording.is.larger.than.the'),
      segment_prerequisites: window.JournalI18n.render('journal.window.health.window.explanation.waiting.for.the.recordings.on.this'),
      spending_confirmation: window.JournalI18n.render('journal.window.health.window.explanation.waiting.for.permission.to.process.older'),
      thinking_unavailable: window.JournalI18n.render('journal.window.health.window.explanation.choose.a.model.in.thinking.before'),
      retry_wait: window.JournalI18n.render('journal.window.health.window.explanation.waiting.before.another.attempt'),
      analysis_failed: window.JournalI18n.render('journal.window.health.window.explanation.analysis.has.failed.repeatedly'),
      schema_invalid: window.JournalI18n.render('journal.window.health.window.explanation.the.model.repeatedly.returned.an.unusable'),
      schema_validation_failed: window.JournalI18n.render('journal.window.health.window.explanation.the.model.repeatedly.returned.an.unusable'),
    };
    return reasons[reason] || (reason && reason.includes(' ') ? reason : window.JournalI18n.render('journal.window.health.window.explanation.this.processing.step.has.been.set'));
  }

  function heldWorkVerdict() {
    const action = { href: '#processingHolds', label: window.JournalI18n.render('journal.window.health.label.view.queued.work') };
    const inventory = state.work?.inventory;
    const workStale = inventory && !(Date.now() >= inventory.as_of_ms
        && Date.now() - Math.max(inventory.as_of_ms, inventory.next_pass_ms ?? inventory.as_of_ms) < 180000);
    if (workStale || state.work?.error) {
      return { key: 'HEALTH_GLANCE_WORK_UNAVAILABLE', vars: {}, action };
    }
    if (!state.heldWorkCurrent) {
      const grace = state.queuedWorkAuthoritySeen !== true && typeof state.loadedAt === 'number' && Date.now() - state.loadedAt < 15000;
      return grace ? null : { key: 'HEALTH_GLANCE_WORK_UNAVAILABLE', vars: {}, action };
    }
    return (queuedWorkHolds(state.heldWorkSnapshot).length + setAsideHolds().length)
      ? { key: 'HEALTH_GLANCE_WORK_HELD', vars: {}, action } : null;
  }

  let heldWorkDeepLinkScrolled = false;
  let heldWorkPage = 0;
  const HELD_WORK_PAGE_SIZE = 30;

  function renderHeldWork() {
    const section = document.getElementById('processingHolds');
    if (!section) return;
    const holds = [...queuedWorkHolds(state.heldWorkSnapshot), ...setAsideHolds()];
    const inventory = state.work?.inventory;
    const workStale = inventory && !(Date.now() >= inventory.as_of_ms
        && Date.now() - Math.max(inventory.as_of_ms, inventory.next_pass_ms ?? inventory.as_of_ms) < 180000);
    const current = state.heldWorkCurrent && !workStale && !state.work?.error;
    section.hidden = current && holds.length === 0;
    section.dataset.current = String(current);
    const notice = section.querySelector('[data-held-work-notice]');
    notice.textContent = workStale ? window.JournalI18n.render('journal.window.health.work.hold.stale', [['checked_at', new Date(inventory.as_of_ms).toLocaleString()]]) : current ? '' : state.heldWorkSnapshot
      ? window.JournalI18n.render('journal.window.health.work.hold.unavailable', [['checked_at', new Date(state.heldWorkSnapshot.observedAt).toLocaleString()]])
      : window.JournalI18n.render('journal.window.health.visible.text.queued.work.status.is.unavailable');
    notice.hidden = current;
    const rows = section.querySelector('[data-held-work-rows]');
    heldWorkPage = Math.min(heldWorkPage, Math.max(0, Math.ceil(holds.length / HELD_WORK_PAGE_SIZE) - 1));
    const visible = holds.slice(heldWorkPage * HELD_WORK_PAGE_SIZE, (heldWorkPage + 1) * HELD_WORK_PAGE_SIZE);
    const pager = section.querySelector('[data-held-work-pages]');
    if (pager) {
      pager.hidden = holds.length <= HELD_WORK_PAGE_SIZE;
      pager.querySelector('span').textContent = window.JournalI18n.render('journal.window.health.work.hold.page', [['page', String(heldWorkPage + 1)], ['pages', String(Math.max(1, Math.ceil(holds.length / HELD_WORK_PAGE_SIZE)))]]);
      const previous = pager.querySelector('[data-held-previous]');
      const next = pager.querySelector('[data-held-next]');
      previous.disabled = heldWorkPage === 0;
      next.disabled = (heldWorkPage + 1) * HELD_WORK_PAGE_SIZE >= holds.length;
      previous.onclick = () => { heldWorkPage -= 1; renderHeldWork(); };
      next.onclick = () => { heldWorkPage += 1; renderHeldWork(); };
    }
    const keys = new Set(visible.map(hold => hold.key));
    for (const row of Array.from(rows.children)) {
      if (!keys.has(row.dataset.key)) rows.removeChild(row);
    }
    const existingRows = new Map(Array.from(rows.children).map(row => [row.dataset.key, row]));
    for (const hold of visible) {
      let row = existingRows.get(hold.key);
      if (!row) {
        row = document.createElement('div');
        row.className = 'held-work-row';
        row.dataset.key = hold.key;
        const line = document.createElement('p');
        const details = document.createElement('details');
        const summary = document.createElement('summary');
        summary.textContent = window.JournalI18n.render('journal.window.health.visible.text.technical.details');
        details.append(summary, document.createElement('pre'));
        const retry = document.createElement('button');
        retry.type = 'button';
        retry.textContent = window.JournalI18n.render('journal.window.health.visible.text.try.again');
        const feedback = document.createElement('p');
        feedback.dataset.retryFeedback = '';
        feedback.setAttribute('role', 'status');
        row.append(line, retry, feedback, details);
        rows.appendChild(row);
      }
      const names = { daily: window.JournalI18n.render('journal.window.health.status.daily.processing'), activity: window.JournalI18n.render('journal.window.health.status.activity.processing'),
        segment: window.JournalI18n.render('journal.window.health.status.segment.processing'), flush: window.JournalI18n.render('journal.window.health.status.finishing.activities'), weekly: window.JournalI18n.render('journal.window.health.status.weekly.reflection'),
        indexer: window.JournalI18n.render('journal.window.health.status.search.updates') };
      const scope = hold.partition
        ? names[hold.partition] || scheduleName(hold.partition) || window.JournalI18n.render('journal.window.health.status.queued.processing')
        : window.JournalI18n.render('journal.window.health.status.all.queued.work');
      const reason = hold.displayReason || heldReason(hold.reason);
      let line;
      if (hold.label) {
        line = hold.count > 1 ? window.JournalI18n.render('journal.window.health.work.hold.reason.count', [['item', hold.label], ['reason', reason], ['count', String(hold.count)], ['quantity', window.JournalI18n.cardinalQuantity(hold.count)]])
          : window.JournalI18n.render('journal.window.health.work.hold.reason', [['item', hold.label], ['reason', reason]]);
      } else {
        const id = state.heldWorkCurrent ? 'journal.window.health.work.hold.tasks.current'
          : 'journal.window.health.work.hold.tasks.last.check';
        line = window.JournalI18n.render(id, [['scope', scope], ['count', String(hold.count)],
          ['quantity', window.JournalI18n.cardinalQuantity(hold.count)]]);
      }
      row.querySelector('p').textContent = line;
      row.querySelector('pre').textContent = JSON.stringify(hold.evidence, null, 2);
      const retry = row.querySelector('button');
      retry.onclick = async () => {
        const feedback = row.querySelector('[data-retry-feedback]');
        retry.disabled = true;
        feedback.textContent = window.JournalI18n.render('journal.window.health.visible.text.checking.this.work');
        try {
          const endpoint = hold.unit || !hold.day ? '/app/health/api/retry-work' : '/app/health/api/reprocess';
          const body = hold.unit ? { unit: hold.unit } : hold.day ? { day: hold.day, flavor: 'process-now' } : { safety: true };
          const response = await fetch(endpoint, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
          const result = await response.json();
          feedback.textContent = result.message || (response.ok ? window.JournalI18n.render('journal.window.health.visible.text.your.request.is.saved') : window.JournalI18n.render('journal.window.health.visible.text.this.work.could.not.be.retried'));
          await loadHealthState();
        } catch {
          feedback.textContent = window.JournalI18n.render('journal.window.health.visible.text.the.retry.could.not.be.saved');
        } finally { retry.disabled = false; }
      };
    }
    section.querySelector('[data-held-work-support]').hidden = holds.length === 0;
    if (!section.hidden && !heldWorkDeepLinkScrolled && window.location.hash === '#processingHolds') {
      heldWorkDeepLinkScrolled = true;
      const list = section.querySelector('[data-held-work-list]');
      if (list) list.open = true;
      requestAnimationFrame(() => section.scrollIntoView({ block: 'start' }));
    }
  }

	  function selectGlanceSentence(state, now) {
    const activeAgents = Array.from(state.agents.values()).filter(agent => agent.event !== 'finish' && agent.event !== 'error').length;
    const activeImports = Array.from(state.imports.values()).filter(imp => imp.event !== 'completed' && imp.event !== 'error').length;
    const staleHeartbeats = state.health?.stale_heartbeats || [];

    if (state.connected === false && connectError === true) {
      return { key: 'HEALTH_GLANCE_SERVICES_UNREACHABLE', vars: {} };
    }

    if (state.storagePrivacy && !state.storagePrivacy.private && !state.storagePrivacy.consented) {
      return { key: 'HEALTH_GLANCE_STORAGE_PRIVACY', vars: { message: state.storagePrivacy.message },
        action: { href: '#storagePrivacy', label: window.JournalI18n.render('journal.window.health.label.review.folder.privacy') } };
    }

    if (state.crashed.size > 0) {
      const names = Array.from(state.crashed.keys()).sort();
      return {
        key: names.length === 1 ? 'HEALTH_GLANCE_SERVICE_ATTENTION' : 'HEALTH_GLANCE_SERVICES_ATTENTION',
        vars: names.length === 1
          ? { services: serviceName(names[0]) }
          : { count: names.length, services: names.map(serviceName).join(', ') },
      };
    }

    // A stale check-in is a journal run that stopped updating its heartbeat:
    // an earlier run here or one on another computer, never a service of this
    // run. It keeps the page off green, with its own sentence and its names as
    // written.
    if (staleHeartbeats.length > 0) {
      const names = Array.from(new Set(staleHeartbeats.map(String))).sort();
      return names.length === 1
        ? { key: 'HEALTH_GLANCE_CHECKIN_STALE', vars: { run: names[0] } }
        : { key: 'HEALTH_GLANCE_CHECKINS_STALE', vars: { count: names.length, runs: names.join(', ') } };
    }

    if (brainSnapshot && ['blocked', 'unhealthy', 'unknown'].includes(brainSnapshot.state)) {
      return {
        key: 'HEALTH_GLANCE_BRAIN_ATTENTION',
        vars: { headline: brainSnapshot.headline || '' },
      };
    }

    // Device delivery is part of the verdict, not only of the rows below it.
    const deviceVerdict = selectDeviceVerdict();
    const deviceUnassessed = deviceVerdict && ['HEALTH_GLANCE_DEVICES_UNAVAILABLE', 'HEALTH_GLANCE_DEVICES_UNKNOWN', 'HEALTH_GLANCE_CHECKING'].includes(deviceVerdict.key);
    if (deviceVerdict && !deviceUnassessed) return deviceVerdict;

    const workVerdict = heldWorkVerdict();
    if (workVerdict?.key === 'HEALTH_GLANCE_WORK_HELD'
        && state.searchIndex?.state !== 'failing') return workVerdict;

    const clients = Array.from(state.clients.values());
    if (!deviceUnassessed && clients.length > 0 && clients.every(client => (now - client.lastSeen) >= STALE_MS)) {
      const ageMs = Math.min(...clients.map(client => now - client.lastSeen));
      return {
        key: 'HEALTH_GLANCE_CLIENT_SILENT',
        vars: { age: relativeTime(ageMs) },
        action: GLANCE_DEVICES_ACTION,
      };
    }

    if (state.searchIndex && state.searchIndex.state === 'failing' && state.searchIndex.text) {
      return {
        key: 'HEALTH_GLANCE_BRAIN_ATTENTION',
        vars: { headline: state.searchIndex.text },
      };
    }

    const backupVerdict = state.backupView.tone !== 'neutral'
      ? { key: 'HEALTH_GLANCE_BACKUP', vars: { headline: state.backupView.text },
        action: { href: '#backup', label: window.JournalI18n.render('journal.window.health.label.view.backup.details') } } : null;
    if (state.backupView.tone === 'attention') return backupVerdict;

    if (deviceVerdict) return deviceVerdict;
    if (workVerdict) return workVerdict;
    if (backupVerdict) {
      return { key: 'HEALTH_GLANCE_BACKUP', vars: { headline: state.backupView.text },
        action: { href: '#backup', label: window.JournalI18n.render('journal.window.health.label.view.backup.details') } };
    }

	    if (activeAgents > 0 || activeImports > 0) {
      const catchingUp = activeAgents + activeImports;
      return {
        key: 'HEALTH_GLANCE_CATCHING_UP',
        vars: {
          count: catchingUp,
          age: ageAgo(now - (state.lastEventTs || now)),
        },
      };
    }

    if (state.services.size > 0 || state.crashed.size > 0) {
      return {
        key: 'HEALTH_GLANCE_OK',
        vars: {},
      };
    }

    return null;
  }

  function formatGlanceSentence(selection) {
    if (!selection) return '';
    const vars = selection.vars || {};
    if (selection.key === 'HEALTH_GLANCE_STORAGE_PRIVACY') return vars.message || '';
    if (selection.key === 'HEALTH_GLANCE_BACKUP' || selection.key === 'HEALTH_GLANCE_BRAIN_ATTENTION') {
      return vars.headline || '';
    }
    const id = HEALTH_GLANCE_MESSAGES[selection.key];
    if (!id) return '';
    const args = Object.entries(vars).map(([key, value]) => [key, String(value ?? '')]);
    if (Object.hasOwn(vars, 'count')) args.push(['quantity', window.JournalI18n.cardinalQuantity(vars.count)]);
    return window.JournalI18n.render(id, args);
  }

  const GLANCE_ATTENTION_KEYS = new Set([
    'HEALTH_GLANCE_STORAGE_PRIVACY',
    'HEALTH_GLANCE_BACKUP',
    'HEALTH_GLANCE_SERVICES_UNREACHABLE',
    'HEALTH_GLANCE_SERVICE_ATTENTION',
    'HEALTH_GLANCE_SERVICES_ATTENTION',
    'HEALTH_GLANCE_BRAIN_ATTENTION',
    'HEALTH_GLANCE_CHECKIN_STALE',
    'HEALTH_GLANCE_CHECKINS_STALE',
    'HEALTH_GLANCE_DEVICE_FAILING',
    'HEALTH_GLANCE_DEVICES_FAILING',
    'HEALTH_GLANCE_DEVICES_UNAVAILABLE',
    'HEALTH_GLANCE_WORK_HELD',
    'HEALTH_GLANCE_WORK_UNAVAILABLE',
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
    renderHeldWork();
    const now = Date.now();
    const selection = selectGlanceSentence(state, now);
    renderGlanceVerdict(selection);

    const today = todayKey();
    if (!state.agentErrorsOk || !state.agentErrorsLoaded) {
      elements.glanceErrorsValue.textContent = '—';
      elements.glanceErrorsValue.classList.remove('has-errors');
      elements.glanceErrorsLabel.textContent = window.JournalI18n.render('journal.window.health.visible.text.errors.today');
    } else {
      const errorsToday = state.recentErrors.filter(error => dayKeyFromTimestamp(error.ts) === today).length;
      elements.glanceErrorsValue.textContent = String(errorsToday);
      elements.glanceErrorsValue.classList.toggle('has-errors', errorsToday > 0);
      elements.glanceErrorsLabel.textContent = (errorsToday === 1 ? window.JournalI18n.render('journal.window.health.visible.text.error.today') : window.JournalI18n.render('journal.window.health.visible.text.errors.today'));
    }
  }

  function renderBrainHealth() {
    const box = elements.brainHealthStatus;
    if (!box) return;
    // G3-304: on a cold load every neighbouring card narrates and this one
    // painted an empty white box, which reads as broken rather than as loading.
    if (!brainSnapshot) {
      box.replaceChildren();
      const pending = document.createElement('p');
      pending.className = 'brain-health-pending';
      // G3-304 painted an empty box on a cold load, so this card narrates. But
      // a read that never lands is not still loading: once the page's own
      // timeout fires the card says so and the retry comes back, instead of
      // waiting forever with the only way out hidden (F-16).
      pending.textContent = timeoutFired ? window.JournalI18n.render('journal.window.health.visible.text.processing.status.unavailable') : window.JournalI18n.render('journal.window.health.visible.text.checking.processing');
      if (timeoutFired) pending.classList.add('unavailable');
      box.appendChild(pending);
      const button = elements.brainCheckBtn;
      if (button) {
        button.hidden = !timeoutFired;
        if (timeoutFired) {
          button.textContent = window.JournalI18n.render('journal.window.health.visible.text.check.again');
          button.disabled = false;
          button.onclick = () => requestBrainCheck();
        }
      }
      return;
    }
    const brain = brainSnapshot;
    const identity = brain.identity || {};
    const evidence = brain.evidence || {};
    const lines = [];
    if (brain.headline) lines.push(brain.headline);
    if (identity.lane && identity.provider && identity.model) {
      if (brain.state === 'ready') {
        // age_text is the server's compact form ("10h"); the page speaks one
        // vocabulary, so derive from age_seconds and fall back only if absent.
        const checkedAge = Number.isFinite(evidence.age_seconds)
          ? ageAgo(evidence.age_seconds * 1000)
          : (evidence.age_text ? window.JournalI18n.render('journal.window.health.time.legacy.age', [['duration', evidence.age_text]]) : '');
        const lane = window.JournalFormat.processingLane(identity.lane);
        lines.push(checkedAge ? window.JournalI18n.render('journal.window.health.thinking.brain.checked', [['lane', lane], ['age', checkedAge]]) : lane);
      } else {
        lines.push(window.JournalI18n.render('journal.window.health.thinking.brain.reason', [['lane', window.JournalFormat.processingLane(identity.lane)], ['reason', brain.reason_text || '']]));
      }
    } else if (identity.lane || identity.provider || identity.model) {
      lines.push(brain.reason_text || '');
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
      ? renderCount('journal.window.health.thinking.tokens.today', state.todayTokens, String(window.JournalFormat.compactTokens(state.todayTokens)))
      : '';
    if (identityText || tokensText) {
      const details = document.createElement('details');
      details.open = identityOpen;
      const summary = document.createElement('summary');
      summary.textContent = window.JournalI18n.render('journal.window.health.visible.text.processing.details');
      details.append(summary);
      if (identityText) {
        const value = document.createElement('p');
        value.textContent = identityText;
        details.append(value);
      }
      if (tokensText) {
        const tokens = document.createElement('p');
        const link = document.createElement('a');
        link.className = 'brain-tokens-link';
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
    // G3-301: the vitals bar read 'TALENTS 1 running' three cards above
    // 'everything is idle'. The card's claim covers talents, so the talent
    // count is part of the test that lets it speak.
    const allHidden = state.supervisorSeen && state.cortexSeen && state.tasks.length === 0 &&
      state.agentCount === 0 &&
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

    // A finish time is a claim about the talent read. When that read never
    // landed, the vital says 'unavailable' and this line may not contradict it.
    if (state.lastAgentFinishTs && state.cortexSeen) {
      const el = ensureChild(idx++);
      el.textContent = window.JournalI18n.render('journal.window.health.thinking.last.run.finished', [['age', ageAgo(Date.now() - state.lastAgentFinishTs)]]);
      el.style.color = '';
    }

    const errCount = state.recentErrors.length;
    const errEl = ensureChild(idx++);
    if (!state.agentErrorsOk) {
      errEl.textContent = window.JournalI18n.render('journal.window.health.visible.text.couldn.t.check.thinking.errors.today');
      errEl.style.color = 'var(--warn-ink)';
    } else if (errCount > 0) {
      errEl.textContent = renderCount('journal.window.health.thinking.recent.errors', errCount);
      errEl.style.color = 'var(--danger)';
    } else {
      errEl.textContent = window.JournalI18n.render('journal.window.health.visible.text.no.recent.errors');
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
        ? window.JournalI18n.render('journal.window.health.schedule.next.named', [['name', label], ['when', formatNextRun(nextSchedule.next_run)]])
        : window.JournalI18n.render('journal.window.health.schedule.next.unnamed', [['when', formatNextRun(nextSchedule.next_run)]]);
      el.style.color = '';
    }

    while (container.children.length > idx) {
      container.removeChild(container.lastChild);
    }
  }

  function updateLogsBadge() {
    if (state.deepLinkMode) return;
    let text = renderCount('journal.window.health.logs.lines.count', state.logTotalCount);
    if (state.lastLogTs) {
      if (window.JournalClock?.parts) {
        const p = window.JournalClock.parts(state.lastLogTs);
        if (p) text += ' · ' + p.hour + ':' + p.minute + ':' + p.second;
      } else {
        const d = new Date(state.lastLogTs);
        const hh = String(d.getHours()).padStart(2, '0');
        const mm = String(d.getMinutes()).padStart(2, '0');
        const ss = String(d.getSeconds()).padStart(2, '0');
        text += ' · ' + hh + ':' + mm + ':' + ss;
      }
    }
    elements.logsSummaryBadge.textContent = text;
    const titleEl = document.querySelector('.logs-title');
    if (titleEl && window.JournalClock?.placeNote) {
      window.JournalClock.placeNote(titleEl, { inside: false });
    }
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
    if (entry.type === 'agent') return window.JournalI18n.render('journal.window.health.status.didn.t.finish');
    if (entry.type === 'import') return window.JournalI18n.render('journal.window.health.status.didn.t.finish.importing');
    return window.JournalI18n.render('journal.window.health.window.explanation.ran.into.a.problem');
  }

  // The name an owner recognises for whatever produced the error.
  function recentErrorName(entry) {
    if (entry.type === 'agent') return talentName(entry.name);
    if (entry.type === 'import') return String(entry.name || window.JournalI18n.render('journal.window.health.status.import'));
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
      degraded.style.cssText = 'padding: 0.3em 0; font-size: 0.85em; color: var(--warn-ink);';
      degraded.textContent = window.JournalI18n.render('journal.window.health.visible.text.couldn.t.check.thinking.errors.today');
      container.appendChild(degraded);
    }

    if (state.recentErrorsFilter) {
      const filter = state.recentErrorsFilter;
      const label = document.createElement('div');
      label.style.cssText = 'padding: 0.2em 0 0.45em; font-size: 0.8em; color: var(--ink-soft); display: flex; gap: 0.5em; align-items: center;';
      const args = filter.day === 'today' ? [] : [['day', filter.day]];
      const scope = filter.day === 'today' ? 'today' : 'day';
      if (filter.talent) args.push(['talent', talentName(filter.talent)]);
      const id = `journal.window.health.errors.filter.${scope}${filter.talent ? '.talent' : ''}`;
      label.appendChild(document.createTextNode(window.JournalI18n.render(id, args)));
      const clearBtn = document.createElement('button');
      clearBtn.type = 'button';
      clearBtn.setAttribute('data-action', 'clear-error-filter');
      clearBtn.className = 'error-advice-link';
      clearBtn.textContent = window.JournalI18n.render('journal.window.health.visible.text.clear');
      label.appendChild(clearBtn);
      container.appendChild(label);
    }

    if (entries.length === 0) {
      const empty = document.createElement('div');
      empty.style.cssText = 'padding: 0.3em 0; font-size: 0.85em; color: var(--ink-soft);';
      empty.textContent = !state.agentErrorsOk
        ? window.JournalI18n.render('journal.window.health.visible.text.couldn.t.check.thinking.errors.today')
        : (state.recentErrorsFilter ? window.JournalI18n.render('journal.window.health.visible.text.no.matching.recent.errors.yet') : window.JournalI18n.render('journal.window.health.visible.text.no.recent.errors.variant2'));
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
        row.style.cssText = 'padding: 0.3em 0; font-size: 0.85em; color: var(--ink); border-bottom: 1px solid var(--hairline); display: flex; align-items: baseline; gap: 0.4em; flex-wrap: wrap;';

        const panelId = 'recent-error-panel-' + (++recentErrorPanelSeq);

        const summaryBtn = document.createElement('button');
        summaryBtn.type = 'button';
        summaryBtn.setAttribute('data-action', 'toggle-error');
        summaryBtn.setAttribute('aria-expanded', 'false');
        summaryBtn.setAttribute('aria-controls', panelId);
        summaryBtn.className = 'error-summary-btn';
        summaryBtn.style.cssText = 'flex: 1 1 auto; min-width: 0; text-align: left; background: none; border: none; margin: 0; font: inherit; color: inherit; cursor: pointer;';
        row.appendChild(summaryBtn);

        const actions = document.createElement('span');
        actions.style.cssText = 'margin-left: auto; display: flex; gap: 0.3em;';
        const viewBtn = document.createElement('button');
        viewBtn.setAttribute('data-action', 'view-logs');
        viewBtn.className = 'error-action-btn';
        viewBtn.textContent = window.JournalI18n.render('journal.window.health.visible.text.view.logs');
        actions.appendChild(viewBtn);
        const dismissBtn = document.createElement('button');
        dismissBtn.setAttribute('data-action', 'dismiss');
        dismissBtn.className = 'error-dismiss-btn';
        dismissBtn.textContent = window.JournalI18n.render('journal.window.health.visible.text.dismiss');
        actions.appendChild(dismissBtn);
        row.appendChild(actions);

        const panel = document.createElement('div');
        panel.id = panelId;
        panel.hidden = true;
        panel.setAttribute('data-error-panel', 'true');
        panel.style.cssText = 'flex-basis: 100%; width: 100%; padding: 0.35em 0 0.1em; color: var(--ink);';
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
      summaryBtn.appendChild(document.createTextNode(': ' + recentErrorOwnerPhrase(e) + ' '));
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
        tech.style.cssText = 'margin-top: 0.3em; font-size: 0.85em; color: var(--ink-soft);';
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
      advice.style.cssText = 'padding: 0.25em 0; font-size: 0.8em; color: var(--ink-faint);';
      const btn = document.createElement('button');
      btn.setAttribute('data-action', 'view-logs');
      btn.setAttribute('data-service', 'cortex');
      btn.className = 'error-advice-link';
      advice.append(...window.JournalI18n.renderMarkup('journal.window.health.errors.advice.thinking', [], {elements: { log_link: btn }}));
      container.appendChild(advice);
    }
    if (types.has('import')) {
      const importErr = recent.find(({ e }) => e.type === 'import')?.e;
      const stage = importErr?.stage && importErr.stage !== 'unknown' ? String(importErr.stage) : '';
      const advice = document.createElement('div');
      advice.style.cssText = 'padding: 0.25em 0; font-size: 0.8em; color: var(--ink-faint);';
      const btn = document.createElement('button');
      btn.setAttribute('data-action', 'view-logs');
      btn.setAttribute('data-service', 'importer');
      btn.className = 'error-advice-link';
      advice.append(...(stage ? window.JournalI18n.renderMarkup('journal.window.health.errors.advice.import.stage', [['stage', stage]], {elements: { log_link: btn }})
        : window.JournalI18n.renderMarkup('journal.window.health.errors.advice.import', [], {elements: { log_link: btn }})));
      container.appendChild(advice);
    }
    // Default fallback for any unrecognized error types (connection/service errors)
    if (Array.from(types).some(t => t !== 'agent' && t !== 'import')) {
      const advice = document.createElement('div');
      advice.style.cssText = 'padding: 0.25em 0; font-size: 0.8em; color: var(--ink-faint);';
      const btn = document.createElement('button');
      btn.setAttribute('data-action', 'view-logs');
      btn.setAttribute('data-service', 'supervisor');
      btn.className = 'error-advice-link';
      advice.append(...window.JournalI18n.renderMarkup('journal.window.health.errors.advice.service', [], {elements: { log_link: btn }}));
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
      elements.logsCollapseIndicator.textContent = window.JournalI18n.render('journal.window.health.visible.text.hide');
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
    btn.textContent = window.JournalI18n.render('journal.window.health.visible.text.retrying');
    window.apiJson('/app/health/api/retry-import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ import_id: importId })
    })
      .then(() => {
        btn.textContent = window.JournalI18n.render('journal.window.health.visible.text.retry.sent');
        btn.style.color = 'var(--ink-faint)';
        if (errorEl) {
          errorEl.textContent = '';
        }
      })
      .catch((err) => {
        window.logError(err, { context: 'health: retry-import failed' });
        btn.disabled = false;
        btn.textContent = window.JournalI18n.render('journal.window.health.visible.text.retry');
        btn.style.color = '';
        if (errorEl) {
          errorEl.textContent = err.serverMessage || window.JournalI18n.render('journal.window.health.visible.text.retry.failed');
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
    if (runningCount > 0) serviceParts.push(renderCount('journal.window.health.vitals.services.active', runningCount));
    if (retryingCount > 0) serviceParts.push(renderCount('journal.window.health.vitals.services.retrying', retryingCount));
    if (crashedCount > 0) serviceParts.push(renderCount('journal.window.health.vitals.services.attention', crashedCount));
    const services = state.supervisorSeen ? serviceParts.join(', ') || window.JournalI18n.render('journal.window.health.accessible.label.none') : pendingVitalLabel();
    sections[0]?.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.vitals.services.accessible', [['status', services]]));
    const thinking = state.cortexSeen ? renderCount('journal.window.health.vitals.thinking.running', state.agentCount) : pendingVitalLabel();
    sections[1]?.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.vitals.thinking.accessible', [['status', thinking]]));
    const tasks = state.supervisorSeen ? renderCount('journal.window.health.vitals.tasks.active', state.tasks.length) : pendingVitalLabel();
    sections[2]?.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.vitals.tasks.accessible', [['status', tasks]]));

    const staleCount = state.health?.stale_heartbeats?.length || 0;
    let healthLabel = timeoutFired ? window.JournalI18n.render('journal.window.health.status.unavailable') : window.JournalI18n.render('journal.window.health.status.loading');
    if (crashedCount > 0) {
      healthLabel = window.JournalI18n.render('journal.window.health.window.explanation.error.services.need.attention');
    } else if (staleCount > 0) {
      healthLabel = renderCount('journal.window.health.vitals.heartbeat.warning', staleCount);
    } else if (retryingCount > 0) {
      healthLabel = window.JournalI18n.render('journal.window.health.status.warning.services.retrying');
    } else if (state.health) {
      healthLabel = window.JournalI18n.render('journal.window.health.status.ok');
    }
    sections[3]?.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.vitals.health.accessible', [['status', healthLabel]]));

    const queueEntries = Object.entries(state.queues).filter(([, count]) => count > 0);
    const queues = queueEntries.map(([cmd, size]) => cmd + ' ' + size).join(', ')
      || (state.supervisorSeen ? window.JournalI18n.render('journal.window.health.accessible.label.none') : pendingVitalLabel());
    sections[4]?.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.vitals.queues.accessible', [['status', queues]]));

    const schedules = state.schedules.length ? renderCount('journal.window.health.vitals.schedules.count', state.schedules.length)
      : state.supervisorSeen ? window.JournalI18n.render('journal.window.health.accessible.label.none') : pendingVitalLabel();
    sections[5]?.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.vitals.schedules.accessible', [['status', schedules]]));
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
        const state = retrying ? 'retrying' : crashed ? 'crashed' : 'active';
        const service = serviceName(name);
        dot.className = retrying ? 'service-dot restarting' : (crashed ? 'service-dot crashed' : 'service-dot');
        dot.children[0].className = 'status-indicator ' + statusClass;
        dot.children[0].setAttribute('aria-label', window.JournalI18n.render(`journal.window.health.vitals.service.${state}`, [['service', service]]));
        dot.children[1].setAttribute('title', name);
        dot.children[1].textContent = retrying ? window.JournalI18n.render('journal.window.health.vitals.service.retrying.label', [['service', service]]) : service;
      }
    }

    if (!state.supervisorSeen && timeoutFired) {
      elements.serviceDots.textContent = window.JournalI18n.render('journal.window.health.visible.text.unavailable');
      elements.healthValue.textContent = window.JournalI18n.render('journal.window.health.visible.text.unavailable');
      updateVitalsStatus('unavailable');
    }

    // G3-304: the button acted on a read that had not landed. It comes alive
    // when the vitals do, or when the sweep gives up on them.


    // Agents count
    writeVital(elements.agentsValue, state.cortexSeen, renderCount('journal.window.health.vitals.thinking.running', state.agentCount));

    // Tasks
    const taskCount = state.tasks.length;
    writeVital(elements.tasksValue, state.supervisorSeen, renderCount('journal.window.health.vitals.tasks.active', taskCount));

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
      const mainSpan = hv.querySelector('.health-main');
      const staleListSpan = hv.querySelector('.stale-list');
      if (hasCrashed || staleCount === 0) hv.replaceChildren(mainSpan, staleListSpan);

      if (hasCrashed) {
        mainSpan.textContent = window.JournalI18n.render('journal.window.health.visible.text.services.need.attention');
        mainSpan.style.color = 'var(--danger)';
        staleListSpan.textContent = '';
        staleListSpan.style.display = 'none';
        updateVitalsStatus('error');
      } else if (staleCount > 0) {
        mainSpan.textContent = '';
        mainSpan.style.color = 'var(--warn-ink)';
        staleListSpan.textContent = '';
        staleListSpan.style.display = '';
        const parts = window.JournalI18n.renderMarkup('journal.window.health.vitals.heartbeat.stopped', [
          ['count', String(staleCount)], ['run_names', staleHeartbeats.map(String).join(', ')],
          ['quantity', window.JournalI18n.cardinalQuantity(staleCount)]
        ], {elements: {warning: mainSpan, runs: staleListSpan}});
        hv.replaceChildren(...parts);
        updateVitalsStatus('warning');
      } else if (hasRetrying) {
        mainSpan.textContent = window.JournalI18n.render('journal.window.health.visible.text.services.retrying');
        mainSpan.style.color = 'var(--warn-ink)';
        staleListSpan.textContent = '';
        staleListSpan.style.display = 'none';
        updateVitalsStatus('warning');
      } else {
        mainSpan.textContent = window.JournalI18n.render('journal.window.health.visible.text.ok');
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
      writeVital(elements.queuesValue, state.supervisorSeen, window.JournalI18n.render('journal.window.health.accessible.label.none'));
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
      const scheduled = schedules.length;
      const summaryArgs = [['count', String(scheduled)]];
      let summaryId;
      if (dueNow > 0) {
        summaryId = 'journal.window.health.schedule.summary.due';
        summaryArgs.push(['due', String(dueNow)], ['due_quantity', window.JournalI18n.cardinalQuantity(dueNow)],
          ['scheduled_quantity', window.JournalI18n.cardinalQuantity(scheduled)]);
      } else {
        summaryId = next ? 'journal.window.health.schedule.summary.next' : 'journal.window.health.schedule.summary.idle';
        summaryArgs.push(['quantity', window.JournalI18n.cardinalQuantity(scheduled)]);
        if (next) summaryArgs.push(['when', next]);
      }
      details.querySelector('summary').textContent = window.JournalI18n.render(summaryId, summaryArgs);
      details.querySelector('ul').innerHTML = schedules.map(schedule => {
        const name = schedule.name || window.JournalI18n.render('journal.window.health.status.unnamed');
        const label = scheduleName(name) || name.replace(/^maintenance:/, '').replace(/[:_-]+/g, ' ');
        const next = schedule.due ? window.JournalI18n.render('journal.window.health.status.due.now') : formatNextRun(schedule.next_run);
        return `<li>${escapeHtml(label)}${next ? ' · ' + escapeHtml(next) : ''}<code>${escapeHtml(name)}</code></li>`;
      }).join('');
    } else {
      writeVital(elements.schedulesValue, state.supervisorSeen, window.JournalI18n.render('journal.window.health.accessible.label.none'));
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
      indicator.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.accessible.label.services.all.running'));
      text.textContent = window.JournalI18n.render('journal.window.health.visible.text.all.services.running');
      severity.textContent = '';
    } else if (status === 'warning') {
      indicator.className = 'status-indicator restarting';
      indicator.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.accessible.label.services.warning'));
      text.textContent = window.JournalI18n.render('journal.window.health.visible.text.some.services.slow.to.respond');
      severity.textContent = window.JournalI18n.render('journal.window.health.visible.text.warning');
      el.classList.add('warning');
    } else if (status === 'error') {
      indicator.className = 'status-indicator crashed';
      indicator.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.accessible.label.services.need.attention'));
      text.textContent = window.JournalI18n.render('journal.window.health.visible.text.services.need.attention');
      severity.textContent = window.JournalI18n.render('journal.window.health.visible.text.error');
      el.classList.add('error');
    } else {
      indicator.className = 'status-indicator';
      indicator.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.accessible.label.services.status.unavailable'));
      text.textContent = window.JournalI18n.render('journal.window.health.visible.text.service.status.unavailable');
      severity.textContent = '';
      el.classList.add('unavailable');
    }
  }

  // What the observe card can honestly say when no stream is reporting live.
  // The registered-device list is a separate population from the observe
  // stream, so "unavailable" is only true when that list is unreadable too.
  function observeQuietState() {
    if (state.registeredClientsFailed) {
      return { badge: window.JournalI18n.render('journal.window.health.status.unavailable'), unavailable: true, heading: window.JournalI18n.render('journal.window.health.heading.device.activity.is.unavailable.right.now') };
    }
    if (!Array.isArray(state.registeredClients)) {
      return { badge: window.JournalI18n.render('journal.window.health.status.checking'), unavailable: false, heading: window.JournalI18n.render('journal.window.health.heading.checking.device.activity') };
    }
    if (state.registeredClients.length === 0) {
      return { badge: window.JournalI18n.render('journal.window.health.status.no.devices'), unavailable: false, heading: window.JournalI18n.render('journal.window.health.heading.no.devices.are.linked.to.your') };
    }
    const adding = state.registeredClients
      .filter(client => client.capture_state === 'active')
      .sort((a, b) => (Number.isFinite(a.capture_elapsed_ms) ? a.capture_elapsed_ms : Infinity)
        - (Number.isFinite(b.capture_elapsed_ms) ? b.capture_elapsed_ms : Infinity));
    if (adding.length > 0) {
      const age = registeredClientAge(adding[0]);
      const name = registeredClientName(adding[0]);
      return {
        badge: window.JournalI18n.render('journal.window.health.status.adding'),
        unavailable: false,
        heading: age
          ? window.JournalI18n.render('journal.window.health.device.capture.offline.last.added', [['device', String(name)], ['age', String(ageAgo(adding[0].capture_elapsed_ms))]])
          : window.JournalI18n.render('journal.window.health.device.capture.offline.adding', [['device', String(name)]]),
      };
    }
    if (state.registeredClients.some(client => client.capture_state === 'unknown')) {
      return { badge: window.JournalI18n.render('journal.window.health.status.unknown'), unavailable: true, heading: window.JournalI18n.render('journal.window.health.glance.glance.devices.delivery.from.some.of.your.devices') };
    }
    return { badge: window.JournalI18n.render('journal.window.health.status.quiet'), unavailable: false, heading: window.JournalI18n.render('journal.window.health.heading.no.device.is.reporting.live.activity') };
  }

  // Update observe mode badge
  function updateObserveMode(displayedClient = null) {
    if (!displayedClient && !Array.from(state.clients.values()).some(client => Date.now() - client.lastSeen < STALE_MS)) {
      elements.observeModeBadge.className = 'health-badge idle';
      elements.observeModeLabel.textContent = observeQuietState().badge;
      return;
    }

    const mode = (displayedClient || state.clients.get(state.localHost))?.mode;
    const badge = elements.observeModeBadge;
    const label = elements.observeModeLabel;

    if (mode === 'screencast') {
      badge.className = 'health-badge recording';
      label.textContent = window.JournalI18n.render('journal.window.health.visible.text.taking.in.your.screen');
    } else if (mode === 'tmux') {
      badge.className = 'health-badge tmux';
      label.textContent = window.JournalI18n.render('journal.window.health.visible.text.terminal.sessions');
    } else if (mode === 'idle') {
      badge.className = 'health-badge idle';
      label.textContent = window.JournalI18n.render('journal.window.health.visible.text.idle');
    } else {
      badge.className = 'health-badge idle';
      label.textContent = window.JournalI18n.render('journal.window.health.visible.text.idle');
    }
  }

  // Update observe card
  function updateObserve() {
    const now = Date.now();
    const freshClients = new Map(Array.from(state.clients.entries())
      .filter(([, client]) => now - client.lastSeen < STALE_MS));
    const quiet = freshClients.size === 0 ? observeQuietState() : null;
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

    const confirmedPrimary = state.localHost ? freshClients.get(state.localHost) : null;
    const fallbackEntry = Array.from(freshClients.entries())
      .filter(([stream]) => !stream.endsWith('.tmux'))
      .sort((a, b) => (b[1].lastSeen || 0) - (a[1].lastSeen || 0))[0] || null;
    const displayedStream = confirmedPrimary ? state.localHost : (fallbackEntry ? fallbackEntry[0] : null);
    const primary = confirmedPrimary || (fallbackEntry ? fallbackEntry[1] : null);
    const tmux = displayedStream ? freshClients.get(displayedStream + '.tmux') : null;
    const confirmedLocal = Boolean(state.localHost && confirmedPrimary && displayedStream === state.localHost);
    if (!confirmedLocal && displayedStream && state.hostCapabilities?.desktop_capture === true) {
      const label = window.JournalFormat.stream(displayedStream);
      const unknown = label === window.JournalFormat.stream('_default');
      if (unknown) {
        elements.observeSourceNote.textContent = state.localHost
          ? HEALTH_GLANCE_COPY.HEALTH_OBSERVE_LOCAL_SILENT
          : HEALTH_GLANCE_COPY.HEALTH_OBSERVE_UNKNOWN_SILENT;
      } else {
        const labelEl = document.createElement('span');
        labelEl.textContent = label;
        labelEl.title = displayedStream;
        const id = state.localHost ? 'journal.window.health.observe.local.reporting'
          : 'journal.window.health.observe.unknown.reporting';
        elements.observeSourceNote.replaceChildren(...window.JournalI18n.renderMarkup(id, [], {
          fixed: { stream: labelEl }
        }));
      }
      elements.observeSourceNote.classList.remove('hidden');
    } else {
      elements.observeSourceNote.textContent = '';
      elements.observeSourceNote.classList.add('hidden');
    }
    const channels = [
      {
        statusEl: elements.screencastStatus,
        detailEl: elements.screencastDetail,
        idleText: window.JournalI18n.render('journal.window.health.status.idle'),
        extract: () => {
          if (!primary?.screencast) return null;
          const recording = primary.screencast.recording;
          if (!recording) return { status: window.JournalI18n.render('journal.window.health.status.idle') };
          const streams = primary.screencast.streams || [];
          const elapsed = primary.screencast.window_elapsed_seconds || 0;
          const streamCount = streams.length;
          const mins = Math.max(1, Math.round(elapsed / 60));
          return {
            status: window.JournalI18n.render('journal.window.health.capture.screen.displays', [['count', String(streamCount)], ['minutes', String(mins)], ['quantity', window.JournalI18n.cardinalQuantity(streamCount)], ['minute_quantity', window.JournalI18n.cardinalQuantity(mins)]]),
            detail: streamCount > 0
              ? streams.map(s => `${s.position || 'unknown'} ${s.connector || 'unknown'}`).join(', ')
              : '',
          };
        },
      },
      {
        statusEl: elements.tmuxStatus,
        detailEl: elements.tmuxDetail,
        idleText: window.JournalI18n.render('journal.window.health.status.idle'),
        extract: () => {
          if (!tmux?.tmux) return null;
          if (!tmux.tmux.capturing) return { status: window.JournalI18n.render('journal.window.health.status.idle') };
          const captures = tmux.tmux.captures || 0;
          const sessions = tmux.tmux.sessions || [];
          const elapsed = tmux.tmux.window_elapsed_seconds || 0;
          const mins = Math.max(1, Math.round(elapsed / 60));
          return {
            status: window.JournalI18n.render('journal.window.health.capture.terminal.snapshots', [['count', String(captures)], ['minutes', String(mins)], ['quantity', window.JournalI18n.cardinalQuantity(captures)], ['minute_quantity', window.JournalI18n.cardinalQuantity(mins)]]),
            detail: sessions.length > 0 ? sessions.join(', ') : '',
          };
        },
      },
      {
        statusEl: elements.audioStatus,
        detailEl: elements.audioDetail,
        idleText: window.JournalI18n.render('journal.window.health.status.quiet'),
        extract: () => {
          if (!primary?.audio) return null;
          const hits = primary.audio.threshold_hits || 0;
          const id = primary.audio.will_save ? 'journal.window.health.capture.audio.sounds.saving'
            : 'journal.window.health.capture.audio.sounds';
          return {
            status: hits === 0 ? window.JournalI18n.render('journal.window.health.status.quiet')
              : window.JournalI18n.render(id, [['count', String(hits)], ['quantity', window.JournalI18n.cardinalQuantity(hits)]]),
          };
        },
      },
      {
        statusEl: elements.activityStatus,
        detailEl: elements.activityDetail,
        idleText: window.JournalI18n.render('journal.window.health.status.idle'),
        extract: () => {
          if (!primary?.activity) return null;
          const idleMs = primary.activity.idle_time_ms || 0;
          if (primary.activity.power_save) return { status: window.JournalI18n.render('journal.window.health.status.power.saving') };
          if (primary.activity.screen_locked) return { status: window.JournalI18n.render('journal.window.health.status.screen.locked') };
          if (primary.activity.sink_muted) return { status: window.JournalI18n.render('journal.window.health.status.audio.muted') };
          return { status: window.JournalI18n.render('journal.window.health.device.capture.idle', [['seconds', String(Math.floor(idleMs/1000))], ['seconds_quantity', window.JournalI18n.cardinalQuantity(Math.floor(idleMs/1000))]]) };
        },
      },
      {
        statusEl: elements.describeStatus,
        detailEl: elements.describeDetail,
        idleText: window.JournalI18n.render('journal.window.health.status.idle'),
        extract: () => {
          if (!primary?.describe) return null;
          return { processor: primary.describe };
        },
      },
      {
        statusEl: elements.transcribeStatus,
        detailEl: elements.transcribeDetail,
        idleText: window.JournalI18n.render('journal.window.health.status.idle'),
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
            ? window.JournalI18n.render('journal.window.health.device.processing.running.and.queued', [['running', String(running.length)], ['queued', String(queued)],
              ['running_quantity', window.JournalI18n.cardinalQuantity(running.length)], ['queue_quantity', window.JournalI18n.cardinalQuantity(queued)]])
            : window.JournalI18n.render('journal.window.health.device.processing.running.with.queue', [['queued', String(queued)], ['queue_quantity', window.JournalI18n.cardinalQuantity(queued)]]);
        } else if (isRunning) {
          ch.statusEl.textContent = running.length > 1 ? window.JournalI18n.render('journal.window.health.device.processing.running.count', [['running', String(running.length)], ['running_quantity', window.JournalI18n.cardinalQuantity(running.length)]]) : window.JournalI18n.render('journal.window.health.visible.text.running');
        } else if (queued > 0) {
          ch.statusEl.textContent = window.JournalI18n.render('journal.window.health.device.processing.queued', [['queued', String(queued)], ['queue_quantity', window.JournalI18n.cardinalQuantity(queued)]]);
        } else {
          ch.statusEl.textContent = ch.idleText;
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
      const registered = data.cid && (state.registeredClients || []).find(client => client.cid === data.cid);
      const reported = registered?.reported || data.reported;
      const named = registered?.display_label || reported?.name || data.host;
      const host = data.cid || data.host || stream;
      if (!byHost.has(host)) byHost.set(host, []);
      byHost.get(host).push({ stream, data, named, reported });
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
      nameEl.textContent = streams[0]?.named || window.JournalI18n.render('journal.window.health.visible.text.journal.source');
      nameEl.title = host;
      const statusIcon = document.createElement('span');
      statusIcon.className = 'status-indicator ' + (anyActive ? 'active' : 'stale');
      statusIcon.setAttribute('aria-label', anyActive ? window.JournalI18n.render('journal.window.health.accessible.label.active') : window.JournalI18n.render('journal.window.health.accessible.label.stale'));
      nameEl.prepend(statusIcon);
      nameEl.style.display = 'flex';
      nameEl.style.alignItems = 'center';
      nameEl.style.gap = '0.4em';
      card.appendChild(nameEl);

      const platform = streams[0]?.reported?.platform || streams[0]?.data?.platform || '';
      if (platform) {
        const platEl = document.createElement('div');
        platEl.className = 'client-host-platform';
        platEl.textContent = platform;
        card.appendChild(platEl);
      }

      for (const { stream, data, reported } of streams) {
        const row = document.createElement('div');
        row.className = 'client-stream-row';

        const stale = (now - data.lastSeen) >= STALE_MS;
        if (stale) row.classList.add('stale');

        const qualEl = document.createElement('div');
        qualEl.className = 'client-stream-qualifier';
        qualEl.textContent = window.JournalFormat.stream(stream);
        qualEl.title = stream;
        row.appendChild(qualEl);

        const modeEl = document.createElement('div');
        modeEl.className = 'client-stream-mode';
        modeEl.textContent = data.mode || '—';
        row.appendChild(modeEl);

        if (stale) {
          const badgeEl = document.createElement('span');
          badgeEl.className = 'stale-badge';
          badgeEl.setAttribute('aria-label', window.JournalI18n.render('journal.window.health.accessible.label.stale.not.responding'));
          badgeEl.textContent = window.JournalI18n.render('journal.window.health.visible.text.stale');
          row.appendChild(badgeEl);
        }

        card.appendChild(row);
      }

      elements.clientsGrid.appendChild(card);
    }
  }

	  // A "since …" sentence needs the day itself, absolute and lowercase (F-15).
	  // It lives in JournalFormat now; health.js and status_pane.js each carried
	  // their own copy of the same twelve months and the same year rule.
	  const sinceDay = ms => window.JournalFormat.sinceDay(ms);

	  function registeredClientMeta(client) {
	    const lastCapture = client.last_accepted_ingest_at && Date.parse(client.last_accepted_ingest_at);
	    if (Number.isFinite(lastCapture)) {
	      const deltaMs = Date.now() - lastCapture;
	      if (deltaMs < 0) return window.JournalI18n.render('journal.window.health.window.explanation.last.added.from.future');
	      return window.JournalI18n.render('journal.window.health.device.activity.last.added', [['age', String(ageAgo(deltaMs))]]);
	    }
	    const lastSeen = client.last_seen_at && Date.parse(client.last_seen_at);
	    if (Number.isFinite(lastSeen)) {
	      const seenDelta = Date.now() - lastSeen;
	      if (seenDelta < 0) return window.JournalI18n.render('journal.window.health.window.explanation.last.seen.from.future');
	      return window.JournalI18n.render('journal.window.health.device.activity.last.reported', [['age', String(ageAgo(seenDelta))]]);
	    }
	    // G3-208: a device that has never reported still has an age the owner can
	    // judge staleness by — the day it was paired. Blank was the least useful
	    // answer on a row whose whole question is "can I get rid of this?".
	    const paired = client.paired_at && Date.parse(client.paired_at);
	    if (Number.isFinite(paired)) {
	      const pairedDelta = Date.now() - paired;
	      if (pairedDelta < 0) return window.JournalI18n.render('journal.window.health.status.never.reported');
	      return window.JournalI18n.render('journal.window.health.device.activity.paired', [['age', String(ageAgo(pairedDelta))]]);
	    }
	    return window.JournalI18n.render('journal.window.health.status.never.reported');
	  }

	  async function requestBacklogReprocess(button) {
	    let row = button.closest('.backlog-row');
	    if (!row) return;
	    const day = button.dataset.day;
	    const flavor = button.dataset.flavor;
	    const findButton = () => Array.from(document.querySelectorAll('.backlog-action')).find(el => el.dataset.day === day && el.dataset.flavor === flavor);
	    if (flavor === 'from-scratch' && !(await window.SolQuestion.ask({ message: button.dataset.confirm, action: button.textContent, opener: findButton }))) {
	      return;
	    }
	    row = findButton()?.closest('.backlog-row');
	    if (!row) return;
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
	        if (result && (result.status === 'already_complete' || result.status === 'current_degraded' || result.status === 'held_by_backoff')) {
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
	          statusEl.textContent = err && err.serverMessage ? err.serverMessage : window.JournalI18n.render('journal.window.health.visible.text.try.again');
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

  // The devices card ships hidden and only appears once there are devices to
  // list, which is after the browser has already tried to honour the hash. The
  // first time it appears we take the owner there ourselves, once: a later
  // refresh must not pull the page out from under them.
  let devicesDeepLinkScrolled = false;

  function renderIngestRejection(row, rejection, sourceName, key, openKeys) {
    const detail = document.createElement('span');
    detail.className = 'registered-client-detail';
    const count = typeof rejection?.active_count === 'number' && Number.isFinite(rejection.active_count)
      ? rejection.active_count : null;
    const firstMs = typeof rejection?.first === 'string' ? Date.parse(rejection.first) : NaN;
    const args = [];
    let id = 'journal.window.health.device.upload.' + (count === null ? 'unknown' : 'count');
    if (sourceName !== null) {
      id += '.source';
      args.push(['source', sourceName || window.JournalI18n.render('journal.window.health.visible.text.default')]);
    }
    if (Number.isFinite(firstMs)) {
      id += '.since';
      args.push(['day', sinceDay(firstMs)]);
    }
    if (count !== null) args.push(['count', String(count)], ['quantity', window.JournalI18n.cardinalQuantity(count)]);
    detail.textContent = window.JournalI18n.render(id, args);
    row.appendChild(detail);
    if (rejection?.reason_code) {
      const tech = document.createElement('details');
      tech.className = 'registered-client-tech';
      tech.dataset.rejectionKey = key;
      tech.open = openKeys.has(key);
      const summary = document.createElement('summary');
      summary.textContent = sourceName === null ? window.JournalI18n.render('journal.window.health.visible.text.technical.details') : window.JournalI18n.render('journal.window.health.device.source.technical.details', [['source', String(sourceName || window.JournalI18n.render('journal.window.health.status.default'))]]);
      tech.appendChild(summary);
      const code = document.createElement('div');
      code.textContent = window.JournalI18n.render('journal.window.health.device.upload.reason.code', [['code', String(rejection.reason_code)]]);
      tech.appendChild(code);
      row.appendChild(tech);
    }
  }

  function renderRegisteredClients(clients) {
    elements.registeredClientsCard.classList.remove('hidden');
    if (!devicesDeepLinkScrolled && window.location.hash === '#registeredClientsCard') {
      devicesDeepLinkScrolled = true;
      const card = elements.registeredClientsCard;
      requestAnimationFrame(() => card.scrollIntoView({ block: 'start' }));
    }
    const openKeys = new Set(Array.from(elements.registeredClientsStrip.querySelectorAll('.registered-client-tech'))
      .filter(detail => detail.open).map(detail => detail.dataset.rejectionKey));
    const focusedKey = document.activeElement?.closest('.registered-client-tech')?.dataset.rejectionKey;
    const focusedClient = document.activeElement?.closest('.registered-client-row')?.dataset.cid;
    elements.registeredClientsStrip.innerHTML = '';

    const list = Array.isArray(clients) ? clients : [];

    const isAttention = client => {
      const capture = client.capture_state;
      return capture === 'degraded' || capture === 'stale' || capture === 'offline' || capture === 'unknown'
        || registeredClientNeedsAttention(client)
        || Boolean(client.clock_skew);
    };

    const attentionClients = list.filter(isAttention);

    if (state.registeredClientsFailed) {
      const notice = document.createElement('p');
      notice.className = 'registered-client-recovery';
      notice.dataset.readState = 'unavailable';
      let id = 'journal.window.health.device.delivery.unavailable';
      const args = [];
      if (state.registeredClientsObservedAt !== null) {
        id += '.checked';
        args.push(['checked_at', new Date(state.registeredClientsObservedAt).toLocaleString()]);
      }
      if (attentionClients.length) id += '.rows';
      notice.textContent = window.JournalI18n.render(id, args);
      elements.registeredClientsStrip.appendChild(notice);
    } else if (list.length === 0) {
      const emptyP = document.createElement('p');
      emptyP.className = 'registered-clients-empty';
      emptyP.textContent = HEALTH_GLANCE_COPY.HEALTH_DEVICES_EMPTY;
      elements.registeredClientsStrip.appendChild(emptyP);

      const pairLink = document.createElement('a');
      pairLink.className = 'registered-clients-manage';
      pairLink.href = '/app/network/#devices';
      pairLink.textContent = HEALTH_GLANCE_COPY.HEALTH_DEVICES_PAIR_LINK;
      elements.registeredClientsStrip.appendChild(pairLink);
      return;
    } else {
      const activeCount = list.filter(c => c.capture_state === 'active').length;
      if (activeCount > 0) {
        const countP = document.createElement('p');
        countP.className = 'registered-clients-count';
        countP.textContent = renderCount('journal.window.health.glance.devices.count.many', activeCount);
        elements.registeredClientsStrip.appendChild(countP);
      }
    }

    for (const client of attentionClients) {
      const needsAttention = registeredClientNeedsAttention(client);
      let stateClass = ['connected', 'stale', 'disconnected'].includes(client.state)
        ? client.state
        : 'disconnected';
      let labelText = client.capture_state || client.label || 'unknown';
      if (client.capture_state === 'active') stateClass = 'connected';
      if (client.capture_state === 'stale') stateClass = 'stale';
      if (client.capture_state === 'offline') stateClass = 'disconnected';
      if (needsAttention) {
        stateClass = 'failing';
        labelText = window.JournalI18n.render('journal.window.health.status.uploads.rejected');
      } else if (client.capture_state === 'unknown') {
        labelText = window.JournalI18n.render('journal.window.health.status.delivery.unknown');
      }
      if (state.registeredClientsFailed) {
        stateClass = 'disconnected';
        labelText = window.JournalI18n.render('journal.window.health.device.delivery.last.known', [['status', labelText]]);
      }
      const row = document.createElement('div');
      row.className = 'registered-client-row';
      row.dataset.cid = client.cid;

      const nameEl = document.createElement('span');
      nameEl.className = 'registered-client-name';
      nameEl.textContent = client.display_label || client.device_label || client.cid_short || client.cid || window.JournalI18n.render('journal.window.health.visible.text.unnamed.device');
      row.appendChild(nameEl);

      const labelEl = document.createElement('span');
      labelEl.className = `registered-client-label ${stateClass}`;
      labelEl.textContent = labelText;
      row.appendChild(labelEl);

      if (needsAttention) {
        const sources = rejectedClientSources(client);
        if (sources.length) {
          for (const [source, delivery] of sources) {
            renderIngestRejection(row, delivery.ingest_rejection, source,
              JSON.stringify([client.cid, source]), openKeys);
          }
        } else if (client.ingest_rejection) {
          renderIngestRejection(row, client.ingest_rejection, null,
            JSON.stringify([client.cid, null]), openKeys);
        }
      }

      const metaText = registeredClientMeta(client);
      if (metaText && metaText !== labelText) {
        const metaEl = document.createElement('span');
        metaEl.className = 'registered-client-meta';
        metaEl.textContent = metaText;
        row.appendChild(metaEl);
      }

      if (client.clock_skew) {
        const skewEl = document.createElement('span');
        skewEl.className = 'registered-client-skew';
        skewEl.textContent = window.JournalI18n.render('journal.window.health.visible.text.clock.skew');
        row.appendChild(skewEl);
      }

      const manage = document.createElement('a');
      manage.className = 'registered-clients-manage';
      manage.href = '/app/network/#devices';
      manage.textContent = HEALTH_GLANCE_COPY.HEALTH_DEVICES_MANAGE_LINK;
      row.appendChild(manage);

      elements.registeredClientsStrip.appendChild(row);
    }

    if (list.length > 0) {
      const seeDevices = document.createElement('a');
      seeDevices.className = 'registered-clients-manage';
      seeDevices.href = '/app/network/#devices';
      seeDevices.textContent = HEALTH_GLANCE_COPY.HEALTH_DEVICES_SEE_LINK;
      elements.registeredClientsStrip.appendChild(seeDevices);
    }

    if (focusedKey) {
      const detail = Array.from(elements.registeredClientsStrip.querySelectorAll('.registered-client-tech'))
        .find(item => item.dataset.rejectionKey === focusedKey);
      detail?.querySelector('summary')?.focus({ preventScroll: true });
    } else if (focusedClient) {
      const row = Array.from(elements.registeredClientsStrip.querySelectorAll('.registered-client-row'))
        .find(item => item.dataset.cid === focusedClient);
      row?.querySelector('.registered-clients-manage')?.focus({ preventScroll: true });
    }
  }

  async function loadRegisteredClients() {
    try {
      const response = await fetch('/app/network/api/clients');
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const payload = await response.json();
      if (!Array.isArray(payload.clients)) throw new Error('Invalid device list');
      state.registeredClientsFailed = false;
      state.registeredClients = payload.clients;
      state.registeredClientsObservedAt = Date.now();
      renderRegisteredClients(state.registeredClients);
      updateObserve();
      updateClients();
      document.dispatchEvent(new CustomEvent('health:devices-loaded'));
    } catch (err) {
      state.registeredClientsFailed = true;
      renderRegisteredClients(state.registeredClients);
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
        idSummary.textContent = window.JournalI18n.render('journal.window.health.visible.text.run.id');
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
      const stateLabel = agent.event === 'thinking' ? window.JournalI18n.render('journal.window.health.status.working') :
                        (agent.event === 'tool_start' || agent.event === 'tool_end') ? window.JournalI18n.render('journal.window.health.status.working') : window.JournalI18n.render('journal.window.health.status.running.variant2');
      const elapsed = runningFor(Number(agent.elapsed_seconds) * 1000);
      card.children[0].querySelector('.activity-card-id-value').textContent = '…' + getAgentId(agent.use_id);
      card.children[1].textContent = talentName(agent.name) || window.JournalI18n.render('journal.window.health.visible.text.default');
      card.children[2].textContent = stateLabel;
      card.children[3].textContent = elapsed;
      card.children[4].textContent = agent.provider || window.JournalI18n.render('journal.window.health.visible.text.unknown');
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
        retryBtn.textContent = window.JournalI18n.render('journal.window.health.visible.text.retry');
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
      const elapsed = runningFor(Number(imp.elapsed_ms));
      const humanStage = imp.stage === 'initialization' ? window.JournalI18n.render('journal.window.health.status.starting') :
                         imp.stage === 'transcribing' ? window.JournalI18n.render('journal.window.health.status.transcribing.audio') :
                         imp.stage === 'segmenting' ? window.JournalI18n.render('journal.window.health.status.organizing.segments') : window.JournalI18n.render('journal.window.health.status.processing');

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

      card.children[0].textContent = '…' + getAgentId(imp.import_id);
      card.children[1].textContent = truncate(imp.input_file || window.JournalI18n.render('journal.window.health.status.import'), 20);
      card.children[2].textContent = (isError ? '! ' : '') + humanStage;
      card.children[2].setAttribute('data-internal-stage', imp.stage || 'processing');
      card.children[5].children[0].style.width = progress + '%';
      card.children[6].textContent = elapsed;
      card.children[7].textContent = imp.file_type || window.JournalI18n.render('journal.window.health.visible.text.unknown');
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
      { label: window.JournalI18n.render('journal.window.health.status.mode'), value: d.mode || null },
      { label: window.JournalI18n.render('journal.window.health.status.day'), value: d.day || null },
      { label: window.JournalI18n.render('journal.window.health.status.facet'), value: d.facet || null },
      { label: window.JournalI18n.render('journal.window.health.status.segment'), value: d.segment || null },
    ]);

    // Progress bars
    const progressItems = [];
    if (d.agents_total > 0) {
      progressItems.push({ label: window.JournalI18n.render('journal.window.health.thinking.check.progress.runs', [['completed', String(d.agents_completed || 0)], ['total', String(d.agents_total)]]), pct: Math.round((d.agents_completed || 0) / d.agents_total * 100) });
    }
    if (d.segments_total > 0) {
      progressItems.push({ label: window.JournalI18n.render('journal.window.health.thinking.check.progress.segments', [['completed', String(d.segments_completed || 0)], ['total', String(d.segments_total)]]), pct: Math.round((d.segments_completed || 0) / d.segments_total * 100) });
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
      elements.thinkAgents.textContent = window.JournalI18n.render('journal.window.health.thinking.check.running', [['agents', d.current_agents.join(', ')]]);
    } else {
      elements.thinkAgents.textContent = '';
    }

    updateAllQuiet();
    updateStatusSummary();
  }

  const LOG_BUFFER_SIZE = 50;

  // Browser-side classification for streamed diagnostic lines.
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
    if (window.JournalClock?.parts) {
      const p = window.JournalClock.parts(ts);
      if (p) return p.hour + ':' + p.minute + ':' + p.second;
    }
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
      const lines = records.length;
      const errors = countErrorRecords(records);
      header.textContent = window.JournalI18n.render('journal.window.health.logs.service.collapsed', [
        ['service', serviceName(service)], ['lines', String(lines)], ['errors', String(errors)],
        ['line_quantity', window.JournalI18n.cardinalQuantity(lines)],
        ['error_quantity', window.JournalI18n.cardinalQuantity(errors)]
      ]);
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
      elements.logErrorBadge.textContent = renderCount('journal.window.health.logs.errors.count', state.logErrorCount);
      elements.logErrorBadge.classList.remove('hidden');
      if (state.logsCollapsed) {
        state.logsCollapsed = false;
        document.querySelector('.logs-card').classList.remove('logs-collapsed');
        elements.logsCollapseIndicator.textContent = window.JournalI18n.render('journal.window.health.visible.text.hide');
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
      state.queuedWorkAuthoritySeen = true;
      invalidateRealtime();
      return;
    }
    if (msg.event === 'status') {
      state.queuedWorkAuthoritySeen = true;
      state.supervisorSeen = true;
      state.supervisorStatusAt = Date.now();
      state.heldWorkSnapshot = {
        queues: msg.queues || {}, held: msg.held || [], queueHold: msg.queue_hold || null,
        observedAt: state.supervisorStatusAt,
      };
      state.heldWorkCurrent = true;
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
    // A stream first met on the socket re-reads the label table once, then
    // renders again with what it learned.
    if (window.JournalFormat.needsStreamLabel(msg.stream)) {
      window.JournalFormat.noteLiveStream(msg.stream).then(() => {
        updateObserve();
        updateClients();
      });
    }
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
  // A phone's late blur as a filter's menu closes goes back to the filter.
  elements.logServiceFilter.addEventListener('change', () => {
    renderLogs();
    window.SolFocus.keepAfterPick(elements.logServiceFilter);
  });
  elements.logLevelFilter.addEventListener('change', () => {
    state.logLevelFilter = elements.logLevelFilter.value;
    renderLogs();
    window.SolFocus.keepAfterPick(elements.logLevelFilter);
  });
  elements.logStreamFilter.addEventListener('change', () => {
    renderLogs();
    window.SolFocus.keepAfterPick(elements.logStreamFilter);
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
    elements.logsCollapseIndicator.textContent = state.logsCollapsed ? window.JournalI18n.render('journal.window.health.visible.text.show') : window.JournalI18n.render('journal.window.health.visible.text.hide');
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
    elements.logServiceFilter.innerHTML = `<option value="all">${window.JournalI18n.renderHtml('journal.window.health.option.label.all.services')}</option>`;
    updateLogsBadge();
    renderLogs();
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
    elements.logsCollapseIndicator.textContent = window.JournalI18n.render('journal.window.health.visible.text.hide');
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
    logsTitle.textContent = window.JournalI18n.render('journal.window.health.visible.text.log.file');
    logsControls.innerHTML = `<button id="logBackBtn">${window.JournalI18n.renderHtml('journal.window.health.button.label.back.to.dashboard')}</button>`;
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
    appendDeepLinkLogLine(viewport, window.JournalI18n.render('journal.window.health.status.loading.variant2'), 'stdout');

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
        appendDeepLinkLogLine(viewport, window.JournalI18n.render('journal.window.health.window.explanation.network.error.loading.log.file'), 'stderr', window.JournalI18n.render('journal.window.health.window.explanation.error.network.error.loading.log.file'));
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
    state.heldWorkCurrent = false;
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
          let streamAttempted = false;
          window.appEvents.onConnectionState?.(({ connected, state: streamState }) => {
            if (connected) {
              streamAttempted = true;
              armSkeletonTimeout();
              return;
            }
            invalidateRealtime();
            if (streamState === 'connecting') {
              streamAttempted = true;
            } else if (streamAttempted) {
              // The stream opened or tried to and then failed: nothing pending
              // is still on its way.
              sweepUnresolvedSkeletons();
            }
          });
    } else {
      armSkeletonTimeout();
    }
	  }

	  function initHealth() {
	    if (healthInitialized) return;
	    healthInitialized = true;
	    applyHealthCopy();
    // Do not hold the first Health paint or its existing readiness signal on
    // this independent read. Later reads share the existing visible-tab cycle.
    const backupReady = new Promise(resolve => requestAnimationFrame(() => resolve(loadBackupStatus())));
    requestAnimationFrame(() => loadSpeechSetup());
    requestAnimationFrame(() => loadLocalPrograms());
    window.addEventListener('hashchange', focusBackupDetails);
    const initialReady = Promise.all([healthInfoReady, registeredClientsReady, loadHealthState()]);
    initialReady.then(() => {
      document.dispatchEvent(new CustomEvent('health:initial-ready'));
    });
    Promise.all([initialReady, backupReady]).then(() => requestAnimationFrame(focusBackupDetails));
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
      el.textContent = window.JournalI18n.render('journal.window.health.visible.text.waiting.for.live.updates');
      el.className = 'connection-indicator';
      elements.logsConnectionNote.textContent = window.JournalI18n.render('journal.window.health.visible.text.log.updates.may.be.delayed');
      elements.logsConnectionNote.classList.remove('hidden');
      return;
    }
    const ageMs = Date.now() - state.lastEventTs;
    const ago = Math.floor(ageMs / 1000);
    if (ago >= 60) {
      el.textContent = window.JournalI18n.render('journal.window.health.vitals.connection.quiet.since', [['duration', String(relativeTime(ageMs))]]);
      el.className = 'connection-indicator disconnected';
      elements.logsConnectionNote.textContent = window.JournalI18n.render('journal.window.health.visible.text.log.updates.may.be.delayed');
      elements.logsConnectionNote.classList.remove('hidden');
    } else if (ago >= 30) {
      el.textContent = window.JournalI18n.render('journal.window.health.visible.text.updates.are.slow');
      el.className = 'connection-indicator stale';
      elements.logsConnectionNote.textContent = window.JournalI18n.render('journal.window.health.visible.text.log.updates.may.be.delayed');
      elements.logsConnectionNote.classList.remove('hidden');
    } else {
      el.textContent = window.JournalI18n.render('journal.window.health.vitals.connection.updated', [['age', String(ageAgo(ageMs))]]);
      el.className = 'connection-indicator';
      elements.logsConnectionNote.textContent = '';
      elements.logsConnectionNote.classList.add('hidden');
    }
  }

  // Sweep stale agents and imports every 60s
  let staleSweepTimer = setInterval(sweepStale, 60000);

  let registeredClientsTimer = setInterval(refreshStatusReads, 60000);

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
      refreshStatusReads();
      registeredClientsTimer = setInterval(refreshStatusReads, 60000);
      connectionHealthTimer = setInterval(updateConnectionHealth, 5000);
    }
  });
})();
