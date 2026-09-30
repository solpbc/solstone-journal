// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  let _zone = null;
  let _label = '';
  let _nowFn = null;
  let _hasLoggedError = false;

  function setNow(fn) {
    _nowFn = typeof fn === 'function' ? fn : null;
  }

  function now() {
    return new Date(_nowFn ? _nowFn() : Date.now());
  }

  function zone() {
    return _zone;
  }

  function label() {
    return _label;
  }

  function seed(payload) {
    const tz = payload && payload.tz;
    const lbl = payload && payload.label;
    if (!tz) {
      _zone = null;
      _label = '';
      return;
    }
    try {
      new Intl.DateTimeFormat('en-US', { timeZone: tz }).format(now());
      _zone = tz;
      _label = typeof lbl === 'string' ? lbl : '';
      _hasLoggedError = false;
    } catch (err) {
      if (err instanceof RangeError || (err && err.name === 'RangeError')) {
        _zone = null;
        _label = '';
        if (!_hasLoggedError) {
          _hasLoggedError = true;
          if (typeof window.logError === 'function') {
            window.logError(err);
          }
        }
      } else {
        throw err;
      }
    }
  }

  // Building an Intl formatter is the expensive part of reading a wall
  // clock, and day starts read it many times, so keep one per zone.
  const _partsFormatters = new Map();
  function partsFormatter(timeZone) {
    let dtf = _partsFormatters.get(timeZone);
    if (!dtf) {
      dtf = new Intl.DateTimeFormat('en-US', {
        timeZone,
        year: 'numeric',
        month: '2-digit',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
        weekday: 'short',
        hourCycle: 'h23',
      });
      _partsFormatters.set(timeZone, dtf);
    }
    return dtf;
  }

  function parts(instant) {
    if (instant === null || instant === undefined || instant === '') {
      instant = now();
    }
    const d = instant instanceof Date ? instant : new Date(instant);
    if (Number.isNaN(d.getTime())) {
      return {
        year: 0,
        month: '00',
        day: '00',
        hour: '00',
        minute: '00',
        second: '00',
        weekday: 0,
      };
    }
    if (_zone) {
      const formatted = partsFormatter(_zone).formatToParts(d);
      let year = 0;
      let month = '01';
      let day = '01';
      let hour = '00';
      let minute = '00';
      let second = '00';
      let weekdayStr = 'Sun';
      for (let i = 0; i < formatted.length; i++) {
        const p = formatted[i];
        if (p.type === 'year') year = parseInt(p.value, 10);
        else if (p.type === 'month') month = p.value;
        else if (p.type === 'day') day = p.value;
        else if (p.type === 'hour') {
          const val = parseInt(p.value, 10) % 24;
          hour = String(val).padStart(2, '0');
        } else if (p.type === 'minute') minute = p.value;
        else if (p.type === 'second') second = p.value;
        else if (p.type === 'weekday') weekdayStr = p.value;
      }
      const weekdays = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
      let weekday = weekdays.indexOf(weekdayStr);
      if (weekday === -1) weekday = 0;
      return { year, month, day, hour, minute, second, weekday };
    }

    const year = d.getFullYear();
    const month = String(d.getMonth() + 1).padStart(2, '0');
    const day = String(d.getDate()).padStart(2, '0');
    const hour = String(d.getHours()).padStart(2, '0');
    const minute = String(d.getMinutes()).padStart(2, '0');
    const second = String(d.getSeconds()).padStart(2, '0');
    const weekday = d.getDay();
    return { year, month, day, hour, minute, second, weekday };
  }

  function dayKey(instant) {
    const p = parts(instant);
    return `${p.year}${p.month}${p.day}`;
  }

  function today() {
    return dayKey(now());
  }

  function getZoneOffsetMinutesAt(date, timeZone) {
    const dtf = new Intl.DateTimeFormat('en-US', {
      timeZone: timeZone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hourCycle: 'h23',
    });
    const formatted = dtf.formatToParts(date);
    let year = 0;
    let month = 1;
    let day = 1;
    let hour = 0;
    let minute = 0;
    let second = 0;
    for (let i = 0; i < formatted.length; i++) {
      const p = formatted[i];
      if (p.type === 'year') year = parseInt(p.value, 10);
      else if (p.type === 'month') month = parseInt(p.value, 10);
      else if (p.type === 'day') day = parseInt(p.value, 10);
      else if (p.type === 'hour') hour = parseInt(p.value, 10) % 24;
      else if (p.type === 'minute') minute = parseInt(p.value, 10);
      else if (p.type === 'second') second = parseInt(p.value, 10);
    }
    const civilUtcMs = Date.UTC(year, month - 1, day, hour, minute, second);
    return Math.round((civilUtcMs - date.getTime()) / 60000);
  }

  function findEarliestCivilInstant(year, month, day, targetHour, targetMinute) {
    if (!_zone) {
      const d = new Date(year, month - 1, day, targetHour, targetMinute, 0, 0);
      if (
        d.getFullYear() === year &&
        d.getMonth() === month - 1 &&
        d.getDate() === day &&
        d.getHours() === targetHour &&
        d.getMinutes() === targetMinute
      ) {
        return d.getTime();
      }
      // If skipped (DST spring-forward), step forward
      let t = new Date(year, month - 1, day, 0, 0, 0, 0).getTime();
      for (let min = 0; min <= 1440; min++) {
        const candidate = new Date(t + min * 60000);
        if (
          candidate.getFullYear() === year &&
          candidate.getMonth() === month - 1 &&
          candidate.getDate() === day
        ) {
          const ch = candidate.getHours();
          const cm = candidate.getMinutes();
          if (ch > targetHour || (ch === targetHour && cm >= targetMinute)) {
            return candidate.getTime();
          }
        }
      }
      return d.getTime();
    }

    // In active zone
    const targetDayStr = String(day).padStart(2, '0');
    const targetMonthStr = String(month).padStart(2, '0');
    const targetHourStr = String(targetHour).padStart(2, '0');
    const targetMinuteStr = String(targetMinute).padStart(2, '0');

    const targetCivil = Date.UTC(year, month - 1, day, targetHour, targetMinute, 0);
    let approxUtc = targetCivil;
    // Iterative offset estimation, on the whole civil date so a month or
    // year boundary between the guess and the target converges too.
    for (let iter = 0; iter < 3; iter++) {
      const p = parts(approxUtc);
      const civil = Date.UTC(
        p.year,
        parseInt(p.month, 10) - 1,
        parseInt(p.day, 10),
        parseInt(p.hour, 10),
        parseInt(p.minute, 10),
        0
      );
      const diffMinutes = Math.round((targetCivil - civil) / 60000);
      if (diffMinutes === 0) break;
      approxUtc += diffMinutes * 60000;
    }

    // Check exact match
    const p = parts(approxUtc);
    if (
      p.year === year &&
      p.month === targetMonthStr &&
      p.day === targetDayStr &&
      p.hour === targetHourStr &&
      p.minute === targetMinuteStr
    ) {
      // In case of fall-back (repeated hour), check if earlier hour exists
      const earlier = approxUtc - 3600000;
      const pe = parts(earlier);
      if (
        pe.year === year &&
        pe.month === targetMonthStr &&
        pe.day === targetDayStr &&
        pe.hour === targetHourStr &&
        pe.minute === targetMinuteStr
      ) {
        return earlier;
      }
      return approxUtc;
    }

    // Skipped instant (spring-forward) or gap: search earliest instant of that day >= target
    let scanStart = Date.UTC(year, month - 1, day - 1, 12, 0, 0);
    let scanEnd = Date.UTC(year, month - 1, day + 1, 12, 0, 0);
    for (let t = scanStart; t <= scanEnd; t += 60000) {
      const pt = parts(t);
      if (pt.year === year && pt.month === targetMonthStr && pt.day === targetDayStr) {
        const ph = parseInt(pt.hour, 10);
        const pm = parseInt(pt.minute, 10);
        if (ph > targetHour || (ph === targetHour && pm >= targetMinute)) {
          return t;
        }
      }
    }
    return approxUtc;
  }

  function dayStart(dayKeyStr) {
    if (!dayKeyStr || String(dayKeyStr).length !== 8) {
      dayKeyStr = today();
    }
    const y = parseInt(dayKeyStr.slice(0, 4), 10);
    const m = parseInt(dayKeyStr.slice(4, 6), 10);
    const d = parseInt(dayKeyStr.slice(6, 8), 10);
    return findEarliestCivilInstant(y, m, d, 0, 0);
  }

  function formatInstant(instant, options) {
    if (instant === null || instant === undefined || instant === '') {
      return 'time unavailable';
    }
    const d = instant instanceof Date ? instant : new Date(instant);
    if (Number.isNaN(d.getTime())) {
      return 'time unavailable';
    }
    const opts = options ? Object.assign({}, options) : {};
    if (_zone) {
      opts.timeZone = _zone;
    }
    // toLocaleString, not a bare DateTimeFormat: with no date or time fields
    // asked for it renders both, as the pages did before they took a zone.
    return d.toLocaleString(undefined, opts);
  }

  function differs() {
    if (!_zone) return false;
    try {
      const currentDate = now();
      const hostTz = Intl.DateTimeFormat().resolvedOptions().timeZone;
      const hostOffset = getZoneOffsetMinutesAt(currentDate, hostTz);
      const journalOffset = getZoneOffsetMinutesAt(currentDate, _zone);
      return hostOffset !== journalOffset;
    } catch (_) {
      return false;
    }
  }

  function noteText(customLabel) {
    const lbl = typeof customLabel === 'string' && customLabel ? customLabel : _label;
    const template =
      (typeof window !== 'undefined' && window.CONVEY_COPY && window.CONVEY_COPY.ZONE_TIME) || '';
    if (!template) return '';
    return template.replace('{label}', lbl);
  }

  function zoneNoteText() {
    return noteText(_label);
  }

  function placeNote(anchor, options) {
    if (!anchor) return;
    const inside = !!(options && options.inside);
    let note = inside
      ? anchor.querySelector('[data-zone-note]')
      : anchor.nextElementSibling &&
        anchor.nextElementSibling.getAttribute &&
        anchor.nextElementSibling.hasAttribute('data-zone-note')
      ? anchor.nextElementSibling
      : null;

    if (differs()) {
      if (!note) {
        note = document.createElement('span');
        note.setAttribute('data-zone-note', '');
        note.className = 'zone-note';
        if (inside) {
          anchor.appendChild(note);
        } else if (anchor.parentNode) {
          anchor.parentNode.insertBefore(note, anchor.nextSibling);
        }
      }
      note.textContent = zoneNoteText();
    } else {
      if (note && note.parentNode) {
        note.parentNode.removeChild(note);
      }
    }
  }

  function addDaysKey(dayKeyStr, numDays) {
    const y = parseInt(dayKeyStr.slice(0, 4), 10);
    const m = parseInt(dayKeyStr.slice(4, 6), 10) - 1;
    const d = parseInt(dayKeyStr.slice(6, 8), 10);
    const res = new Date(y, m, d + numDays);
    const ry = res.getFullYear();
    const rm = String(res.getMonth() + 1).padStart(2, '0');
    const rd = String(res.getDate()).padStart(2, '0');
    return `${ry}${rm}${rd}`;
  }

  function span(dayKeyStr, startMinute, endMinute, offsetSeconds) {
    const y = parseInt(dayKeyStr.slice(0, 4), 10);
    const m = parseInt(dayKeyStr.slice(4, 6), 10);
    const d = parseInt(dayKeyStr.slice(6, 8), 10);

    if (typeof offsetSeconds === 'number') {
      const civilMidnightUtc = Date.UTC(y, m - 1, d, 0, 0, 0) - offsetSeconds * 1000;
      return {
        fromMs: civilMidnightUtc + startMinute * 60000,
        toMs: civilMidnightUtc + endMinute * 60000,
      };
    }

    // Active zone
    let fromMs;
    if (startMinute === 0) {
      fromMs = dayStart(dayKeyStr);
    } else {
      const sh = Math.floor(startMinute / 60);
      const sm = startMinute % 60;
      fromMs = findEarliestCivilInstant(y, m, d, sh, sm);
    }

    let toMs;
    if (endMinute === 1440) {
      const nextKey = addDaysKey(dayKeyStr, 1);
      toMs = dayStart(nextKey);
    } else {
      const eh = Math.floor(endMinute / 60);
      const em = endMinute % 60;
      toMs = findEarliestCivilInstant(y, m, d, eh, em);
    }

    return { fromMs, toMs };
  }

  window.JournalClock = {
    seed,
    setNow,
    now,
    zone,
    label,
    parts,
    dayKey,
    today,
    dayStart,
    formatInstant,
    differs,
    noteText,
    zoneNoteText,
    placeNote,
    span,
  };
})();
