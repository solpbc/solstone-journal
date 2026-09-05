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
    const monthsAgo =
      (today.getFullYear() - parsed.getFullYear()) * 12 +
      (today.getMonth() - parsed.getMonth());
    if (monthsAgo > 6 && parsed.getFullYear() !== today.getFullYear()) {
      short += ` '${String(parsed.getFullYear()).slice(-2)}`;
    }
    return short;
  }

  function formatStreamLabel(stream) {
    return String(stream || '').replace(/[._-]+/g, ' ').trim();
  }

  function formatSegmentTime(segment) {
    const match = /^(\d{2})(\d{2})(\d{2})(?:_|$)/.exec(String(segment || ''));
    if (!match || Number(match[1]) > 23 || Number(match[2]) > 59 || Number(match[3]) > 59) return 'time unavailable';
    return `${match[1]}:${match[2]}:${match[3]}`;
  }

  function formatTimestamp(timestamp) {
    if (timestamp === null || timestamp === undefined || timestamp === '') return 'time unavailable';
    const value = new Date(timestamp);
    return Number.isNaN(value.getTime()) ? 'time unavailable' : value.toLocaleString(undefined, {
      year: value.getFullYear() === new Date().getFullYear() ? undefined : 'numeric',
      month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit'
    });
  }

  function formatDuration(seconds) {
    if (seconds === null || seconds === undefined || !Number.isFinite(Number(seconds))) return 'duration unavailable';
    const value = Math.round(Math.max(0, Number(seconds)));
    return value < 60 ? `${Math.round(value)} sec` : `${Math.floor(value / 60)} min ${Math.round(value % 60)} sec`;
  }

  window.JournalFormat = { compactTokens: value => value >= 999500 ? `${Math.round(value / 1000000)}M` : value >= 1000 ? `${Math.round(value / 1000)}K` : String(Math.round(value)),  day: formatDateShort, stream: formatStreamLabel, segmentTime: formatSegmentTime, timestamp: formatTimestamp, duration: formatDuration };
  window.formatDateShort = formatDateShort;
})();
