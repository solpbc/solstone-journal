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

  function formatDateShort(dateString, now) {
    const parsed = parseDay(dateString);
    if (!parsed) return dateString;

    const today = normalizeToday(now);
    const deltaDays = Math.round((parsed.getTime() - today.getTime()) / 86400000);

    if (deltaDays === 0) return 'Today';
    if (deltaDays === -1) return 'Yesterday';
    if (deltaDays === 1) return 'Tomorrow';
    if (deltaDays >= -6 && deltaDays < 0) return WEEKDAYS[parsed.getDay()];

    let short = `${WEEKDAYS_SHORT[parsed.getDay()]} ${MONTHS_SHORT[parsed.getMonth()]} ${parsed.getDate()}`;
    // Same rule formatDateFull already uses: a year suffix whenever the day
    // isn't in the current year. The old "more than 6 months ago" gate used
    // today-minus-parsed, which is negative for every future day, so a
    // future-dated day never carried its year at all (G2-52).
    if (parsed.getFullYear() !== today.getFullYear()) {
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
    const today = normalizeToday(now);
    let label = `${WEEKDAYS_SHORT[parsed.getDay()]} ${MONTHS_SHORT[parsed.getMonth()]} ${parsed.getDate()}`;
    if (parsed.getFullYear() !== today.getFullYear()) {
      label += ` '${String(parsed.getFullYear()).slice(-2)}`;
    }
    return label;
  }

  // How an `import.<source>` stream reads. Six of these are the import
  // catalog's own `display_name` (`SOURCES` in
  // solstone-core-import-web/src/imports.rs), so a stream reads the source the
  // way the import screen and its guides write it. The other two are body
  // sources, which the import catalog does not carry and which come in through
  // `solstone-core body`. A source with no entry here keeps the parsed
  // lowercase form. S-8 / fresh-eyes 3 #7.
  const IMPORT_SOURCE_LABELS = {
    // body sources
    apple_health: 'Apple Health',
    oura: 'Oura',
    // import catalog
    chatgpt: 'ChatGPT',
    claude: 'Claude',
    gemini: 'Gemini',
    journal_archive: 'journal',
    kindle: 'Kindle',
    obsidian: 'notes',
  };

  // A stream key is structured, so parse it rather than stripping punctuation:
  // `device.watch.audio` names a watch's audio, not a device called "device
  // watch audio", and `import.chatgpt` is a ChatGPT import, not "import
  // chatgpt". A dot-free key is the machine's own name and stands as it is.
  // Anything else falls back to the old separator swap. G1-203.
  function formatStreamLabel(stream) {
    const raw = String(stream || '').trim();
    const parts = raw.split('.').filter(part => part !== '');
    const words = part => part.replace(/[_-]+/g, ' ').trim();
    // A machine's own stream name is one dot-free label the journal already
    // reduced from its hostname; it reads as itself, dashes and all.
    if (parts.length === 1 && /^[a-z0-9][a-z0-9-]*$/.test(parts[0])) return parts[0];
    if (parts[0] === 'device' && parts.length >= 3) return parts.slice(1).map(words).join(' ').trim();
    if (parts[0] === 'import' && parts.length === 2) {
      const branded = Object.prototype.hasOwnProperty.call(IMPORT_SOURCE_LABELS, parts[1])
        ? IMPORT_SOURCE_LABELS[parts[1]]
        : words(parts[1]);
      return `${branded} import`;
    }
    return raw.replace(/[._-]+/g, ' ').trim();
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
    const value = new Date(timestamp);
    return Number.isNaN(value.getTime()) ? 'time unavailable' : value.toLocaleString(undefined, {
      year: value.getFullYear() === new Date().getFullYear() ? undefined : 'numeric',
      month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit'
    });
  }

  // The time half of the absolute ladder, for a list that already knows which
  // day it is showing. Same rendering as `formatTimestamp`, without the date.
  function formatTimeOfDay(timestamp) {
    if (timestamp === null || timestamp === undefined || timestamp === '') return 'time unavailable';
    const value = new Date(timestamp);
    return Number.isNaN(value.getTime()) ? 'time unavailable' : value.toLocaleString(undefined, {
      hour: 'numeric', minute: '2-digit'
    });
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
  // This map is the sentence register: it is read after "running on" and
  // before a reason, so the local lane says 'local processing' here. The runs
  // and token tables name the same lane in a provider column, where the row's
  // other cells carry the sense and the bare 'local' is the value (X-05,
  // G2-43, S-5, and the matching notes in thinking.js and token-card.js).
  // 'confidential processing' is one phrase in both, so the column already
  // reads that way.
  const PROCESSING_LANES = {spp: 'confidential processing', local: 'local processing',
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
  function sinceDay(ms) {
    const value = new Date(ms);
    if (Number.isNaN(value.getTime())) return '';
    const day = SINCE_MONTHS[value.getMonth()] + ' ' + value.getDate();
    return value.getFullYear() === new Date().getFullYear()
      ? day
      : day + " '" + String(value.getFullYear()).slice(-2);
  }

  window.JournalFormat = { processingLane, sinceDay, compactTokens: value => value >= 999500 ? `${Math.round(value / 1000000)}M` : value >= 1000 ? `${Math.round(value / 1000)}K` : String(Math.round(value)),  day: formatDateShort, dayFull: formatDateFull, stream: formatStreamLabel, segmentTime: formatSegmentTime, segmentTimeOfDay: formatSegmentTimeOfDay, timestamp: formatTimestamp, time: formatTimeOfDay, duration: formatDuration };
  window.formatDateShort = formatDateShort;
})();
