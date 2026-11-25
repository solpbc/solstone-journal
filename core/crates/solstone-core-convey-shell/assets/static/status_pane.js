// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Navigation status and inline Health details
window.whenShellReady(() => {
  const statusIcon = document.querySelector('#status-instrument .status-icon');
  const statusPane = document.getElementById('status-pane');
  const statusConsoleLink = document.getElementById('status-pane-console-link');
  const liveRegion = document.getElementById('status-live-region');
  let statusPaneOpen = false;
  let lastCaptureStatusForPane = null;
  let _lastHistoryLen = -1;

  // G3-204: the status map's silent-device row is a fixed singular label, and
  // health's banner counts the devices from the rollup. One screen was stating
  // the same fact twice with two different numbers. Both now read the count
  // from the one rollup this pane already fetches.
  const SILENT_DEVICE_MESSAGE = 'journal.window.shared.websocket.label.a.device.hasn.t.added.recently';
  let silentDeviceCount = 0;
  function statusLabelText() {
    const label = window.appEvents?.statusLabel || window.JournalI18n.render('journal.window.shared.websocket.label.connecting');
    if (window.appEvents?.statusLabelMessage === SILENT_DEVICE_MESSAGE && silentDeviceCount > 1) {
      return window.JournalI18n.render('journal.window.shell.status.silent.devices', [['count', String(silentDeviceCount)]]);
    }
    return label;
  }

  function countSilentDevices(capture) {
    return (capture && Array.isArray(capture.clients) ? capture.clients : [])
      .filter(client => client.status === 'offline' || client.status === 'stale')
      .length;
  }

  let latestServerBadges = null;
  let badgesFetchFailed = false;
  let badgesRefreshTimer = null;

  function syncHomeBadge() {
    if (!window.solShellChrome?.setBadge) return;
    const unviewed = window.AppServices?.quietNotifs?.unviewedCount?.() || 0;
    const serverIssues = Number.isFinite(latestServerBadges?.home?.issues)
      ? Math.max(0, Math.floor(latestServerBadges.home.issues))
      : 0;
    const serverSeverity = latestServerBadges?.home?.severity || null;
    const totalIssues = serverIssues + unviewed;
    const wsState = window.appEvents?.getMetrics?.()?.state || 'connecting';
    if (totalIssues > 0) {
      window.solShellChrome.setBadge('home', {
        count: totalIssues,
        tone: serverSeverity === 'red' ? 'danger' : 'warn'
      });
      return;
    }
    const uncertain = wsState === 'disconnected'
      || badgesFetchFailed
      || serverSeverity === 'amber'
      || lastCaptureStatusForPane === 'unknown';
    window.solShellChrome.setBadge('home', uncertain ? { count: 0, tone: 'warn', marker: '!' } : null);
  }

  function fetchShellBadges() {
    const reqOpts = window.solWindowRequest
      ? window.solWindowRequest('/api/shell/badges', { credentials: 'same-origin', cache: 'no-store' })
      : { credentials: 'same-origin', cache: 'no-store' };
    fetch('/api/shell/badges', reqOpts)
      .then(r => r.ok ? r.json() : null)
      .catch(() => null)
      .then(data => {
        if (data && typeof data === 'object') {
          latestServerBadges = data;
          badgesFetchFailed = false;
          syncHomeBadge();
          const curationCount = Number.isFinite(data.curation) ? Math.max(0, Math.floor(data.curation)) : 0;
          window.solShellChrome?.setBadge?.('curation', curationCount > 0 ? { count: curationCount, tone: 'neutral' } : null);
          const importCount = Number.isFinite(data.import) ? Math.max(0, Math.floor(data.import)) : 0;
          window.solShellChrome?.setBadge?.('import', importCount > 0 ? { count: importCount, tone: 'review' } : null);
        } else {
          badgesFetchFailed = true;
          syncHomeBadge();
        }
      });
  }

  function scheduleBadgesRefresh() {
    if (badgesRefreshTimer !== null) return;
    badgesRefreshTimer = window.setTimeout(() => {
      badgesRefreshTimer = null;
      fetchShellBadges();
    }, 1500);
  }

  // Shared label updater — called from here and from websocket.js
  window.updateStatusLabel = function() {
    const label = statusLabelText();
    if (statusIcon) {
      statusIcon.setAttribute('aria-label', label);
      statusIcon.setAttribute('title', label);
      const unread = window.AppServices?.quietNotifs?.unviewedCount?.() || 0;
      statusIcon.href = lastCaptureStatusForPane === 'degraded'
        ? '/app/health/#registeredClientsCard'
        : unread ? '/app/health/#quiet-notifs-section'
        : ['offline', 'stale'].includes(lastCaptureStatusForPane)
          ? '/app/health/#registeredClientsCard' : '/app/health/#healthSystemDetails';
      const visibleLabel = document.querySelector('#status-instrument .status-label');
      if (visibleLabel) visibleLabel.textContent = label;
    }
    if (liveRegion && liveRegion.textContent !== label) liveRegion.textContent = label;
    syncHomeBadge();
  };

  function updateDiagnosticConsoleLink() {
    if (!statusConsoleLink) return;
    const base = window.CONVEY_COPY.CONSOLE_LINK_LABEL;
    const unread = window.convey?.diagnosticConsole?.unreadCount?.() || 0;
    const count = [['quantity', window.JournalI18n.cardinalQuantity(unread)], ['count', String(unread)]];
    statusConsoleLink.textContent = unread > 0 ? window.JournalI18n.render('journal.window.shared.console.link.message.count', count) : base;
    statusConsoleLink.setAttribute('aria-label', unread > 0 ? window.JournalI18n.render('journal.window.shared.console.link.unread.count', count) : base);
  }

  let initialDestinationPending = true;
  let initialHealthReady = false;

  // X-10: the pane is the last thing on health, so `block: start` has nothing
  // left to scroll into and the browser clamps at the end of the page — the
  // deep link landed the section below the fold. A gutter gives the scroller
  // the room the destination needs and nothing more. It is measured, not
  // guessed, and it is removed the moment the page grows enough on its own.
  const SCROLL_GUTTER_ID = 'deep-link-scroll-gutter';
  function ensureScrollEndGutter(target) {
    const existing = document.getElementById(SCROLL_GUTTER_ID);
    if (existing) existing.style.height = '0px';
    const scroller = document.scrollingElement || document.documentElement;
    const top = target.getBoundingClientRect().top + scroller.scrollTop;
    const scrollMarginTop = parseFloat(window.getComputedStyle(target).scrollMarginTop) || 0;
    const needed = Math.ceil(top - scrollMarginTop + window.innerHeight - scroller.scrollHeight);
    if (needed <= 0) {
      existing?.remove();
      return;
    }
    const gutter = existing || document.createElement('div');
    if (!existing) {
      gutter.id = SCROLL_GUTTER_ID;
      gutter.setAttribute('aria-hidden', 'true');
      document.body.appendChild(gutter);
    }
    gutter.style.height = needed + 'px';
  }

  function revealInitialDestination() {
    if (!statusPaneOpen || !initialDestinationPending || !initialHealthReady) return;
    const id = window.location.hash.slice(1);
    if (!['healthSystemDetails', 'quiet-notifs-section', 'registeredClientsCard'].includes(id)) return;
    const target = document.getElementById(id);
    if (!target || target.getClientRects().length === 0) return;
    initialDestinationPending = false;
    requestAnimationFrame(() => {
      ensureScrollEndGutter(target);
      target.scrollIntoView({block: 'start'});
    });
  }
  // Initial API renders can change the page height after the workspace mounts.
  document.addEventListener('health:initial-ready', () => {
    initialHealthReady = true;
    revealInitialDestination();
    // G3-207: health's cards keep growing the page after that first scroll,
    // which left the destination three quarters down the fold. One more pass
    // once the page has settled, and only if the owner has not scrolled since,
    // because a refresh must never pull the page out from under them.
    const landedAt = window.scrollY;
    setTimeout(() => {
      if (window.scrollY !== landedAt) return;
      initialDestinationPending = true;
      revealInitialDestination();
    }, 700);
  });

  statusConsoleLink?.addEventListener('click', (event) => {
    event.preventDefault();
    window.convey?.diagnosticConsole?.open?.();
    window.convey?.diagnosticConsole?.markAllRead?.();
    updateDiagnosticConsoleLink();
  });

  // Update status pane metrics
  let paneTickRunning = false;

  function startPaneTick() {
    if (paneTickRunning) return;
    paneTickRunning = true;
    if (window.AppServices?.visibleInterval) {
      window.AppServices.visibleInterval(updateStatusPane, 1000);
    } else {
      window.setInterval(updateStatusPane, 1000);
    }
  }

  function updateStatusPane() {
    window.renderStatusMark?.();
    updateDiagnosticConsoleLink();
    if (!window.appEvents) return;
    if (!statusPaneOpen) return;

    const metrics = window.appEvents.getMetrics();
    const statusSentence = document.getElementById('status-sentence');
    const statusDetail = document.getElementById('status-detail');
    const wsStatusRaw = document.getElementById('ws-status-raw');
    const wsUptimeRaw = document.getElementById('ws-uptime-raw');
    const wsLastMessageRaw = document.getElementById('ws-last-message-raw');

    if (statusSentence) {
      // Headline answers the same question the mark answers (is your journal
      // receiving your life right now?). Transport health stays in technical
      // details below — naming that axis, rather than claiming "all systems
      // connected" under an offline mark.
      const markLabel = window.appEvents?.statusLabel;
      if (markLabel) {
        statusSentence.textContent = statusLabelText();
        statusSentence.style.color = '';
      } else if (metrics.state === 'connecting') {
        statusSentence.textContent = window.CONVEY_COPY.CONNECTION_CONNECTING;
        statusSentence.style.color = '';
      } else {
        statusSentence.textContent = window.CONVEY_COPY.CONNECTION_RECONNECTING;
        statusSentence.style.color = '';
      }
    }

    if (statusDetail) {
      if (metrics.state === 'connected') {
        // G3-104: the pane speaks the shared relativeTime ladder, so the shell
        // and Health never describe the same instant differently. Under a
        // minute reads as "just now" rather than a vague "a few seconds".
        const MINUTE = 60000;
        const uptimeText = metrics.uptimeMs < MINUTE
          ? window.JournalI18n.render('journal.window.shared.status.pane.connection.just.now')
          : relativeTime(metrics.uptimeMs, 'journal.window.shared.status.pane.connected.elapsed');
        if (metrics.lastMessageMs !== null) {
          const lastUpdate = metrics.lastMessageMs < MINUTE
            ? window.JournalI18n.render('journal.window.shared.status.pane.update.just.now')
            : relativeTime(metrics.lastMessageMs, 'journal.window.shared.status.pane.updated.elapsed');
          statusDetail.textContent = `${uptimeText} · ${lastUpdate}`;
        } else {
          statusDetail.textContent = uptimeText;
        }
      } else if (metrics.state === 'connecting') {
        statusDetail.textContent = '';
      } else {
        statusDetail.textContent = window.CONVEY_COPY.CONNECTION_DELAYED;
      }
    }

    if (wsStatusRaw) {
      wsStatusRaw.textContent = metrics.state;
    }

    if (wsUptimeRaw) {
      if (metrics.connected) {
        // G3-104: the ladder's floor said as a length of time. "24s" and
        // "0 seconds ago" were the same defect on two lines of one disclosure.
        wsUptimeRaw.textContent = metrics.uptimeMs < 60000
          ? window.JournalI18n.render('journal.window.shared.status.pane.visible.text.under.a.minute')
          : relativeTime(metrics.uptimeMs);
      } else {
        wsUptimeRaw.textContent = '-';
      }
    }

    if (wsLastMessageRaw) {
      if (metrics.lastMessageMs !== null) {
        wsLastMessageRaw.textContent = metrics.lastMessageMs < 60000
          ? window.JournalI18n.render('journal.window.shared.status.pane.visible.text.just.now')
          : relativeTime(metrics.lastMessageMs, 'journal.window.shared.status.pane.age.elapsed');
      } else if (metrics.connected) {
        wsLastMessageRaw.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.no.messages.yet');
      } else {
        wsLastMessageRaw.textContent = '-';
      }
    }

    renderQuietNotifs();
    syncNotificationBlocks();
    const quietSection = document.getElementById('quiet-notifs-section');
    const quietBounds = quietSection?.getBoundingClientRect();
    if (document.visibilityState !== 'hidden' && quietBounds?.height > 0
        && quietBounds.bottom > 0 && quietBounds.top < window.innerHeight
        && window.AppServices?.quietNotifs?.unviewedCount() > 0) {
      window.AppServices.quietNotifs.markViewed();
    }
    // Update notification history
    updateNotificationHistory();
    updateBellState();
  }

  function fetchSystemStatus() {
    fetch('/api/system/status')
      .then(r => r.ok ? r.json() : null)
      .catch(() => null)
      .then(data => {
        if (data) {
          lastCaptureStatusForPane = data.capture?.status ?? 'unknown';
          silentDeviceCount = countSilentDevices(data.capture);
          window.updateStatusLabel?.();
          window.appEvents?.setCaptureStatus?.(data.capture?.status ?? 'unknown');
          if (statusPaneOpen) {
            renderCaptureSection(data.capture);
            renderVersionSection(data.version);
            renderSecuritySection(data.security);
            renderAutomaticUpdate(data.version);
          }
        } else {
          lastCaptureStatusForPane = 'unknown';
          silentDeviceCount = 0;
          window.appEvents?.setCaptureStatus?.('unknown');
          syncHomeBadge();
        }
      });
  }

  // X-02: the pane printed its own lowercase 'sep 6'. Dates on a surface go
  // through the shared formatter, the way the rest of the shell reads them.
  // A "since …" sentence needs the day itself, absolute and lowercase (F-15).
  // It lives in JournalFormat now; status_pane.js and health.js each carried
  // their own copy of the same twelve months and the same year rule.
  const sinceDay = ms => window.JournalFormat.sinceDay(ms);

  // Plural forms are real: the pane read '1 uploads turned away'.
  function uploadsTurnedAway(count) {
    return window.JournalI18n.render('journal.window.shared.status.pane.capture.rejected.upload.count', [['quantity', window.JournalI18n.cardinalQuantity(count)], ['count', String(count)]]);
  }

	  function renderCaptureSection(capture) {
	    const section = document.getElementById('capture-status-section');
	    const text = document.getElementById('capture-status-text');
    if (!section || !text) return;

    section.style.display = '';
	    const status = capture?.status;
		    if (status === 'active') {
		      text.style.color = 'var(--success-ink)';
		    } else if (status === 'degraded') {
		      text.textContent = '';
		      text.style.color = '';
		      const degraded = (capture.clients || []).filter(o => o.status === 'degraded' && o.ingest_rejection);
		      const appendLine = (value, cssText) => {
		        const line = document.createElement('div');
		        line.textContent = value;
		        if (cssText) line.style.cssText = cssText;
		        text.appendChild(line);
		        return line;
		      };
		      const appendHealthLink = () => {
		        const link = document.createElement('a');
		        link.href = '/app/health/';
		        link.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.view.device.status');
		        link.style.cssText = 'display: inline-block; margin-top: 4px; color: var(--danger); font-size: 12px;';
		        text.appendChild(link);
		      };

		      if (!degraded.length) {
		        appendLine(window.JournalI18n.render('journal.window.shared.status.pane.status.text.a.device.needs.attention'), 'color: var(--danger); font-weight: 600;');
		        appendLine(window.JournalI18n.render('journal.window.shared.status.pane.status.text.a.device.isn.t.reaching.your'), 'color: var(--danger); font-size: 12px;');
		        appendHealthLink();
		        return;
		      }

		      const first = degraded[0];
		      const rej = first.ingest_rejection;
		      const name = (first.name || '').trim();
		      const title = name ? window.JournalI18n.render('journal.window.shared.status.pane.capture.device.attention', [['device', String(name)]]) : window.JournalI18n.render('journal.window.shared.status.pane.window.explanation.a.device.needs.attention');
		      const firstMs = typeof rej.first === 'string' ? Date.parse(rej.first) : NaN;
		      const latestMs = typeof rej.latest === 'string' ? Date.parse(rej.latest) : NaN;
		      const hasFirstTs = Number.isFinite(firstMs);
		      const hasActiveCount = typeof rej.active_count === 'number' && isFinite(rej.active_count);
		      appendLine(title, 'color: var(--danger); font-weight: 600;');

		      // X-02: health's sentence is the source for this state. The pane
		      // echoes it, says what the device adds rather than what it senses,
		      // and counts in a real plural.
		      let consequence;
		      if (hasFirstTs && hasActiveCount) {
			        consequence = window.JournalI18n.render('journal.window.shared.status.pane.capture.last.reached.day', [['since_day', sinceDay(firstMs)]]) + ' ' + uploadsTurnedAway(rej.active_count);
		      } else if (hasActiveCount) {
			        consequence = window.JournalI18n.render('journal.window.shared.status.pane.window.explanation.what.it.adds.isn.t.reaching') + ' ' + uploadsTurnedAway(rej.active_count);
		      } else if (hasFirstTs) {
			        consequence = window.JournalI18n.render('journal.window.shared.status.pane.capture.last.reached.day', [['since_day', sinceDay(firstMs)]]);
		      } else {
			        consequence = window.JournalI18n.render('journal.window.shared.status.pane.window.explanation.what.it.adds.isn.t.reaching');
		      }
		      appendLine(consequence, 'color: var(--danger); font-size: 12px;');

		      appendLine(window.JournalI18n.render('journal.window.shared.status.pane.status.text.update.or.restart.it.on.that'), 'color: var(--ink-soft); font-size: 12px;');

		      const parts = [];
		      if (rej.reason_code) parts.push('reason: ' + rej.reason_code);
		      if (Number.isFinite(latestMs)) {
		        parts.push(relativeTime(Date.now() - latestMs, 'journal.window.shared.status.pane.rejected.elapsed'));
		      }
		      if (parts.length) {
		        appendLine(parts.join(' · '), 'color: var(--ink-soft); font-size: 11px;');
		      }

		      appendHealthLink();
		      if (degraded.length > 1) {
		        appendLine(window.JournalI18n.render('journal.window.shared.status.pane.capture.more.need.attention', [['quantity', window.JournalI18n.cardinalQuantity(degraded.length - 1)], ['count', String(degraded.length - 1)]]), 'color: var(--ink-soft); font-size: 12px; margin-top: 2px;');
		      }
		      return;
	    } else if (status === 'offline' || status === 'stale') {
      text.style.color = 'var(--ink-soft)';
    } else {
      text.style.color = 'var(--ink-faint)';
    }
    if (status === 'no_clients') {
      text.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.no.devices.are.running.the.solstone');
    } else if (status === 'offline' || status === 'stale') {
      text.textContent = silentDeviceSentence(capture);
    } else if (status === 'active') {
      text.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.devices.are.adding.to.your.journal');
    } else {
      text.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.device.status.is.unavailable.right.now');
    }
  }

  // Name the device and how long it has been quiet. "a device" gave the owner
  // nothing to act on, and the health glance now carries the same rollup.
  function silentDeviceSentence(capture) {
    const named = (capture && capture.clients ? capture.clients : [])
      .filter(client => client.status === 'offline' || client.status === 'stale')
      .map(client => ({
        name: String(client.name || '').trim(),
        ageMs: captureAgeMs(client.last_accepted_ingest_at)
      }))
      .filter(client => client.name)
      .sort((a, b) => (b.ageMs === null ? -1 : b.ageMs) - (a.ageMs === null ? -1 : a.ageMs));
    if (named.length === 0) {
      return window.JournalI18n.render('journal.window.shared.status.pane.window.explanation.a.device.hasn.t.added.to');
    }
    if (named.length === 1) {
      const only = named[0];
      return only.ageMs === null
        ? window.JournalI18n.render('journal.window.home.health.quiet.one.unknown', [['device', only.name]])
        : relativeTime(only.ageMs, 'journal.window.home.health.quiet.elapsed', [['device', only.name]]);
    }
    const devices = named.map(client => client.ageMs === null ? client.name
      : window.JournalI18n.render('journal.window.home.health.device.age', [
        ['device', client.name], ['age', relativeTime(client.ageMs)]
      ]));
    return window.JournalI18n.render('journal.window.home.health.quiet.devices', [
      ['quantity', window.JournalI18n.cardinalQuantity(named.length)],
      ['count', String(named.length)], ['devices', devices.join(', ')]
    ]);
  }

  function captureAgeMs(timestamp) {
    const parsed = timestamp ? Date.parse(timestamp) : NaN;
    if (!Number.isFinite(parsed)) return null;
    const age = Date.now() - parsed;
    return age < 0 ? null : age;
  }

  function renderVersionSection(version) {
    const section = document.getElementById('version-section');
    const text = document.getElementById('version-text');
    if (!section || !text) return;

    section.style.display = '';
    const isAvailable = version?.availability === 'available' || (!version?.availability && Boolean(version?.update_available));
    if (!version?.availability && version?.update_available === false) {
      text.textContent = 'v' + (version?.current || window.JournalI18n.render('journal.window.shared.status.pane.visible.text.unknown'));
      text.style.color = 'var(--ink-faint)';
    } else if (isAvailable) {
      const differ = Boolean(
        version?.platform_version &&
        version?.latest_platform_version &&
        version.platform_version !== version.latest_platform_version
      );
      const targetVer = differ ? version.latest_platform_version : (version?.latest || '?');
      const span = document.createElement('span');
      span.style.color = 'var(--warn-ink)';
      text.replaceChildren(...window.JournalI18n.renderMarkup('journal.window.shared.status.pane.version.update.available', [
        ['current_version', String(version?.current || '?')], ['available_version', String(targetVer)]
      ], { elements: { notice: span } }));
      const how = automaticSuffix(version, true);
      if (how) text.appendChild(document.createTextNode(' · ' + how));
      text.style.color = '';
    } else if (version?.availability === 'installed') {
      text.textContent = window.JournalI18n.render('journal.window.shared.status.pane.version.update.installed', [
        ['current_version', String(version?.current || '?')],
        ['installed_version', String(version?.target?.journal_version || '?')]
      ]);
      text.style.color = '';
    } else if (version?.availability === 'not_available') {
      const how = automaticSuffix(version, false);
      const status = version?.current
        ? window.JournalI18n.render('journal.window.shared.status.pane.version.current.up.to.date', [['current_version', String(version.current)]])
        : window.JournalI18n.render('journal.window.shared.status.pane.version.unknown.up.to.date');
      text.textContent = how ? `${status} · ${how}` : status;
      text.style.color = '';
    } else {
      text.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.update.status.unknown');
      text.style.color = '';
    }
  }

  // How an update gets here, said after the version when the journal updates itself.
  function automaticSuffix(version, available) {
    const automatic = version?.automatic;
    if (automatic?.choice !== 'on' || automatic.paused) return '';
    if (!available) return window.JournalI18n.render('journal.window.shared.status.pane.automatic.updates');
    if (automatic.last_result === 'metered') return window.JournalI18n.render('journal.window.shared.status.pane.window.explanation.installs.on.a.connection.that.isn');
    if (automatic.last_result === 'held') return window.JournalI18n.render('journal.window.shared.status.pane.automatic.held');
    // Nothing is promised while the release names no installer yet, or while
    // the hold can't be read.
    if (automatic.last_result === 'waiting-for-signed-installer' || automatic.last_result === 'hold-unknown') return '';
    return window.JournalI18n.render('journal.window.shared.status.pane.automatic.installs');
  }

  // A failed press says why under the button's own section; a pressed button
  // otherwise just shows the new state.
  function postSystem(path, body, box) {
    return fetch(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body || {})
    })
      .then(r => r.ok ? null : r.json().then(reply => (typeof reply?.error === 'string' ? reply.error : ''), () => ''))
      .catch(() => '')
      .then(failed => {
        if (failed === null) {
          fetchSystemStatus();
          return;
        }
        if (box && failed) {
          // One reason at a time: pressing again replaces it.
          box.querySelector('[data-press-failure]')?.remove();
          const line = document.createElement('div');
          line.dataset.pressFailure = '';
          line.textContent = failed;
          line.style.color = 'var(--warn-ink)';
          box.appendChild(line);
        }
      });
  }

  function actionButton(label, onPress) {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = label;
    button.style.marginRight = '8px';
    button.addEventListener('click', function () {
      button.disabled = true;
      onPress();
    });
    return button;
  }

  // Automatic updates: the one-time question, a pause after two tries that
  // didn't work, and where a package install gets its updates.
  function renderAutomaticUpdate(version) {
    const box = document.getElementById('automatic-update-text');
    if (!box) return;
    box.textContent = '';
    const automatic = version?.automatic;
    const line = text => {
      const div = document.createElement('div');
      div.textContent = text;
      box.appendChild(div);
      return div;
    };
    if (version?.package) {
      if (version.package.repository) {
        line(window.JournalI18n.render('journal.window.shared.status.pane.window.explanation.installed.from.a.package.updates.come'));
      } else {
        const div = line('');
        const link = document.createElement('a');
        link.href = 'https://solstone.app/install#linux-packages';
        link.target = '_blank';
        link.rel = 'noreferrer';
        link.referrerPolicy = 'no-referrer';
        link.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.solstone.app.install.linux.packages');
        div.append(...window.JournalI18n.renderMarkup('journal.window.shared.status.pane.version.package.repository.link', [], { fixed: { repository_link: link } }));
      }
    } else if (automatic?.eligible && automatic.choice === 'unasked') {
      const ask = line(window.JournalI18n.render('journal.window.shared.status.pane.window.explanation.update.your.journal.automatically'));
      ask.style.fontWeight = '600';
      line(automatic.with_app
        ? window.JournalI18n.render('journal.window.shared.status.pane.window.explanation.once.a.day.your.journal.and')
        : window.JournalI18n.render('journal.window.shared.status.pane.window.explanation.once.a.day.your.journal.checks'));
      const buttons = document.createElement('div');
      buttons.style.marginTop = '4px';
      buttons.appendChild(actionButton(window.JournalI18n.render('journal.window.shared.status.pane.automatic.enable'), () => postSystem('/api/system/updates/automatic', { on: true }, box)));
      buttons.appendChild(actionButton(window.JournalI18n.render('journal.window.shared.status.pane.automatic.decline'), () => postSystem('/api/system/updates/automatic', { on: false }, box)));
      box.appendChild(buttons);
    } else if (automatic?.choice === 'on' && automatic.paused) {
      line(window.JournalI18n.render('journal.window.shared.status.pane.automatic.paused')).style.color = 'var(--warn-ink)';
      const buttons = document.createElement('div');
      buttons.style.marginTop = '4px';
      buttons.appendChild(actionButton(window.JournalI18n.render('journal.window.shared.status.pane.automatic.retry'), () => postSystem('/api/system/updates/retry', {}, box)));
      box.appendChild(buttons);
    }
    box.style.display = box.childNodes.length ? '' : 'none';
  }

  // What the security advisory check found. Nothing shows while it checks
  // or when nothing is behind; an advisory it couldn't confirm says so.
  function renderSecuritySection(security) {
    const text = document.getElementById('security-text');
    if (!text) return;
    const flags = Array.isArray(security?.flags) ? security.flags : [];
    text.textContent = '';
    text.style.color = '';
    if (flags.length) {
      text.style.color = 'var(--warn-ink)';
      flags.forEach(function (flag) {
        const line = document.createElement('div');
        if (flag?.restart) {
          line.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.a.security.update.is.installed.restart');
          text.appendChild(line);
          const buttons = document.createElement('div');
          buttons.style.marginTop = '4px';
          buttons.appendChild(actionButton(window.JournalI18n.render('journal.window.shared.status.pane.security.restart'), () => postSystem('/api/system/restart', {}, text)));
          text.appendChild(buttons);
          return;
        }
        const args = [['fixed_version', String(flag?.fixed_in || '?')]];
        let messageId;
        if (flag?.subject === 'journal') {
          messageId = 'journal.window.shared.status.pane.security.journal.update.version';
        } else if (flag?.device) {
          messageId = 'journal.window.shared.status.pane.security.device.update.version';
          args.push(['device_name', String(flag.device)]);
        } else {
          messageId = 'journal.window.shared.status.pane.security.unnamed.device.update.version';
        }
        line.textContent = window.JournalI18n.render(messageId, args);
        text.appendChild(line);
      });
    } else if (security?.state === 'unconfirmed') {
      text.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.couldn.t.check.for.security.updates');
      text.style.color = 'var(--ink-faint)';
    }
    text.style.display = text.textContent ? '' : 'none';
  }

	  function renderQuietNotifs() {
    const section = document.getElementById('quiet-notifs-section');
    const list = document.getElementById('quiet-notifs-list');
    if (!section || !list) return;

    const notifs = window.AppServices?.quietNotifs?.getAll() || [];
    // The settings notifications row links straight here, so the section stays
    // on the page with an honest empty state rather than collapsing to 0x0 and
    // landing the owner at the top of health (G3-107).
    section.style.display = '';

    if (notifs.length === 0) {
      list.textContent = '';
      const empty = document.createElement('span');
      empty.style.color = 'var(--ink-faint-paper)';
      empty.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.no.notifications.held.back');
      list.appendChild(empty);
      return;
    }

    const newIds = new Set(notifs.map(n => String(n.id)));
    const existingById = new Map();
    for (const child of Array.from(list.children)) {
      const k = child.getAttribute('data-id');
      if (k) existingById.set(k, child);
    }
    // drop stale rows AND any non-keyed node (e.g. a prior "no quiet notifications" span)
    for (const child of Array.from(list.children)) {
      const k = child.getAttribute('data-id');
      if (!k || !newIds.has(k)) list.removeChild(child);
    }

    for (const n of notifs) {
      const key = String(n.id);
      let row = existingById.get(key);
      let btn, panel;
      if (!row) {
        row = document.createElement('div');
        row.setAttribute('data-id', key);
        row.style.cssText = 'padding: 4px 0;';

        const panelId = 'quiet-notif-panel-' + key;
        btn = document.createElement('button');
        btn.type = 'button';
        btn.setAttribute('data-action', 'toggle-quiet-notif');
        btn.setAttribute('aria-expanded', 'false');
        btn.setAttribute('aria-controls', panelId);
        btn.style.cssText = 'display: flex; align-items: center; gap: 8px; width: 100%; text-align: left; background: none; border: none; padding: 0; margin: 0; font: inherit; color: inherit; cursor: pointer;';
        row.appendChild(btn);

        panel = document.createElement('div');
        panel.id = panelId;
        panel.hidden = true;
        panel.setAttribute('data-quiet-notif-panel', 'true');
        panel.style.cssText = 'padding: 4px 0 2px; color: var(--danger); font-size: 13px; white-space: pre-wrap; word-break: break-word;';
        row.appendChild(panel);
      } else {
        btn = row.querySelector('[data-action="toggle-quiet-notif"]');
        panel = row.querySelector('[data-quiet-notif-panel]');
      }

      const relativeAge = window.AppServices.notifications._getRelativeTime(n.ts);
      btn.textContent = '';
      const ageSpan = document.createElement('span');
      ageSpan.style.cssText = 'color: var(--ink-faint); font-size: 11px; flex-shrink: 0;';
      ageSpan.textContent = relativeAge;
      btn.appendChild(ageSpan);

      const srcCode = document.createElement('code');
      srcCode.style.cssText = 'flex-shrink: 0; font-size: 11px;';
      srcCode.textContent = n.source || '';
      btn.appendChild(srcCode);

      const snippet = document.createElement('span');
      snippet.style.cssText = 'color: var(--danger); font-size: 13px; flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;';
      snippet.textContent = n.message || '';
      btn.appendChild(snippet);

      const hint = document.createElement('span');
      hint.setAttribute('data-quiet-notif-hint', 'true');
      hint.style.cssText = 'flex-shrink: 0; color: var(--ink-faint); font-size: 11px;';
      hint.textContent = btn.getAttribute('aria-expanded') === 'true' ? window.JournalI18n.render('journal.window.shared.status.pane.visible.text.hide.details') : window.JournalI18n.render('journal.window.shared.status.pane.visible.text.show.details');
      btn.appendChild(hint);

      panel.textContent = n.message || '';

      list.appendChild(row);
    }
  }

  const quietNotifsList = document.getElementById('quiet-notifs-list');
  if (quietNotifsList) {
    quietNotifsList.addEventListener('click', (e) => {
      const btn = e.target.closest('[data-action="toggle-quiet-notif"]');
      if (!btn) return;
      const expanded = btn.getAttribute('aria-expanded') === 'true';
      btn.setAttribute('aria-expanded', expanded ? 'false' : 'true');
      const panel = document.getElementById(btn.getAttribute('aria-controls'));
      if (panel) panel.hidden = expanded;
      const hint = btn.querySelector('[data-quiet-notif-hint]');
      if (hint) hint.textContent = expanded ? window.JournalI18n.render('journal.window.shared.status.pane.visible.text.show.details') : window.JournalI18n.render('journal.window.shared.status.pane.visible.text.hide.details');
    });
  }

  // G3-206: two kickers, two explanatory lines and two empty states for one
  // subject said nothing, twice, at the foot of the trust page. When neither
  // list holds anything the block is one kicker and one line; the split returns
  // the moment either has content. The bell moves with the kicker so the one
  // control here is never lost.
  function syncNotificationBlocks() {
    const quietLabel = document.getElementById('quiet-notifs-label');
    const quietHeading = document.getElementById('quiet-notifs-heading');
    const quietExplainer = document.getElementById('quiet-notifs-explainer');
    const quietList = document.getElementById('quiet-notifs-list');
    const recentHeading = document.getElementById('recent-notifs-heading');
    const history = document.getElementById('notification-history');
    if (!quietLabel || !quietHeading || !quietList || !recentHeading || !history) return;

    const quietCount = window.AppServices?.quietNotifs?.getAll?.()?.length || 0;
    const recentCount = window.AppServices?.notifications?.getHistory?.()?.length || 0;
    const bothEmpty = quietCount === 0 && recentCount === 0;

    quietLabel.textContent = bothEmpty ? window.JournalI18n.render('journal.window.shared.status.pane.visible.text.notifications') : window.JournalI18n.render('journal.window.shared.status.pane.visible.text.quiet.notifications');
    if (quietExplainer) quietExplainer.hidden = bothEmpty;
    recentHeading.hidden = bothEmpty;
    history.hidden = bothEmpty;

    const bell = document.getElementById('notif-bell');
    const bellHome = bothEmpty ? quietHeading : recentHeading;
    if (bell && bell.parentElement !== bellHome) bellHome.appendChild(bell);

    if (bothEmpty) {
      quietList.textContent = '';
      const line = document.createElement('span');
      line.style.color = 'var(--ink-faint-paper)';
      line.textContent = window.JournalI18n.render('journal.window.shared.status.pane.visible.text.nothing.held.back.nothing.new');
      quietList.appendChild(line);
    }
  }

  function updateNotificationHistory() {
    const container = document.getElementById('notification-history');
    if (!container || !window.AppServices?.notifications) return;

    const history = window.AppServices.notifications.getHistory();
    if (!statusPaneOpen && history.length === _lastHistoryLen) return;
    _lastHistoryLen = history.length;
    const escape = window.AppServices.escapeHtml;
    const resolveIcon = value => window.AppServices.notifications._resolveIcon(value);

    if (history.length === 0) {
      container.innerHTML = `<span style="color: var(--ink-faint-paper);">${window.JournalI18n.renderHtml('journal.window.shared.status.pane.visible.text.no.notifications.yet')}</span>`;
      return;
    }

    container.innerHTML = history.map(n => {
      const relativeAge = window.AppServices.notifications._getRelativeTime(n.timestamp);

      const action = window.AppServices.sameOriginPath(n.action);
      if (action) {
        return `<a href="${escape(action)}" class="status-pane-history-item" style="display: flex; align-items: center; gap: 8px; padding: 6px 8px; margin: 0 -8px; border-radius: 4px; text-decoration: none; color: inherit;">
          <span class="icon-slot" style="font-size: 16px; flex-shrink: 0;" aria-hidden="true">${resolveIcon(n.icon)}</span>
          <span style="font-weight: 500; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${escape(n.title)}</span>
          <span style="color: var(--ink-faint); font-size: 11px; flex-shrink: 0;">${escape(relativeAge)}</span>
        </a>`;
      } else {
        return `<div style="display: flex; align-items: center; gap: 8px; padding: 6px 8px; margin: 0 -8px;">
          <span class="icon-slot" style="font-size: 16px; flex-shrink: 0;" aria-hidden="true">${resolveIcon(n.icon)}</span>
          <span style="font-weight: 500; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${escape(n.title)}</span>
          <span style="color: var(--ink-faint); font-size: 11px; flex-shrink: 0;">${escape(relativeAge)}</span>
        </div>`;
      }
    }).join('');
  }

  function updateBellState() {
    const bell = document.getElementById('notif-bell');
    if (!bell || !('Notification' in window)) {
      if (bell) bell.style.display = 'none';
      return;
    }
    const perm = Notification.permission;
    bell.setAttribute('data-perm', perm);
    if (perm === 'granted') {
      bell.innerHTML = `<span aria-hidden="true">${(window.ConveyIcons?.svg('bell') || '')}</span>`;
      bell.title = window.JournalI18n.render('journal.window.shared.status.pane.tooltip.browser.notifications.enabled');
      bell.setAttribute('aria-label', window.JournalI18n.render('journal.window.shared.status.pane.accessible.label.browser.notifications.enabled'));
    } else if (perm === 'denied') {
      bell.innerHTML = `<span aria-hidden="true">${(window.ConveyIcons?.svg('bell-off') || '')}</span>`;
      bell.title = window.JournalI18n.render('journal.window.shared.status.pane.tooltip.notifications.blocked.update.in.browser.settings');
      bell.setAttribute('aria-label', window.JournalI18n.render('journal.window.shared.status.pane.accessible.label.notifications.blocked.update.in.browser.settings'));
    } else {
      bell.innerHTML = `<span aria-hidden="true">${(window.ConveyIcons?.svg('bell') || '')}</span>`;
      bell.title = window.JournalI18n.render('journal.window.shared.status.pane.tooltip.enable.browser.notifications');
      bell.setAttribute('aria-label', window.JournalI18n.render('journal.window.shared.status.pane.accessible.label.enable.browser.notifications'));
    }
  }

  const bellEl = document.getElementById('notif-bell');
  if (bellEl) {
    bellEl.addEventListener('click', async (e) => {
      e.stopPropagation();
      if (!('Notification' in window)) return;
      if (Notification.permission === 'default') {
        await AppServices.requestNotificationPermission();
        updateBellState();
      }
    });
  }

  // Health's first mount can happen before this file arrives. Adopt that
  // mount now, and each one after it.
  function onWorkspaceMounted() {
    const host = document.getElementById('healthSystemDetails');
    statusPaneOpen = Boolean(host);
    if (statusPaneOpen) startPaneTick();
    if (host && statusPane) {
      host.appendChild(statusPane);
      statusPane.hidden = false;
      renderQuietNotifs();
      syncNotificationBlocks();
      updateStatusPane();
      fetchSystemStatus();
      revealInitialDestination();
    }
  }
  if (window.bindWorkspaceMount) window.bindWorkspaceMount(onWorkspaceMounted);
  else document.addEventListener('workspace:mounted', onWorkspaceMounted);

  fetchSystemStatus();
  // Defer the first badge fetch until after the current frame paints.
  if (typeof window.requestAnimationFrame === 'function') {
    window.requestAnimationFrame(() => window.setTimeout(fetchShellBadges, 0));
  } else {
    window.setTimeout(fetchShellBadges, 0);
  }
  // The capture rollup, the quiet-device count and the version come from this
  // read only. It waits while the page is hidden and reads again on return.
  const every = window.AppServices?.visibleInterval
    ? (fn, ms) => window.AppServices.visibleInterval(fn, ms)
    : (fn, ms) => window.setInterval(fn, ms);
  every(fetchSystemStatus, 60000);
  every(fetchShellBadges, 60000);
  if (window.appEvents?.listen) {
    for (const tract of ['change', 'importer', 'supervisor', 'owner_notice', 'observe', 'cortex']) {
      window.appEvents.listen(tract, scheduleBadgesRefresh);
    }
  }
  window.appEvents?.onConnectionState?.(() => {
    syncHomeBadge();
    scheduleBadgesRefresh();
  });
  // The pane's clock ticks only while the pane is on the page and in view;
  // the mark redraws itself on every change in connection or capture.
  if (statusPaneOpen) startPaneTick();
  window.addEventListener('diagnostic-console-updated', updateDiagnosticConsoleLink);
  // Initial update
  setTimeout(updateStatusPane, 100);
});
