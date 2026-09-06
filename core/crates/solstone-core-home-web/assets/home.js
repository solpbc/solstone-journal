// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const PULSE_URL = '/app/home/api/pulse';
  const BRIEFING_URL = '/app/home/api/briefing';
  const SECTION_STATE_KEY = 'pulse-section-state';
  const SECTION_IDS = ['pulse-narrative', 'pulse-today', 'pulse-needs'];
  const SECTION_DEFAULTS = {
    'pulse-narrative': 'true',
    'pulse-today': 'true',
    'pulse-needs': 'false'
  };
  const briefingSectionOrder = ['your_day', 'yesterday', 'forward_look', 'reading'];
  const briefingRenderOrder = ['your_day', 'yesterday', 'needs_attention', 'forward_look', 'reading'];
  const briefingSectionLabels = {
    your_day: 'your day',
    yesterday: 'yesterday',
    needs_attention: 'needs attention',
    forward_look: 'forward look',
    reading: 'reading'
  };

  let homeInitialized = false;
  let interactionsWired = false;
  let realtimeWired = false;
  let root = null;
  let surface = null;
  let lastPulse = null;
  let briefingSections = {};

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
        serverMessage: error?.serverMessage || error?.message || '',
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
    renderBriefingSections(briefingSections);
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
        serverMessage: error?.serverMessage || error?.message || '',
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
    surface.innerHTML = surfaceLoading('loading pulse...');
    apiJson(PULSE_URL)
      .then(function (data) {
        renderPulse(data);
      })
      .catch(function (error) {
        logError(error, 'home: loadPulse failed');
        surface.innerHTML = surfaceError(error, "Couldn't load pulse");
        bindSurfaceRetry();
      });
  }

  function renderPulse(data) {
    validatePulse(data);
    lastPulse = data;
    briefingSections = isPlainObject(data.briefing_sections) ? data.briefing_sections : {};
    const cards = [renderVitalsHtml(data)];
    if (data.home_state === 'welcome') {
      cards.push(renderWelcomeHtml());
      setSurfaceHtml(cards.join(''));
      return;
    }
    cards.push(renderYesterdayProcessingHtml(data));
    cards.push(renderBriefingShellHtml(data));
    cards.push(renderNarrativeHtml(data));
    cards.push(renderWeeklyReflectionHtml(data));
    cards.push(renderTodayHtml(data));
    cards.push(renderNeedsYouHtml(data));
    cards.push(renderConnectionsHtml(data));
    setSurfaceHtml(cards.join(''));
    restoreSectionState();
    refreshBriefing();
  }

  function renderVitalsHtml(pulse) {
    const g = pulse.health_glance || {};
    const severity = g.severity || 'amber'; // 'neutral' is truthy; do not fold it into this fallback
    let html = '<div class="pulse-vitals" id="pulse-vitals" data-home-surface="vitals" role="status" aria-live="polite">'
      + '<div class="pulse-vitals-item"><span class="pulse-vitals-dot ' + esc(severity) + '" aria-hidden="true"></span>'
      + '<span class="pulse-vitals-verdict ' + esc(severity) + '">' + esc(g.headline || '') + '</span></div>';
    if (g.verdict === 'attention') {
      (g.issues || []).forEach(function (issue) {
        html += '<a class="pulse-vitals-chip ' + esc(issue.severity || 'amber') + '" href="' + esc(issue.href || '#') + '">'
          + esc(issue.text || '') + ' →</a>';
      });
    } else {
      if (g.last_observation) {
        html += '<div class="pulse-vitals-sep"></div><div class="pulse-vitals-item">last reached your journal ' + esc(g.last_observation) + '</div>';
      }
      if (g.cta) {
        html += '<div class="pulse-vitals-sep"></div><a class="pulse-vitals-item pulse-vitals-cta" href="' + esc(g.cta.href || '#') + '">'
          + esc(g.cta.text || '') + '</a>';
      }
    }
    html += '<a class="pulse-vitals-health-link" href="/app/health">health →</a></div>';
    return html;
  }

  function renderVitals(pulse) {
    validatePulse(pulse);
    replaceHomeSurface('vitals', renderVitalsHtml(pulse), []);
  }

  function renderWelcomeHtml() {
    return '<div class="pulse-welcome" data-home-surface="welcome">'
      + '<h2>welcome to your journal</h2>'
      + '<p>this is where your day comes together: narrative summaries, calendar events, and tasks. as the solstone app takes in what you share with it, and all of it goes into your journal, sections will appear automatically.</p>'
      + '<a href="/app/health">check system health →</a>'
      + '</div>';
  }

  function renderNarrativeHtml(pulse) {
    if (pulse.narrative_content !== null && pulse.narrative_content !== undefined) {
      const updated = pulse.narrative_updated_at ? '<div class="pulse-narrative-meta">updated at ' + esc(window.JournalFormat.timestamp(pulse.narrative_updated_at)) + '</div>' : '';
      return '<div class="pulse-narrative" id="pulse-narrative" data-home-surface="narrative" data-section-collapsed="true">'
        + '<div class="pulse-section-toggle" role="button" tabindex="0" aria-expanded="false">'
        + '<h2 class="pulse-section-header">' + esc(pulse.narrative_header || "today's flow") + '</h2>'
        + '<span class="pulse-section-summary">' + esc(pulse.narrative_updated_at ? 'updated ' + window.JournalFormat.timestamp(pulse.narrative_updated_at) : pulse.narrative_header || '') + '</span>'
        + '</div>'
        + '<div class="pulse-section-body">'
        + '<div class="pulse-narrative-content" id="pulse-narrative-content">' + markdown(pulse.narrative_content || '') + '</div>'
        + '<a class="pulse-tell-more" href="/app/thinking/#runs/' + encodeURIComponent(pulse.today) + '/' + encodeURIComponent(pulse.narrative_source || 'pulse') + '">view generation →</a>'
        + updated
        + '</div>'
        + '</div>';
    }
    if (Number(pulse.segment_count || 0) > 0) {
      return '<div class="pulse-narrative" id="pulse-narrative" data-home-surface="narrative">'
        + '<h2 class="pulse-section-header">' + esc(pulse.narrative_header || "today's flow") + '</h2>'
        + '<div class="pulse-narrative-empty">analysis will be available after the next processing cycle.</div>'
        + '</div>';
    }
    return '<div class="pulse-empty-state" data-home-surface="narrative">'
      + '<h2 class="pulse-section-header">today\'s flow</h2>'
      + '<div class="pulse-empty-message">no narrative yet. one will appear after some audio goes into your journal.</div>'
      + '</div>';
  }

  function renderNarrative(pulse) {
    replaceHomeSurface('narrative', renderNarrativeHtml(pulse), ['vitals', 'yesterday', 'briefing']);
  }

  function renderWeeklyReflectionHtml(pulse) {
    const reflection = pulse.latest_weekly_reflection;
    if (!reflection || !reflection.url) return '';
    return '<div class="pulse-reflection" data-home-surface="reflection">'
      + '<h2 class="pulse-section-header">weekly reflection</h2>'
      + '<a class="pulse-reflection-link" href="' + esc(reflection.url) + '">week of ' + esc(reflection.label || reflection.day || '') + ' →</a>'
      + '</div>';
  }

  function renderWeeklyReflection(pulse) {
    replaceHomeSurface('reflection', renderWeeklyReflectionHtml(pulse), ['vitals', 'yesterday', 'briefing', 'narrative']);
  }

  function currentTimeString(now) {
    const text = String(now || '');
    if (text.includes('T')) {
      return text.split('T', 2)[1].substring(0, 8);
    }
    return text.substring(0, 8);
  }

  function renderTodayHtml(pulse) {
    const anticipated = Array.isArray(pulse.anticipated_activities) ? pulse.anticipated_activities : [];
    const activities = Array.isArray(pulse.activities) ? pulse.activities : [];
    if (!anticipated.length && !activities.length) {
      return '<div class="pulse-empty-state" data-home-surface="today">'
        + '<h2 class="pulse-section-header">today</h2>'
        + '<div class="pulse-empty-message">no anticipated activities or recent activity yet today.</div>'
        + '</div>';
    }
    const nowTime = currentTimeString(pulse.now);
    let html = '<div class="pulse-today" id="pulse-today" data-home-surface="today" data-section-collapsed="true">'
      + '<div class="pulse-section-toggle" role="button" tabindex="0" aria-expanded="false">'
      + '<h2 class="pulse-section-header">today</h2>'
      + '<span class="pulse-section-summary">' + esc(pulse.today_summary || '') + '</span>'
      + '</div>'
      + '<div class="pulse-section-body">';
    if (anticipated.length) {
      html += '<div class="pulse-events">';
      anticipated.forEach(function (event) {
        const title = event.title || 'Untitled';
        const start = String(event.start || '').substring(0, 5);
        const isPast = Boolean(event.occurred) || Boolean(event.end && String(event.end) < nowTime);
        html += '<div class="pulse-event' + (isPast ? ' past' : '') + '">'
          + '<span class="pulse-event-time">' + esc(start) + '</span>'
          + '<span class="pulse-event-title">' + esc(title) + '</span>'
          + '</div>';
      });
      html += '</div>';
    }
    if (activities.length) {
      html += '<div class="pulse-activities-label">recent activity</div><div class="pulse-activities">';
      activities.slice(0, 6).forEach(function (activity) {
        const description = activity.description || activity.activity || '';
        html += '<div class="pulse-activity">'
          + '<span class="pulse-activity-time">' + esc(activity.display_time ? window.JournalFormat.timestamp(activity.display_time) : '') + '</span>'
          + '<span>' + esc(description) + '</span>'
          + '</div>';
      });
      html += '</div>';
    }
    if (isPlainObject(pulse.facet_data) && Object.keys(pulse.facet_data).length) {
      html += '<div class="pulse-facet-dist">';
      Object.entries(pulse.facet_data).forEach(function ([name, data]) {
        html += '<span class="pulse-facet-chip">' + esc(name) + ' · ' + esc(parseInt(data?.minutes || 0, 10) || 0) + 'm</span>';
      });
      html += '</div>';
    }
    html += '</div></div>';
    return html;
  }

  function renderToday(pulse) {
    replaceHomeSurface('today', renderTodayHtml(pulse), ['vitals', 'yesterday', 'briefing', 'narrative', 'reflection']);
  }

  function renderYesterdayProcessingHtml(pulse) {
    const yesterday = pulse.yesterday_processing;
    if (!yesterday) return '';
    const collapsed = yesterday.default_collapsed ? 'true' : 'false';
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
      const gapLinks = yesterday.gap_links || [];
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
    replaceHomeSurface('yesterday', renderYesterdayProcessingHtml(pulse), ['vitals']);
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

  function renderNeedsYou(pulse) {
    replaceHomeSurface('needs', renderNeedsYouHtml(pulse), ['vitals', 'yesterday', 'briefing', 'narrative', 'reflection', 'today']);
  }

  function formatConnectionDay(day, referenceDay) {
    const text = String(day || '');
    if (!/^\d{8}$/.test(text)) return text;
    const date = new Date(Number(text.slice(0, 4)), Number(text.slice(4, 6)) - 1, Number(text.slice(6, 8)));
    let label = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' }).toLowerCase();
    const reference = String(referenceDay || '');
    if (/^\d{8}$/.test(reference) && text.slice(0, 4) !== reference.slice(0, 4)) {
      label += " '" + text.slice(2, 4);
    }
    return label;
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

  function connectionRowMeta(neighbor, referenceDay) {
    const count = Number(neighbor.count || 0);
    const moments = count === 1 ? '1 moment' : String(count) + ' moments';
    const day = formatConnectionDay(neighbor.last_seen, referenceDay);
    return day ? moments + ' · ' + day : moments;
  }

  function connectionRowHtml(neighbor, connections, referenceDay) {
    const entityId = typeof neighbor.entity_id === 'string' ? neighbor.entity_id : '';
    const name = typeof neighbor.name === 'string' && neighbor.name ? neighbor.name : entityId;
    if (!entityId || !name) return '';
    return '<div class="pulse-connections-row">'
      + '<a class="pulse-connections-name" href="' + esc(connectionEntityHref(entityId)) + '">' + esc(name) + '</a>'
      + connectionKindChipsHtml(neighbor, connections)
      + '<span class="pulse-connections-meta">' + esc(connectionRowMeta(neighbor, referenceDay)) + '</span>'
      + (neighbor.latest_label ? '<span class="pulse-connections-evidence">' + esc(neighbor.latest_label) + '</span>' : '')
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

  function renderConnectionsHtml(pulse) {
    const connections = isPlainObject(pulse?.connections) ? pulse.connections : null;
    if (!connections || typeof connections.state !== 'string') return '';
    if (connections.state === 'empty') {
      return '<div class="pulse-empty-state" data-home-surface="connections">'
        + '<h2 class="pulse-section-header">connections</h2>'
        + '<div class="pulse-empty-message">no connections yet. this is built from the people and things your days involve.</div>'
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
      .filter(function (neighbor) { return neighbor.evidence_class !== 'attendance' && !mentionOnly(neighbor); })
      .slice(0, 6);
    const attendanceRows = neighbors
      .filter(function (neighbor) { return neighbor.evidence_class === 'attendance'; })
      .slice(0, 8);
    const referenceDay = pulse?.today || '';
    const relationshipItems = relationshipRows.map(function (neighbor) {
      return connectionRowHtml(neighbor, connections, referenceDay);
    }).filter(Boolean);
    const attendanceItems = attendanceRows.map(connectionAttendanceItemHtml).filter(Boolean);
    let html = '<div class="pulse-connections" data-home-surface="connections">'
      + '<h2 class="pulse-section-header">connections</h2>';

    if (relationshipItems.length) {
      html += '<div class="pulse-connections-shelf">'
        + '<div class="pulse-connections-shelf-label">connections found in your journal</div>'
        + '<div class="pulse-connections-list">'
        + relationshipItems.join('')
        + '</div></div>';
    } else {
      html += '<div class="pulse-connections-note">no direct connections found yet.</div>';
    }

    if (attendanceItems.length) {
      html += '<div class="pulse-connections-shelf">'
        + '<div class="pulse-connections-shelf-label">often around — events only</div>'
        + '<div class="pulse-connections-cluster">' + attendanceItems.join(' · ') + '</div>'
        + '</div>';
    }

    if (mentionRows.length) {
      html += '<details class="pulse-connections-shelf"><summary>mentioned in your journal</summary><div class="pulse-connections-list">'
        + mentionRows.map(neighbor => connectionRowHtml(neighbor, connections, referenceDay)).join('') + '</div></details>';
    }
    const rendered = relationshipItems.length + attendanceItems.length + mentionRows.length;
    if (typeof connections.horizon_note === 'string' && connections.horizon_note
        && typeof connections.horizon_day === 'string' && connections.horizon_day) {
      html += '<div class="pulse-connections-horizon">'
        + esc(connections.horizon_note.replace('{day}', formatConnectionDay(connections.horizon_day, referenceDay)))
        + '</div>';
    }
    if (Number(connections.total || 0) > rendered) {
      html += '<div class="pulse-connections-footer"><a href="/app/entities">all connections →</a></div>';
    }
    return html + '</div>';
  }

  function renderConnections(pulse) {
    replaceHomeSurface('connections', renderConnectionsHtml(pulse), ['vitals', 'yesterday', 'briefing', 'narrative', 'reflection', 'today', 'needs']);
  }

  function normalizeBriefingFromPulse(pulse) {
    return {
      exists: Boolean(pulse.briefing_exists),
      phase: pulse.briefing_phase || 'eod',
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

  function briefingPlaceholderHtml(data, pulseContext) {
    if (data.phase !== 'pending' || data.exists) return '';
    const lateness = pulseContext?.briefing_lateness || {};
    if (lateness.late) {
      return '<div class="pulse-briefing-placeholder">'
        + "your briefing is usually ready by 10 am; it's " + esc(lateness.late_hours || 0) + 'h late.'
        + '<a class="pulse-briefing-status-link" href="/app/thinking/#runs/' + esc(pulseContext?.today || '') + '/morning_briefing">check status</a>'
        + '</div>';
    }
    return '<div class="pulse-briefing-placeholder">your morning briefing is being prepared...</div>';
  }

  function renderBriefingCardHtml(data, pulseContext) {
    if (!data.exists && data.phase !== 'pending') return '';
    const existing = document.getElementById('pulse-briefing');
    const collapsed = data.phase === 'morning' ? 'false' : (existing?.dataset?.collapsed || (data.phase === 'morning' ? 'false' : 'true'));
    const metaText = data.meta && data.meta.generated ? formatBriefingTime(data.meta.generated) : '';
    const summary = data.summary ? '<div class="pulse-briefing-summary">' + esc(data.summary) + '</div>' : '';
    const badge = data.needs_badge ? '<span class="pulse-briefing-badge">' + esc(data.needs_badge) + '</span>' : '';
    const meta = metaText ? '<span class="pulse-briefing-meta">' + esc(metaText) + '</span>' : '';
    const body = data.exists
      ? '<div class="pulse-briefing-body" id="pulse-briefing-body">' + renderBriefingSectionsHtml(data) + '</div>'
      : '';
    return '<div class="pulse-briefing-card" id="pulse-briefing" data-home-surface="briefing" data-phase="' + esc(data.phase) + '" data-collapsed="' + collapsed + '">'
      + '<div class="pulse-briefing-header" role="button" tabindex="0" aria-expanded="' + (collapsed === 'false' ? 'true' : 'false') + '">'
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
              const text = String(item || '');
              return '<li>' + esc(text) + '</li>';
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
    replaceHomeSurface('briefing', html, ['vitals', 'yesterday']);
  }

  function renderBriefingError(error) {
    const html = '<div class="pulse-briefing-card" id="pulse-briefing" data-home-surface="briefing" data-phase="error" data-collapsed="false">'
      + surfaceError(error, "Couldn't refresh briefing")
      + '</div>';
    replaceHomeSurface('briefing', html, ['vitals', 'yesterday']);
    const retry = document.querySelector('#pulse-briefing .surface-state-retry');
    if (retry) {
      retry.addEventListener('click', function () {
        refreshBriefing();
      });
    }
  }

  function refreshVitals() {
    apiJson(PULSE_URL)
      .then(function (data) {
        validatePulse(data);
        lastPulse = Object.assign({}, lastPulse || {}, data);
        renderNeedsYou(data);
        renderConnections(data);
        renderVitals(data);
        clearPulseRefreshError('pulse-vitals');
      })
      .catch(function (error) {
        logError(error, 'home: refreshVitals failed');
        renderRefreshError('pulse-vitals', error, "Couldn't refresh vitals — showing last known state.");
      });
  }

  function refreshNarrative() {
    apiJson(PULSE_URL)
      .then(function (data) {
        validatePulse(data);
        lastPulse = Object.assign({}, lastPulse || {}, data);
        renderNarrative(data);
        renderNeedsYou(data);
        renderConnections(data);
        clearPulseRefreshError('pulse-narrative');
      })
      .catch(function (error) {
        logError(error, 'home: refreshNarrative failed');
        renderRefreshError('pulse-narrative', error, "Couldn't refresh narrative — showing last known state.");
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
    if (!card || card.dataset.phase === 'pending') return;
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
    try {
      const saved = JSON.parse(sessionStorage.getItem(SECTION_STATE_KEY) || '{}');
      SECTION_IDS.forEach(function (id) {
        const el = document.getElementById(id);
        if (!el || !el.hasAttribute('data-section-collapsed')) return;
        const collapsed = Object.prototype.hasOwnProperty.call(saved, id) ? saved[id] : SECTION_DEFAULTS[id];
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
  }

  function handleDashboardKeydown(event) {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    const target = event.target;
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
  }

  function wireInteractions() {
    if (interactionsWired || !root) return;
    root.addEventListener('click', handleDashboardClick);
    root.addEventListener('keydown', handleDashboardKeydown);
    interactionsWired = true;
  }

  function wireRealtime() {
    if (realtimeWired || !window.appEvents) return;
    window.appEvents.listen('supervisor', function (msg) {
      if (msg.event === 'status') refreshVitals();
    });
    window.appEvents.listen('observe', function (msg) {
      if (msg.event === 'observed' || msg.event === 'status') refreshVitals();
    });
    window.appEvents.listen('cortex', function (msg) {
      if (msg.event === 'finish' && (msg.name === 'flow' || msg.name === 'pulse')) refreshNarrative();
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
    wireInteractions();
    wireRealtime();
    loadPulse();
  }

  window.toggleBriefingCard = toggleBriefingCard;
  window.toggleYesterdayCard = toggleYesterdayCard;
  window.toggleBriefingSection = toggleBriefingSection;
  window.toggleSection = toggleSection;

  document.addEventListener('workspace:mounted', function (event) {
    const appName = event?.detail?.app || event?.detail?.name || '';
    if (appName && appName !== 'home') return;
    initHome();
  });
  if (document.readyState === 'complete') {
    initHome();
  }
})();
