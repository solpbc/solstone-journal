// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  'use strict';

  const MONTH_SHORT = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'
  ];
  const WEEKDAY_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
  const DAY_RE = /^\d{8}$/;
  const DAY_ATTR = 'data-daygrid-date';
  const MIN_AUTOSCROLL_OVERFLOW = 24;
  const stateByHost = new WeakMap();

  function dateNav() {
    const nav = window.DateNav;
    if (
      !nav ||
      typeof nav.heatIntensity !== 'function' ||
      typeof nav.countLabel !== 'function' ||
      typeof nav.coerceCount !== 'function'
    ) {
      throw new Error('DayGrid requires DateNav');
    }
    return nav;
  }

  function dateFromDay(day) {
    if (typeof day !== 'string' || !DAY_RE.test(day)) return null;
    const y = Number(day.slice(0, 4));
    const m = Number(day.slice(4, 6));
    const d = Number(day.slice(6, 8));
    const date = new Date(y, m - 1, d);
    if (
      date.getFullYear() !== y ||
      date.getMonth() !== m - 1 ||
      date.getDate() !== d
    ) {
      return null;
    }
    return date;
  }

  function validDay(day) {
    return typeof day === 'string' && DAY_RE.test(day) && Boolean(dateFromDay(day));
  }

  function dayString(date) {
    return String(date.getFullYear()).padStart(4, '0') +
      String(date.getMonth() + 1).padStart(2, '0') +
      String(date.getDate()).padStart(2, '0');
  }

  function addDays(day, delta) {
    const date = dateFromDay(day);
    if (!date) return null;
    date.setDate(date.getDate() + delta);
    return dayString(date);
  }

  function sundayOf(day) {
    const date = dateFromDay(day);
    if (!date) return null;
    date.setDate(date.getDate() - date.getDay());
    return dayString(date);
  }

  function daysInSpan(start, end) {
    if (!validDay(start) || !validDay(end) || start > end) return 0;
    let count = 1;
    let cursor = start;
    while (cursor !== end) {
      cursor = addDays(cursor, 1);
      if (!cursor) return 0;
      count += 1;
    }
    return count;
  }

  function own(map, key) {
    return Object.prototype.hasOwnProperty.call(map || {}, key);
  }

  function activeDayKeys(data) {
    const nav = dateNav();
    const out = new Set();
    for (const [day, value] of Object.entries(data?.days || {})) {
      if (validDay(day) && nav.coerceCount(value) > 0) out.add(day);
    }
    for (const [day, value] of Object.entries(data?.pending || {})) {
      if (validDay(day) && nav.coerceCount(value) > 0) out.add(day);
    }
    return Array.from(out).sort();
  }

  function gate(data, opts = {}) {
    const minSpanDays = Number(opts.minSpanDays ?? 70);
    const minActiveDays = Number(opts.minActiveDays ?? 14);
    const coverage = data?.coverage || null;
    const span = coverage ? daysInSpan(coverage.start, coverage.end) : 0;
    if (span < minSpanDays) return { ok: false, reason: 'span-too-short' };
    if (activeDayKeys(data).length < minActiveDays) {
      return { ok: false, reason: 'too-few-active-days' };
    }
    return { ok: true, reason: null };
  }

  function scrollTargetDay(data, today) {
    const coverage = data?.coverage || null;
    if (!coverage || !validDay(coverage.start) || !validDay(coverage.end)) return null;
    if (validDay(today) && today >= coverage.start && today <= coverage.end) {
      return today;
    }
    const active = activeDayKeys(data);
    return active.length ? active[active.length - 1] : coverage.end;
  }

  function maxRolledCount(data) {
    const nav = dateNav();
    return Object.values(data?.days || {}).reduce((max, value) => {
      return Math.max(max, nav.coerceCount(value));
    }, 0);
  }

  function todayString(now = new Date()) {
    return dayString(now);
  }

  // The peek and the cell's accessible name are prose, not an instrument reading:
  // they get the shared human ladder (Today / Yesterday / Last Saturday /
  // Saturday, July 11), never a hand-rolled ISO string a screen reader spells out.
  // The ladder is anchored to the mount's own `today`; week mode keeps that prose
  // anchor separate from the Sunday cell that wears the today ring.
  function displayDay(day, today) {
    const now = today ? dateFromDay(today) : null;
    return now ? dateNav().headingLabel(day, now) : dateNav().headingLabel(day);
  }

  function rangeFor(first, second) {
    return first <= second
      ? { from: first, to: second }
      : { from: second, to: first };
  }

  function monthLabel(month) {
    const index = Number(month.slice(4, 6)) - 1;
    return MONTH_SHORT[index];
  }

  function firstOfMonth(day) {
    return `${day.slice(0, 6)}01`;
  }

  function lastOfMonth(day) {
    const date = dateFromDay(firstOfMonth(day));
    if (!date) return null;
    date.setMonth(date.getMonth() + 1);
    date.setDate(0);
    return dayString(date);
  }

  function addMonths(month, delta) {
    if (!/^\d{6}$/.test(String(month || ''))) return null;
    const date = new Date(Number(month.slice(0, 4)), Number(month.slice(4, 6)) - 1, 1);
    date.setMonth(date.getMonth() + delta);
    return String(date.getFullYear()).padStart(4, '0') +
      String(date.getMonth() + 1).padStart(2, '0');
  }

  function joinPath(appPath, value) {
    return `${String(appPath || '').replace(/\/+$/, '')}/${value}`;
  }

  function yearDay(year, suffix) {
    return `${String(year).padStart(4, '0')}${suffix}`;
  }

  function saturdayOf(day) {
    const sunday = sundayOf(day);
    return sunday ? addDays(sunday, 6) : null;
  }

  // A week column belongs to exactly one month: the month of its Saturday. Placing
  // labels by the month's own first/last day instead lets consecutive months claim
  // the same straddling column, and grid resolves that overlap by wrapping the
  // label row onto a second line.
  function monthColumnRanges(block) {
    const ranges = new Map();
    const december = `${String(block.year).padStart(4, '0')}12`;
    let cursor = block.gridStart;
    let index = 1;
    while (cursor && cursor <= block.gridEnd) {
      const saturday = addDays(cursor, 6);
      if (!saturday) break;
      // the block's trailing column can end in January of the next year — keep it
      // with December so the last week of the year still carries a label
      const key = saturday.slice(0, 4) === String(block.year).padStart(4, '0')
        ? saturday.slice(0, 6)
        : december;
      const range = ranges.get(key);
      if (range) range.end = index;
      else ranges.set(key, { start: index, end: index });
      cursor = addDays(cursor, 7);
      index += 1;
    }
    return ranges;
  }

  function renderMonthLabels(block, config) {
    const row = document.createElement('div');
    row.className = 'daygrid-months';
    const linkMonths = config.mode === 'navigate' && config.monthLinks;
    if (!linkMonths) row.setAttribute('aria-hidden', 'true');

    const ranges = monthColumnRanges(block);
    let month = `${String(block.year).padStart(4, '0')}01`;
    const lastMonth = `${String(block.year).padStart(4, '0')}12`;
    while (month <= lastMonth) {
      const range = ranges.get(month);
      if (range) {
        const label = linkMonths
          ? document.createElement('a')
          : document.createElement('span');
        label.textContent = monthLabel(month);
        label.style.gridColumnStart = String(range.start);
        label.style.gridColumnEnd = `span ${range.end - range.start + 1}`;
        if (linkMonths) label.href = joinPath(config.appPath, month);
        row.appendChild(label);
      }
      const next = addMonths(month, 1);
      if (!next || next <= month) break;
      month = next;
    }
    return row;
  }

  // `data` is optional but wanted: the pending key is only drawn when the grid
  // actually holds pending days. A key for a state the surface cannot be in is
  // scaffolding — search has no rollups at all, so it must never advertise one.
  function legend(host, { unit, data, encode } = {}) {
    if (!host) throw new Error('DayGrid.legend requires a host element');
    dateNav();
    host.replaceChildren();
    const mode = encode === 'presence' ? 'presence' : 'heat';

    const root = document.createElement('div');
    root.className = 'daygrid-legend';

    const scale = document.createElement('div');
    scale.className = 'daygrid-legend-scale';
    scale.setAttribute('aria-hidden', 'true');
    if (mode === 'presence') {
      root.classList.add('daygrid-legend--presence');
      scale.classList.add('daygrid-legend-scale--presence');
      const empty = document.createElement('span');
      empty.className = 'daygrid-legend-swatch daygrid-legend-swatch--empty';
      const present = document.createElement('span');
      present.className = 'daygrid-legend-swatch daygrid-legend-swatch--presence';
      // Each swatch is named from the declared nouns. A heat ramp carries its own
      // order (less → more), so bare swatches still read; two presence swatches do
      // not — without words, nothing says which square is the one that happened.
      const emptyLabel = document.createElement('span');
      emptyLabel.textContent = unit?.none || 'none';
      const presentLabel = document.createElement('span');
      presentLabel.textContent = unit?.one || 'yes';
      scale.append(empty, emptyLabel, present, presentLabel);
      root.append(scale);
      host.appendChild(root);
      return root;
    }

    const less = document.createElement('span');
    less.textContent = 'less';
    scale.appendChild(less);
    const empty = document.createElement('span');
    empty.className = 'daygrid-legend-swatch daygrid-legend-swatch--empty';
    scale.appendChild(empty);
    [0.15, 0.45, 0.72, 1].forEach((heat) => {
      const swatch = document.createElement('span');
      swatch.className = 'daygrid-legend-swatch';
      swatch.style.setProperty('--daygrid-heat', String(heat));
      scale.appendChild(swatch);
    });
    const more = document.createElement('span');
    more.textContent = 'more';
    scale.appendChild(more);

    const hasPending = data
      ? Object.values(data.pending || {}).some((value) => dateNav().coerceCount(value) > 0)
      : true;
    if (hasPending) {
      const pending = document.createElement('div');
      pending.className = 'daygrid-legend-pending';
      pending.innerHTML = '<span class="daygrid-legend-pending-mark" aria-hidden="true"></span><span>pending rollup</span>';
      root.append(scale, pending);
    } else {
      root.append(scale);
    }
    host.appendChild(root);
    return root;
  }

  // A worklist grid has two zeros that mean opposite things — nothing happened, or
  // everything happened and is done — so the done one is spoken with the adopter's
  // own activity noun ("26 segments, all named"), never a bare count. The vocabulary
  // stays in the adopter's copy; the component only picks which noun applies.
  function cellLabel(day, count, unit, pending, today, activityCount, activityUnit) {
    const nav = dateNav();
    const normalized = nav.coerceCount(count);
    const activity = nav.coerceCount(activityCount);
    const label = normalized === 0 && activity > 0 && activityUnit
      ? nav.countLabel(activity, activityUnit)
      : nav.countLabel(normalized, unit);
    const suffix = pending ? ', rollup pending' : '';
    return `${displayDay(day, today)}: ${label}${suffix}`;
  }

  function buildCells(data, config) {
    const start = data.coverage.start;
    const end = data.coverage.end;
    const startYear = Number(start.slice(0, 4));
    const endYear = Number(end.slice(0, 4));
    if (!Number.isInteger(startYear) || !Number.isInteger(endYear)) return null;

    const nav = dateNav();
    const maxCount = maxRolledCount(data);
    const blocks = [];
    const focusable = [];

    for (let year = endYear; year >= startYear; year -= 1) {
      const gridStart = sundayOf(yearDay(year, '0101'));
      const gridEnd = saturdayOf(yearDay(year, '1231'));
      if (!gridStart || !gridEnd) return null;

      const block = {
        year,
        gridStart,
        gridEnd,
        cells: [],
        rowItemsByWeekday: [[], [], [], [], [], [], []],
      };

      let cursor = gridStart;
      while (cursor <= gridEnd) {
        const cursorYear = Number(cursor.slice(0, 4));
        const inCoverage = cursorYear === year && cursor >= start && cursor <= end;
        const isRolled = inCoverage && own(data.days, cursor);
        const isPending = inCoverage && own(data.pending, cursor);
        const rawCount = isRolled ? data.days[cursor] : data.pending?.[cursor];
        const count = nav.coerceCount(rawCount);
        const date = dateFromDay(cursor);
        if (!date) return null;
        const weekday = config.granularity === 'week' ? 0 : date.getDay();
        let cell;
        if (!inCoverage) {
          cell = document.createElement('span');
          cell.className = 'daygrid-cell daygrid-cell--pad';
          cell.setAttribute('aria-hidden', 'true');
        } else if ((isRolled || isPending) && count > 0) {
          cell = document.createElement(config.mode === 'select' ? 'button' : 'a');
          cell.className = 'daygrid-cell';
          cell.setAttribute(DAY_ATTR, cursor);
          cell.textContent = String(Number(cursor.slice(6, 8)));
          if (config.mode === 'select') {
            cell.type = 'button';
          } else {
            cell.href = joinPath(config.appPath, cursor);
          }
          if (cursor === config.todayCell) cell.classList.add('daygrid-cell--today');
          if (isRolled) {
            if (config.encode === 'presence') {
              cell.classList.add('daygrid-cell--presence');
            } else {
              cell.classList.add('daygrid-cell--data');
              cell.style.setProperty('--daygrid-heat', String(nav.heatIntensity(count, maxCount)));
            }
          } else {
            cell.classList.add('daygrid-cell--pending');
          }
          const label = cellLabel(
            cursor,
            count,
            config.unit,
            isPending && !isRolled,
            config.today,
            data.activity?.[cursor],
            config.activityUnit
          );
          cell.setAttribute('aria-label', label);
          cell.title = label;
          cell.tabIndex = -1;
        } else {
          cell = document.createElement(config.mode === 'select' ? 'button' : 'span');
          cell.className = 'daygrid-cell daygrid-cell--empty';
          cell.setAttribute(DAY_ATTR, cursor);
          if (config.mode === 'select') {
            cell.type = 'button';
          } else {
            cell.setAttribute('role', 'button');
            cell.setAttribute('aria-disabled', 'true');
          }
          cell.textContent = String(Number(cursor.slice(6, 8)));
          if (cursor === config.todayCell) cell.classList.add('daygrid-cell--today');
          const label = cellLabel(
            cursor,
            0,
            config.unit,
            false,
            config.today,
            data.activity?.[cursor],
            config.activityUnit
          );
          cell.setAttribute('aria-label', label);
          cell.title = label;
          cell.tabIndex = -1;
        }
        const item = { day: cursor, element: cell, inCoverage, block, weekday };
        block.cells.push(item);
        if (inCoverage) {
          block.rowItemsByWeekday[weekday].push(item);
          focusable.push(item);
        }
        cursor = addDays(cursor, config.granularity === 'week' ? 7 : 1);
        if (!cursor) break;
      }
      blocks.push(block);
    }
    return { blocks, focusable };
  }

  function clamp(value, min, max) {
    return Math.max(min, Math.min(max, value));
  }

  function normalizeConfig(options) {
    const config = {
      data: options?.data || null,
      unit: options?.unit || {},
      activityUnit: options?.activityUnit || null,
      mode: options?.mode || 'navigate',
      appPath: options?.appPath || '',
      monthLinks: Boolean(options?.monthLinks),
      onRange: typeof options?.onRange === 'function' ? options.onRange : null,
      today: validDay(options?.today) ? options.today : todayString(),
      granularity: options?.granularity === 'week' ? 'week' : 'day',
      encode: options?.encode === 'presence' ? 'presence' : 'heat',
    };
    config.todayCell = config.granularity === 'week'
      ? sundayOf(config.today) || config.today
      : config.today;
    if (!['navigate', 'select'].includes(config.mode)) {
      throw new Error('DayGrid.mount supports mode "navigate" or "select"');
    }
    if (config.mode === 'navigate' && !config.appPath) {
      throw new Error('DayGrid.mount requires appPath');
    }
    if (config.mode !== 'navigate') config.monthLinks = false;
    return config;
  }

  function renderGutter(block, granularity) {
    const gutter = document.createElement('div');
    gutter.className = 'daygrid-gutter';
    if (granularity === 'week') gutter.classList.add('daygrid-gutter--week');
    gutter.setAttribute('aria-hidden', 'true');

    const year = document.createElement('span');
    year.className = 'daygrid-year-label';
    year.textContent = String(block.year);

    if (granularity === 'week') {
      gutter.append(year);
      return gutter;
    }

    const weekdays = document.createElement('div');
    weekdays.className = 'daygrid-weekdays';
    WEEKDAY_SHORT.forEach((label, index) => {
      const item = document.createElement('span');
      item.className = 'daygrid-weekday-label';
      item.textContent = [1, 3, 5].includes(index) ? label : '';
      weekdays.appendChild(item);
    });

    gutter.append(year, weekdays);
    return gutter;
  }

  function renderBlock(block, config) {
    const row = document.createElement('section');
    row.className = 'daygrid-year-block';
    row.setAttribute('aria-label', `${block.year} calendar`);

    const grid = document.createElement('div');
    grid.className = 'daygrid-year-grid';
    grid.appendChild(renderMonthLabels(block, config));

    const track = document.createElement('div');
    track.className = 'daygrid-track';
    if (config.granularity === 'week') track.classList.add('daygrid-track--week');
    for (const item of block.cells) track.appendChild(item.element);
    grid.appendChild(track);

    row.append(renderGutter(block, config.granularity), grid);
    return row;
  }

  function mount(host, options = {}) {
    if (!host) throw new Error('DayGrid.mount requires a host element');
    dateNav();

    const previous = stateByHost.get(host);
    if (previous) previous.abort.abort();
    host.replaceChildren();

    const config = normalizeConfig(options);
    const data = config.data;
    const coverage = data?.coverage || null;
    if (!coverage || !validDay(coverage.start) || !validDay(coverage.end)) {
      stateByHost.delete(host);
      return null;
    }

    const abort = new AbortController();
    const signal = abort.signal;
    const built = buildCells(data, config);
    if (!built) return null;
    const targetDay = scrollTargetDay(data, config.todayCell);

    const root = document.createElement('div');
    root.className = 'daygrid';
    if (config.granularity === 'week') root.classList.add('daygrid--week');
    root.__dayGridScrollTarget = targetDay || '';

    const scroller = document.createElement('div');
    scroller.className = 'daygrid-scroller';
    scroller.tabIndex = -1;

    const body = document.createElement('div');
    body.className = 'daygrid-body';
    for (const block of built.blocks) body.appendChild(renderBlock(block, config));
    scroller.appendChild(body);

    const peek = document.createElement('div');
    peek.className = 'daygrid-peek';
    peek.hidden = true;
    const live = document.createElement('div');
    live.className = 'daygrid-live';
    live.setAttribute('aria-live', 'polite');
    root.append(scroller, peek, live);
    host.appendChild(root);

    const focusable = built.focusable;
    const byDay = new Map(focusable.map((item) => [item.day, item]));
    let active = byDay.get(targetDay) || focusable[0] || null;
    let peekCell = null;
    let armedCell = null;
    let selectionAnchor = null;
    let previewDay = null;
    let selectedRange = null;
    let dragStartDay = null;
    let suppressNextSelectClick = false;
    const coarsePointer = Boolean(
      window.matchMedia && window.matchMedia('(pointer: coarse)').matches
    );

    function applyTabStop(next, shouldFocus) {
      if (!next) return;
      if (active) active.element.tabIndex = -1;
      active = next;
      active.element.tabIndex = 0;
      if (shouldFocus) active.element.focus();
    }

    function dayForMove(currentDay, key) {
      const date = dateFromDay(currentDay);
      if (!date) return currentDay;
      if (key === 'ArrowLeft') return addDays(currentDay, -7) || currentDay;
      if (key === 'ArrowRight') return addDays(currentDay, 7) || currentDay;
      if (key === 'ArrowUp') return date.getDay() === 0 ? currentDay : addDays(currentDay, -1) || currentDay;
      if (key === 'ArrowDown') return date.getDay() === 6 ? currentDay : addDays(currentDay, 1) || currentDay;
      return currentDay;
    }

    function moveFocus(key) {
      if (!active) return;
      const target = dayForMove(active.day, key);
      applyTabStop(byDay.get(target) || byDay.get(active.day), true);
    }

    function hidePeek() {
      peek.hidden = true;
      peekCell = null;
      armedCell = null;
    }

    function announce(message) {
      live.textContent = '';
      window.setTimeout(() => {
        if (!signal.aborted) live.textContent = message;
      }, 0);
    }

    function syncRangeClasses() {
      const current = selectionAnchor
        ? rangeFor(selectionAnchor, previewDay || selectionAnchor)
        : selectedRange;
      for (const item of focusable) {
        const cell = item.element;
        cell.classList.remove(
          'daygrid-cell--range-endpoint',
          'daygrid-cell--range-inner',
          'daygrid-cell--range-outside'
        );
        if (!current) continue;
        if (item.day < current.from || item.day > current.to) {
          cell.classList.add('daygrid-cell--range-outside');
        } else if (item.day === current.from || item.day === current.to) {
          cell.classList.add('daygrid-cell--range-endpoint');
        } else {
          cell.classList.add('daygrid-cell--range-inner');
        }
      }
    }

    function startAnchor(day) {
      selectionAnchor = day;
      previewDay = day;
      selectedRange = null;
      syncRangeClasses();
      announce(`range starts ${displayDay(day, config.today)}. pick the last day.`);
    }

    function commitRange(range) {
      selectionAnchor = null;
      previewDay = null;
      selectedRange = range;
      syncRangeClasses();
      announce(`showing ${displayDay(range.from, config.today)} to ${displayDay(range.to, config.today)}`);
      if (config.onRange) config.onRange({ ...range });
    }

    function clearPendingAnchor() {
      if (!selectionAnchor) return false;
      selectionAnchor = null;
      previewDay = null;
      syncRangeClasses();
      announce('range cleared');
      if (config.onRange) config.onRange(null);
      return true;
    }

    function clearAppliedRange() {
      if (!selectedRange) return false;
      selectedRange = null;
      syncRangeClasses();
      announce('range cleared');
      if (config.onRange) config.onRange(null);
      return true;
    }

    function updatePreview(day) {
      if (!selectionAnchor) return;
      previewDay = day;
      syncRangeClasses();
    }

    function activateSelectCell(cell) {
      const day = cell.getAttribute(DAY_ATTR);
      if (!day) return;
      if (!selectionAnchor) {
        startAnchor(day);
        return;
      }
      commitRange(rangeFor(selectionAnchor, day));
    }

    function cellAtPoint(event) {
      const node = document.elementFromPoint(event.clientX, event.clientY);
      const cell = node?.closest?.(`.daygrid-cell[${DAY_ATTR}]`);
      return cell && root.contains(cell) ? cell : null;
    }

    function showPeek(cell) {
      const day = cell.getAttribute(DAY_ATTR);
      if (!day) return;
      const isRolled = own(data.days, day);
      const isPending = own(data.pending, day);
      const count = isRolled ? data.days[day] : data.pending?.[day] || 0;
      const label = cellLabel(
        day,
        count,
        config.unit,
        isPending && !isRolled,
        config.today,
        data.activity?.[day],
        config.activityUnit
      );
      const text = document.createElement('span');
      text.textContent = label;
      peek.replaceChildren(text);
      if (config.mode === 'select' && selectionAnchor) {
        const hint = document.createElement('span');
        hint.className = 'daygrid-peek-hint';
        hint.textContent = `finishes the range from ${displayDay(selectionAnchor, config.today)}`;
        peek.appendChild(hint);
      }
      if (cell.matches('a[href]')) {
        const open = document.createElement('a');
        open.className = 'daygrid-peek-open';
        open.href = cell.href;
        open.textContent = 'open →';
        peek.appendChild(open);
      }
      peek.hidden = false;
      peekCell = cell;
      const rootRect = root.getBoundingClientRect();
      const cellRect = cell.getBoundingClientRect();
      const left = clamp(
        cellRect.left + cellRect.width / 2 - rootRect.left,
        12,
        Math.max(12, rootRect.width - 12)
      );
      peek.style.left = `${left}px`;
      const peekHeight = peek.offsetHeight || 0;
      const top = clamp(
        cellRect.top - rootRect.top - peekHeight - 6,
        0,
        Math.max(0, rootRect.height - peekHeight)
      );
      peek.style.top = `${top}px`;
    }

    root.addEventListener('keydown', (event) => {
      const targetCell = event.target.closest(`.daygrid-cell[${DAY_ATTR}]`);
      if (!targetCell) return;
      if (['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) {
        event.preventDefault();
        moveFocus(event.key);
        return;
      }
      if (event.key === 'Home' || event.key === 'End') {
        event.preventDefault();
        const item = byDay.get(event.target.closest(`.daygrid-cell[${DAY_ATTR}]`).getAttribute(DAY_ATTR)) || active;
        const row = item?.block?.rowItemsByWeekday?.[item.weekday] || [];
        const next = event.key === 'Home' ? row[0] : row[row.length - 1];
        applyTabStop(next || item, true);
        return;
      }
      if (
        event.key.toLowerCase() === 't' &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.altKey
      ) {
        const todayItem = byDay.get(config.todayCell);
        if (todayItem) {
          event.preventDefault();
          applyTabStop(todayItem, true);
        }
        return;
      }
      if (
        event.key === 'Enter' ||
        event.key === ' '
      ) {
        if (config.mode === 'select') {
          event.preventDefault();
          activateSelectCell(targetCell);
        } else if (targetCell.closest('.daygrid-cell--empty')) {
          event.preventDefault();
        }
      }
    }, { signal });

    root.addEventListener('click', (event) => {
      if (event.target.closest('.daygrid-peek-open')) return;
      const cell = event.target.closest(`.daygrid-cell[${DAY_ATTR}]`);
      if (!cell) return;
      if (config.mode === 'select') {
        event.preventDefault();
        if (suppressNextSelectClick) {
          suppressNextSelectClick = false;
          return;
        }
        activateSelectCell(cell);
        return;
      }
      if (cell.closest('.daygrid-cell--empty')) {
        event.preventDefault();
        if (coarsePointer && event.detail > 0) {
          armedCell = cell;
          showPeek(cell);
        }
        return;
      }
      if (
        coarsePointer &&
        event.detail > 0 &&
        cell.matches('.daygrid-cell[href]') &&
        armedCell !== cell
      ) {
        event.preventDefault();
        armedCell = cell;
        showPeek(cell);
      }
    }, { signal });

    root.addEventListener('focusin', (event) => {
      const cell = event.target.closest(`.daygrid-cell[${DAY_ATTR}]`);
      if (cell) {
        const item = byDay.get(cell.getAttribute(DAY_ATTR));
        if (item) applyTabStop(item, false);
        updatePreview(cell.getAttribute(DAY_ATTR));
        showPeek(cell);
      }
    }, { signal });

    root.addEventListener('focusout', (event) => {
      if (!root.contains(event.relatedTarget)) hidePeek();
    }, { signal });

    root.addEventListener('mouseover', (event) => {
      const cell = event.target.closest(`.daygrid-cell[${DAY_ATTR}]`);
      if (cell) {
        updatePreview(cell.getAttribute(DAY_ATTR));
        showPeek(cell);
      }
    }, { signal });

    root.addEventListener('mouseout', (event) => {
      if (!root.contains(event.relatedTarget)) hidePeek();
    }, { signal });

    scroller.addEventListener('scroll', () => {
      if (peekCell && !peek.hidden) showPeek(peekCell);
    }, { signal, passive: true });

    if (config.mode === 'select' && !coarsePointer) {
      root.addEventListener('pointerdown', (event) => {
        if (event.button !== 0) return;
        const cell = event.target.closest(`.daygrid-cell[${DAY_ATTR}]`);
        if (!cell) return;
        try {
          root.setPointerCapture(event.pointerId);
        } catch (error) {
          if (event.isTrusted) throw error;
        }
        dragStartDay = cell.getAttribute(DAY_ATTR);
      }, { signal });

      root.addEventListener('pointermove', (event) => {
        if (!dragStartDay) return;
        const cell = cellAtPoint(event);
        if (!cell) return;
        const day = cell.getAttribute(DAY_ATTR);
        if (day !== dragStartDay) {
          if (selectionAnchor !== dragStartDay) startAnchor(dragStartDay);
          updatePreview(day);
          showPeek(cell);
        }
      }, { signal });

      root.addEventListener('pointerup', (event) => {
        if (!dragStartDay) return;
        const startDay = dragStartDay;
        dragStartDay = null;
        const cell = cellAtPoint(event) ||
          event.target.closest(`.daygrid-cell[${DAY_ATTR}]`);
        if (!cell) return;
        const endDay = cell.getAttribute(DAY_ATTR);
        const shouldCommit = endDay !== startDay;
        if (!shouldCommit) return;
        commitRange(rangeFor(startDay, endDay));
        suppressNextSelectClick = true;
        window.setTimeout(() => {
          suppressNextSelectClick = false;
        }, 0);
      }, { signal });

      root.addEventListener('pointercancel', () => {
        dragStartDay = null;
      }, { signal });
    }

    document.addEventListener('keydown', (event) => {
      if (event.key !== 'Escape') return;
      hidePeek();
      if (
        config.mode === 'select' &&
        root.contains(document.activeElement) &&
        (clearPendingAnchor() || clearAppliedRange())
      ) {
        event.preventDefault();
      }
    }, { signal });

    document.addEventListener('pointerdown', (event) => {
      if (!root.contains(event.target)) hidePeek();
    }, { signal });

    applyTabStop(active, false);

    // The grid is wider than a phone and scrolls inside its own container. Say
    // so: a shadow on the sticky gutter once content has slid under it, and an
    // edge fade while there is more to the right.
    function updateOverflowCues() {
      // scrollWidth counts the reserved scrollbar gutter, so it overstates the
      // reachable end by the gutter width. The body's own width does not.
      const overflow = Math.max(0, body.offsetWidth - scroller.clientWidth);
      root.classList.toggle('daygrid--scrolled', scroller.scrollLeft > 1);
      root.classList.toggle('daygrid--more-right', scroller.scrollLeft < overflow - 1);
    }
    scroller.addEventListener('scroll', updateOverflowCues, { signal, passive: true });
    window.addEventListener('resize', updateOverflowCues, { signal });
    updateOverflowCues();

    const targetElement = targetDay ? root.querySelector(`[${DAY_ATTR}="${targetDay}"]`) : null;
    requestAnimationFrame(() => {
      const maxScroll = Math.max(0, scroller.scrollWidth - scroller.clientWidth);
      if (targetElement && maxScroll > MIN_AUTOSCROLL_OVERFLOW) {
        const rawLeft = targetElement.offsetLeft - (scroller.clientWidth / 2) + (targetElement.clientWidth / 2);
        scroller.scrollLeft = clamp(rawLeft, 0, maxScroll);
      }
      updateOverflowCues();
    });

    stateByHost.set(host, { abort });
    return root;
  }

  window.DayGrid = Object.freeze({ mount, legend, gate });
})();
