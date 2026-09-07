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

  function formatStreamLabel(stream) {
    return String(stream || '').replace(/[._-]+/g, ' ').trim();
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
    return value < 60 ? `${Math.round(value)} sec` : `${Math.floor(value / 60)} min ${Math.round(value % 60)} sec`;
  }

  function processingLane(lane) {
    return ({spp: 'confidential processing', local: 'local processing', openai: 'OpenAI',
      anthropic: 'Anthropic', google: 'Google'})[lane] || 'processing';
  }

  window.JournalFormat = { processingLane, compactTokens: value => value >= 999500 ? `${Math.round(value / 1000000)}M` : value >= 1000 ? `${Math.round(value / 1000)}K` : String(Math.round(value)),  day: formatDateShort, dayFull: formatDateFull, stream: formatStreamLabel, segmentTime: formatSegmentTime, segmentTimeOfDay: formatSegmentTimeOfDay, timestamp: formatTimestamp, time: formatTimeOfDay, duration: formatDuration };
  window.formatDateShort = formatDateShort;
})();
