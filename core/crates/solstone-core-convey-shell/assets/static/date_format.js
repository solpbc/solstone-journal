// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const WEEKDAYS = [
    'Sunday',
    'Monday',
    'Tuesday',
    'Wednesday',
    'Thursday',
    'Friday',
    'Saturday'
  ];
  const WEEKDAYS_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
  const MONTHS_SHORT = [
    'Jan',
    'Feb',
    'Mar',
    'Apr',
    'May',
    'Jun',
    'Jul',
    'Aug',
    'Sep',
    'Oct',
    'Nov',
    'Dec'
  ];

  function parseDay(dateString) {
    dateString = String(dateString || '');
    if (!/^\d{8}$/.test(dateString)) return null;
    const year = Number(dateString.slice(0, 4));
    const month = Number(dateString.slice(4, 6)) - 1;
    const day = Number(dateString.slice(6, 8));
    const parsed = new Date(year, month, day);
    if (
      parsed.getFullYear() !== year ||
      parsed.getMonth() !== month ||
      parsed.getDate() !== day
    ) {
      return null;
    }
    parsed.setHours(0, 0, 0, 0);
    return parsed;
  }

  function normalizeToday(now) {
    const today = now instanceof Date ? new Date(now) : new Date();
    today.setHours(0, 0, 0, 0);
    return today;
  }

  function currentJournalYear() {
    if (window.JournalClock) {
      return window.JournalClock.parts(window.JournalClock.now()).year;
    }
    return new Date().getFullYear();
  }

  function formatDateShort(dateString, now) {
    const parsed = parseDay(dateString);
    if (!parsed) return dateString;

    let today;
    if (now) {
      today = normalizeToday(now);
    } else if (window.JournalClock) {
      const todayKey = window.JournalClock.today();
      today = parseDay(todayKey) || normalizeToday();
    } else {
      today = normalizeToday();
    }
    const deltaDays = Math.round((parsed.getTime() - today.getTime()) / 86400000);

    if (deltaDays === 0) return 'Today';
    if (deltaDays === -1) return 'Yesterday';
    if (deltaDays === 1) return 'Tomorrow';
    if (deltaDays >= -6 && deltaDays < 0) return WEEKDAYS[parsed.getDay()];

    let short = `${WEEKDAYS_SHORT[parsed.getDay()]} ${MONTHS_SHORT[parsed.getMonth()]} ${parsed.getDate()}`;
    const thisYear = currentJournalYear();
    if (parsed.getFullYear() !== thisYear) {
      short += ` '${String(parsed.getFullYear()).slice(-2)}`;
    }
    return short;
  }

  // Always an anchored date. `formatDateShort` deliberately collapses the last
  // week to a bare weekday, which loses its anchor in a list that is not in
  // date order — and a bare "Tuesday" is not enough to decide a deletion on.
  function formatDateFull(dateString, now) {
    const parsed = parseDay(dateString);
    if (!parsed) return dateString;
    let label = `${WEEKDAYS_SHORT[parsed.getDay()]} ${MONTHS_SHORT[parsed.getMonth()]} ${parsed.getDate()}`;
    const thisYear = currentJournalYear();
    if (parsed.getFullYear() !== thisYear) {
      label += ` '${String(parsed.getFullYear()).slice(-2)}`;
    }
    return label;
  }

  // How an `import.<source>` stream reads, matching the journal's provenance
  // table (solstone-core-home/src/provenance.rs). The catalog sources use the
  // import catalog's own `display_name` (`SOURCES` in
  // solstone-core-import-web/src/imports.rs); the body sources come in through
  // `solstone-core body`; the rest name the kind of file imported. A source
  // with no entry here keeps the parsed lowercase form. S-8 / fresh-eyes 3 #7.
  const IMPORT_SOURCE_LABELS = {
    // body sources
    apple_health: 'Apple Health',
    oura: 'Oura',
    strava: 'Strava',
    ics: 'calendar',
    document: 'documents',
    image: 'images',
    audio: 'audio',
    // import catalog
    chatgpt: 'ChatGPT',
    claude: 'Claude',
    gemini: 'Gemini',
    journal_archive: 'another journal',
    kindle: 'Kindle',
    obsidian: 'notes',
  };

  // The journal's provenance table (`/api/streams/labels`). A name the table
  // holds reads its label; a name a loaded table lacks has no registry record
  // and reads unknown. Only while no table has loaded, or for a name whose own
  // re-read failed, does a name alone decide, and then only a dotted one.
  let streamLabelsTable = null;
  let inFlightLabelsFetch = null;
  const settledRereadStreams = new Set();
  const fallbackStreams = new Set();
  const loggedLabelProblems = new Set();

  // A table the journal could not read, or a read that errored, is a fault
  // and goes to the page's error log (with the original error, so a fetch
  // cancelled by navigation is recognised as one). A slow read and a partial
  // table are not faults: the page still names what it can, so they only warn.
  function logLabelProblem(kind, error) {
    if (loggedLabelProblems.has(kind)) return;
    loggedLabelProblems.add(kind);
    if (kind === 'failed' && typeof window !== 'undefined' && typeof window.logError === 'function') {
      window.logError(error || new Error('stream labels unavailable'), { context: 'stream-labels' });
    } else if (typeof console !== 'undefined' && typeof console.warn === 'function') {
      console.warn(`stream labels ${kind}`);
    }
  }

  function dottedFallback(raw) {
    if (!raw || raw === '_default') return 'source unknown';
    // Exactly one dot with a name on each side, the shape the journal's own
    // table reads; anything else is unknown there too.
    const parts = raw.split('.');
    if (parts.length !== 2 || parts.some(part => part === '')) return 'source unknown';
    if (parts[0] === 'import' && parts.length === 2) {
      const branded = Object.prototype.hasOwnProperty.call(IMPORT_SOURCE_LABELS, parts[1])
        ? IMPORT_SOURCE_LABELS[parts[1]]
        : parts[1].replace(/[_-]+/g, ' ').trim().toLowerCase();
      return `${branded} · imported`;
    }
    if (parts.length === 2) {
      const qualifier = parts[1];
      if (qualifier === 'tmux') return 'your terminal';
      if (qualifier === 'watch') return 'your watch';
      if (qualifier === 'mobile' || qualifier === 'phone') return 'your phone';
      if (qualifier === 'browser') return 'your browser';
    }
    return 'source unknown';
  }

  // A complete table replaces any table; a partial one replaces a partial
  // one. A partial read never changes a label a complete table already holds:
  // it only adds the names that table lacks. A failed or malformed body
  // installs nothing.
  function installStreamLabels(body) {
    if (!body || typeof body !== 'object' || !body.labels || typeof body.labels !== 'object') return;
    if (body.status !== 'complete' && body.status !== 'partial') return;
    const says = body.says && typeof body.says === 'object' ? body.says : {};
    if (body.status === 'partial' && streamLabelsTable && streamLabelsTable.status === 'complete') {
      for (const [name, label] of Object.entries(body.labels)) {
        if (!Object.prototype.hasOwnProperty.call(streamLabelsTable.labels, name)) {
          streamLabelsTable.labels[name] = label;
          if (says[name]) streamLabelsTable.says[name] = says[name];
        }
      }
    } else {
      streamLabelsTable = {
        status: body.status,
        labels: Object.assign({}, body.labels),
        says: Object.assign({}, says),
      };
    }
    // The registry was read whole: a name an earlier failed read left on the
    // fallback now reads what this table says of it.
    fallbackStreams.clear();
  }

  // What a stream's label says, as parts: its what word ("audio", "screen",
  // "camera", "location", "calendar") or null, and whether it names an
  // import. Pages use this to avoid printing a content type the label already
  // says.
  function streamLabelSays(stream) {
    const raw = String(stream || '').trim();
    const none = { what: null, imported: false };
    if (hasStreamLabel(raw)) {
      const says = streamLabelsTable.says[raw];
      return says ? { what: says.what || null, imported: Boolean(says.imported) } : none;
    }
    if (!streamLabelsTable || fallbackStreams.has(raw)) {
      const parts = raw.split('.');
      return { what: null, imported: parts.length === 2 && parts[0] === 'import' && parts[1] !== '' };
    }
    return none;
  }

  function hasStreamLabel(stream) {
    const raw = String(stream || '').trim();
    return Boolean(streamLabelsTable && Object.prototype.hasOwnProperty.call(streamLabelsTable.labels, raw));
  }

  // Whether a live page should re-read the table for this name: it is not in
  // the table and has not had its one re-read yet.
  function needsStreamLabel(stream) {
    const raw = String(stream || '').trim();
    return Boolean(raw) && raw !== '_default' && !hasStreamLabel(raw) && !settledRereadStreams.has(raw);
  }

  function formatStreamLabel(stream) {
    const raw = String(stream || '').trim();
    if (!raw || raw === '_default') return 'source unknown';
    if (hasStreamLabel(raw)) return streamLabelsTable.labels[raw];
    if (!streamLabelsTable || fallbackStreams.has(raw)) return dottedFallback(raw);
    return 'source unknown';
  }

  // Reads the table. Resolves true when the read answered (complete or
  // partial), false when it failed or took longer than LABELS_READ_LIMIT_MS;
  // never rejects. Concurrent callers share one read.
  const LABELS_READ_LIMIT_MS = 10000;
  function beginStreamLabels() {
    if (inFlightLabelsFetch) return inFlightLabelsFetch;
    const controller = typeof AbortController === 'function' ? new AbortController() : null;
    const limit = controller ? setTimeout(() => controller.abort(), LABELS_READ_LIMIT_MS) : null;
    inFlightLabelsFetch = fetch('/api/streams/labels', controller ? { signal: controller.signal } : undefined)
      .then((res) => {
        if (!res.ok) throw new Error(`stream labels unavailable: HTTP ${res.status}`);
        return res.json();
      })
      .then((body) => {
        if (body && (body.status === 'complete' || body.status === 'partial')) {
          installStreamLabels(body);
          if (body.status === 'partial') logLabelProblem('partial');
          return true;
        }
        logLabelProblem('failed', new Error('stream labels unavailable'));
        return false;
      })
      .catch((err) => {
        if (controller && controller.signal.aborted) logLabelProblem('slow');
        else logLabelProblem('failed', err);
        return false;
      })
      .finally(() => {
        if (limit !== null) clearTimeout(limit);
        inFlightLabelsFetch = null;
      });
    return inFlightLabelsFetch;
  }

  // A stream a live page first meets after load: re-read the table once for
  // it. A read already in flight may have been taken before the stream
  // existed, so a miss there is followed by one read of its own. If the
  // re-read fails, only the names waiting on it fall back; labels already
  // shown stay as they are.
  function noteLiveStream(name) {
    const raw = String(name || '').trim();
    if (!raw || raw === '_default') return Promise.resolve('source unknown');
    if (hasStreamLabel(raw) || settledRereadStreams.has(raw)) {
      return Promise.resolve(formatStreamLabel(raw));
    }
    const joinedEarlierRead = Boolean(inFlightLabelsFetch);
    return beginStreamLabels()
      .then((answered) => (answered && joinedEarlierRead && !hasStreamLabel(raw) ? beginStreamLabels() : answered))
      .then((answered) => {
        settledRereadStreams.add(raw);
        if (!answered && !hasStreamLabel(raw)) fallbackStreams.add(raw);
        return formatStreamLabel(raw);
      });
  }

  function parseSegmentClock(segment) {
    const match = /^(\d{2})(\d{2})(\d{2})(?:_|$)/.exec(String(segment || ''));
    if (!match || Number(match[1]) > 23 || Number(match[2]) > 59 || Number(match[3]) > 59) return null;
    return match;
  }

  function formatSegmentTime(segment) {
    const clock = parseSegmentClock(segment);
    return clock ? `${clock[1]}:${clock[2]}:${clock[3]}` : 'time unavailable';
  }

  // The same segment prefix on the page's own clock. `formatSegmentTime` is
  // 24-hour and always carries seconds, which is right inside a disclosure and
  // wrong on a row that sits under a "10:55 PM". G1-202.
  function formatSegmentTimeOfDay(segment) {
    const clock = parseSegmentClock(segment);
    if (!clock) return 'time unavailable';
    return new Date(2000, 0, 1, Number(clock[1]), Number(clock[2]), Number(clock[3]))
      .toLocaleString(undefined, { hour: 'numeric', minute: '2-digit' });
  }

  function formatTimestamp(timestamp) {
    if (timestamp === null || timestamp === undefined || timestamp === '') return 'time unavailable';
    const thisYear = currentJournalYear();
    const d = timestamp instanceof Date ? timestamp : new Date(timestamp);
    if (Number.isNaN(d.getTime())) return 'time unavailable';
    const p = window.JournalClock ? window.JournalClock.parts(d) : null;
    const isThisYear = p ? p.year === thisYear : d.getFullYear() === thisYear;
    const options = {
      year: isThisYear ? undefined : 'numeric',
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    };
    if (window.JournalClock) {
      return window.JournalClock.formatInstant(d, options);
    }
    return d.toLocaleString(undefined, options);
  }

  // The time half of the absolute ladder, for a list that already knows which
  // day it is showing. Same rendering as `formatTimestamp`, without the date.
  function formatTimeOfDay(timestamp) {
    if (timestamp === null || timestamp === undefined || timestamp === '') return 'time unavailable';
    const d = timestamp instanceof Date ? timestamp : new Date(timestamp);
    if (Number.isNaN(d.getTime())) return 'time unavailable';
    const options = { hour: 'numeric', minute: '2-digit' };
    if (window.JournalClock) {
      return window.JournalClock.formatInstant(d, options);
    }
    return d.toLocaleString(undefined, options);
  }

  function formatDuration(seconds) {
    if (seconds === null || seconds === undefined || !Number.isFinite(Number(seconds))) return 'duration unavailable';
    const value = Math.round(Math.max(0, Number(seconds)));
    if (value < 60) return `${value} sec`;
    // Stats' hour buckets reach thousands of minutes, so the ladder needs an
    // hours rung or 1,995 minutes reads "1995 min 0 sec". S-3 / G2-B14. A zero
    // remainder is dropped at every rung above seconds, so five minutes reads
    // "5 min" and an even hour reads "2 hr". S-8.
    if (value < 3600) {
      const remainderSeconds = value % 60;
      const minutes = Math.floor(value / 60);
      return remainderSeconds ? `${minutes} min ${remainderSeconds} sec` : `${minutes} min`;
    }
    const remainderMinutes = Math.floor((value % 3600) / 60);
    const hours = Math.floor(value / 3600);
    return remainderMinutes ? `${hours} hr ${remainderMinutes} min` : `${hours} hr`;
  }

  // A bare lookup on an object literal reaches Object.prototype, so a lane
  // named 'constructor' or 'toString' returned a function and the sentence
  // rendered it. Own keys only.
  //
  // This map is the sentence register: it is read before a reason or a
  // "last confirmed" age, so the local lane says 'local thinking' here, the
  // same noun as the status headline above it. The runs
  // and token tables name the same lane in a provider column, where the row's
  // other cells carry the sense and the bare 'local' is the value (X-05,
  // G2-43, S-5, and the matching notes in thinking.js and token-card.js).
  // 'confidential processing' is one phrase in both, so the column already
  // reads that way.
  const PROCESSING_LANES = {spp: 'confidential processing', local: 'local thinking',
    bundled: 'local thinking', 'byo-cloud': 'your own model', 'byo-endpoint': 'your own model',
    openai: 'OpenAI', anthropic: 'Anthropic', google: 'Google'};
  function processingLane(lane) {
    return Object.hasOwn(PROCESSING_LANES, lane) ? PROCESSING_LANES[lane] : 'processing';
  }

  // A "since ..." sentence needs the day itself. formatDateShort answers
  // "which day is this" with Today and Yesterday, which read as nonsense after
  // "since" and stop being true while the sentence is still on screen. So this
  // one is absolute, lowercase, and carries the year only when it is not this
  // one (F-15). status_pane.js and health.js each held their own copy of it.
  const SINCE_MONTHS = ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec'];
  function sinceDay(input) {
    if (input === null || input === undefined || input === '') return '';
    const thisYear = currentJournalYear();
    if (typeof input === 'string' && /^\d{8}$/.test(input)) {
      const y = parseInt(input.slice(0, 4), 10);
      const m = parseInt(input.slice(4, 6), 10) - 1;
      const d = parseInt(input.slice(6, 8), 10);
      const day = SINCE_MONTHS[m] + ' ' + d;
      return y === thisYear ? day : day + " '" + String(y).slice(-2);
    }
    if (window.JournalClock) {
      const p = window.JournalClock.parts(input);
      if (p && p.year > 0) {
        const m = parseInt(p.month, 10) - 1;
        const d = parseInt(p.day, 10);
        const day = SINCE_MONTHS[m] + ' ' + d;
        return p.year === thisYear ? day : day + " '" + String(p.year).slice(-2);
      }
    }
    const value = new Date(input);
    if (Number.isNaN(value.getTime())) return '';
    const day = SINCE_MONTHS[value.getMonth()] + ' ' + value.getDate();
    return value.getFullYear() === thisYear
      ? day
      : day + " '" + String(value.getFullYear()).slice(-2);
  }

  window.JournalFormat = {
    processingLane,
    sinceDay,
    compactTokens: value => value >= 999500 ? `${Math.round(value / 1000000)}M` : value >= 1000 ? `${Math.round(value / 1000)}K` : String(Math.round(value)),
    day: formatDateShort,
    dayFull: formatDateFull,
    stream: formatStreamLabel,
    streamSays: streamLabelSays,
    installStreamLabels,
    beginStreamLabels,
    hasStreamLabel,
    needsStreamLabel,
    noteLiveStream,
    logStreamLabelProblem: logLabelProblem,
    segmentTime: formatSegmentTime,
    segmentTimeOfDay: formatSegmentTimeOfDay,
    timestamp: formatTimestamp,
    time: formatTimeOfDay,
    duration: formatDuration,
  };
  window.formatDateShort = formatDateShort;
})();
