// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const DAY_RE = /^\d{8}$/;
  const APP_DAY_RE = /^\/app\/[a-z0-9_-]+\/(\d{8})\/?$/;
  const MS_PER_DAY = 24 * 60 * 60 * 1000;
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
  const MONTHS = [
    'January',
    'February',
    'March',
    'April',
    'May',
    'June',
    'July',
    'August',
    'September',
    'October',
    'November',
    'December'
  ];
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

  let mountAbort = null;
  let scopedMount = null;
  let parsedDayInitialized = false;

  const state = {
    appName: null,
    config: null,
    day: null,
    host: null,
    root: null,
    heading: null,
    trigger: null,
    warning: null,
    popover: null,
    grid: null,
    title: null,
    prev: null,
    next: null,
    panelPrev: null,
    panelNext: null,
    open: false,
    zoom: 'days',
    month: null,
    year: null,
    coverage: null,
    months: {},
    indexLoaded: false,
    indexInflight: null,
    indexError: null,
    monthCache: new Map(),
    monthInflight: new Map(),
    warningVisible: false,
    facet: null,
    currentMax: 0
  };

  function parseDayString(value) {
    if (!DAY_RE.test(String(value || ''))) return null;
    const text = String(value);
    const year = Number(text.slice(0, 4));
    const month = Number(text.slice(4, 6));
    const day = Number(text.slice(6, 8));
    const date = new Date(year, month - 1, day);
    if (
      date.getFullYear() !== year ||
      date.getMonth() !== month - 1 ||
      date.getDate() !== day
    ) {
      return null;
    }
    return text;
  }

  function parseDayFromPath(pathname) {
    const match = String(pathname || '').match(APP_DAY_RE);
    return match ? parseDayString(match[1]) : null;
  }

  function DateNavDay() {
    if (parsedDayInitialized) return state.day;
    return parseDayFromPath(window.location.pathname);
  }

  function dateFromDay(day) {
    const normalized = parseDayString(day);
    if (!normalized) return null;
    return new Date(
      Number(normalized.slice(0, 4)),
      Number(normalized.slice(4, 6)) - 1,
      Number(normalized.slice(6, 8))
    );
  }

  function localMidnight(date) {
    return new Date(date.getFullYear(), date.getMonth(), date.getDate());
  }

  function dayDelta(day, now) {
    const target = dateFromDay(day);
    if (!target) return null;
    return Math.round((localMidnight(target) - localMidnight(now)) / MS_PER_DAY);
  }

  function dayString(date) {
    return (
      String(date.getFullYear()) +
      String(date.getMonth() + 1).padStart(2, '0') +
      String(date.getDate()).padStart(2, '0')
    );
  }

  function monthString(date) {
    return (
      String(date.getFullYear()) +
      String(date.getMonth() + 1).padStart(2, '0')
    );
  }

  function addDays(day, delta) {
    const date = dateFromDay(day);
    if (!date) return null;
    date.setDate(date.getDate() + delta);
    return dayString(date);
  }

  function leafZoom() {
    return state.config?.step === 'week' ? 'weeks' : 'days';
  }

  function isWeekStep() {
    return state.config?.step === 'week';
  }

  function sundayOf(day) {
    const date = dateFromDay(day);
    return date ? addDays(day, -date.getDay()) : null;
  }

  function stepDay(day, delta, config) {
    return addDays(day, delta * (config?.step === 'week' ? 7 : 1));
  }

  function addMonths(month, delta) {
    if (!/^\d{6}$/.test(String(month || ''))) return null;
    const date = new Date(Number(month.slice(0, 4)), Number(month.slice(4, 6)) - 1, 1);
    date.setMonth(date.getMonth() + delta);
    return monthString(date);
  }

  function headingLabel(day, now = new Date()) {
    const target = dateFromDay(day);
    if (!target) return '';
    const delta = dayDelta(day, now);
    if (delta === 0) return 'Today';
    if (delta === -1) return 'Yesterday';
    if (delta === 1) return 'Tomorrow';
    if (delta >= -6 && delta <= -2) return `Last ${WEEKDAYS[target.getDay()]}`;

    let label = `${WEEKDAYS[target.getDay()]}, ${MONTHS[target.getMonth()]} ${target.getDate()}`;
    if (target.getFullYear() !== now.getFullYear()) {
      label += `, ${target.getFullYear()}`;
    }
    return label;
  }

  function controlLabel(day, now = new Date()) {
    const target = dateFromDay(day);
    if (!target) return '';
    let label = `${WEEKDAYS_SHORT[target.getDay()]}, ${MONTHS_SHORT[target.getMonth()]} ${target.getDate()}`;
    if (target.getFullYear() !== now.getFullYear()) {
      label += ` '${String(target.getFullYear()).slice(-2)}`;
    }
    return label;
  }

  function weekControlLabel(day, now = new Date()) {
    const target = dateFromDay(day);
    if (!target) return '';
    let label = `week of ${MONTHS_SHORT[target.getMonth()]} ${target.getDate()}`;
    if (target.getFullYear() !== now.getFullYear()) {
      label += ` '${String(target.getFullYear()).slice(-2)}`;
    }
    return label;
  }

  function weekHeadingLabel(day, now = new Date()) {
    const target = dateFromDay(day);
    if (!target) return '';
    const normalized = dayString(target);
    const thisSunday = sundayOf(dayString(now));
    const lastSunday = thisSunday ? addDays(thisSunday, -7) : null;
    if (normalized === thisSunday) return 'This week';
    if (normalized === lastSunday) return 'Last week';
    let label = `week of ${MONTHS_SHORT[target.getMonth()]} ${target.getDate()}`;
    if (target.getFullYear() !== now.getFullYear()) {
      label += `, ${target.getFullYear()}`;
    }
    return label;
  }

  function countLabel(count, unit) {
    const normalized = coerceCount(count);
    const noun = unit || {};
    if (noun.kind === 'currency') {
      if (normalized === 0) return 'nothing spent';
      if (normalized > 0 && normalized < 0.01) return '<$0.01';
      return `$${normalized.toFixed(2)}`;
    }
    if (normalized === 1) return `1 ${noun.one}`;
    if (normalized > 0) return `${normalized} ${noun.other}`;
    return noun.none || '';
  }

  function coerceCount(value) {
    if (value == null) return 0;
    if (typeof value === 'number') return Number.isFinite(value) ? value : 0;
    if (typeof value === 'object' && !Array.isArray(value)) {
      return Object.values(value).reduce((total, item) => {
        const numeric = Number(item);
        return total + (Number.isFinite(numeric) ? numeric : 0);
      }, 0);
    }
    return 0;
  }

  function heatIntensity(value, max) {
    const numeric = coerceCount(value);
    const maximum = coerceCount(max);
    if (numeric <= 0 || maximum <= 0) return 0;
    return 0.15 + 0.85 * (Math.log1p(numeric) / Math.log1p(maximum));
  }

  function yearTotals(months) {
    const totals = {};
    Object.entries(months || {}).forEach(([month, value]) => {
      if (!/^\d{6}$/.test(month)) return;
      const year = month.slice(0, 4);
      totals[year] = (totals[year] || 0) + coerceCount(value);
    });
    return totals;
  }

  function openingMonth(indexPayload, now = new Date()) {
    const end = indexPayload?.coverage?.end;
    if (parseDayString(end)) return end.slice(0, 6);
    return monthString(now);
  }

  function logDateNavError(error, context) {
    if (window.logError) {
      window.logError(error, context);
      return;
    }
    if (window.console && typeof window.console.error === 'function') {
      window.console.error(error, context);
    }
  }

  function apiBase() {
    return `/app/${state.appName || 'transcripts'}/`;
  }

  function indexPayload() {
    return { coverage: state.coverage, months: state.months };
  }

  function normalizeIndexPayload(payload) {
    return {
      coverage: payload?.coverage || null,
      months: payload?.months || {}
    };
  }

  async function fetchIndex(force = false) {
    if (state.indexInflight) return state.indexInflight;
    if (!force && state.indexLoaded) {
      return { data: indexPayload(), error: state.indexError };
    }

    state.indexInflight = (async () => {
      try {
        const payload = normalizeIndexPayload(
          await window.apiJson(`${apiBase()}api/index`)
        );
        state.coverage = payload.coverage;
        state.months = payload.months;
        state.indexLoaded = true;
        state.indexError = null;
        state.warningVisible = false;
        updateLabels();
        return { data: payload, error: null };
      } catch (error) {
        state.indexError = error;
        state.warningVisible = true;
        updateLabels();
        logDateNavError(error, {
          context: 'date-nav:index',
          url: `${apiBase()}api/index`
        });
        return {
          data: state.indexLoaded ? indexPayload() : null,
          error
        };
      } finally {
        state.indexInflight = null;
      }
    })();
    return state.indexInflight;
  }

  async function fetchMonth(month, force = false) {
    if (!/^\d{6}$/.test(String(month || ''))) {
      return { data: {}, error: null };
    }
    if (state.monthInflight.has(month)) return state.monthInflight.get(month);
    if (!force && state.monthCache.has(month)) return state.monthCache.get(month);

    const request = (async () => {
      try {
        const data = await window.apiJson(`${apiBase()}api/stats/${month}`);
        const result = { data: data || {}, error: null };
        state.monthCache.set(month, result);
        state.warningVisible = false;
        updateLabels();
        return result;
      } catch (error) {
        state.warningVisible = true;
        updateLabels();
        const stale = state.monthCache.get(month) || null;
        return { data: stale ? stale.data : {}, error };
      } finally {
        state.monthInflight.delete(month);
      }
    })();
    state.monthInflight.set(month, request);
    return request;
  }

  function prefetchAdjacentMonths(month) {
    const previous = addMonths(month, -1);
    const next = addMonths(month, 1);
    if (previous) fetchMonth(previous).catch(() => {});
    if (next) fetchMonth(next).catch(() => {});
  }

  function getApp(shell, appName) {
    const apps = shell?.apps || window.solShellData?.apps || [];
    return apps.find((app) => app.name === appName) || null;
  }

  function selectedUnit() {
    return state.config?.unit || {};
  }

  function allowFutureDates() {
    return Boolean(state.config?.allow_future);
  }

  function isFutureDay(day, now = new Date()) {
    const delta = dayDelta(day, now);
    return delta !== null && delta > 0;
  }

  function isSelectableFutureDay(day, config = {}, now = new Date()) {
    return Boolean(config?.allow_future) && isFutureDay(day, now);
  }

  function isSelectableFutureMonth(month, now = new Date()) {
    if (!/^\d{6}$/.test(String(month || ''))) return false;
    return compareMonth(month, monthString(now)) >= 0;
  }

  function isSelectableFutureYear(year, now = new Date()) {
    return Number(year) >= now.getFullYear();
  }

  function resetMountState() {
    if (mountAbort) mountAbort.abort();
    mountAbort = null;
    state.appName = null;
    state.config = null;
    state.day = null;
    state.host = null;
    state.root = null;
    state.heading = null;
    state.trigger = null;
    state.warning = null;
    state.popover = null;
    state.grid = null;
    state.title = null;
    state.prev = null;
    state.next = null;
    state.panelPrev = null;
    state.panelNext = null;
    state.open = false;
    state.zoom = 'days';
    state.month = null;
    state.year = null;
    state.coverage = null;
    state.months = {};
    state.indexLoaded = false;
    state.indexInflight = null;
    state.indexError = null;
    state.monthCache = new Map();
    state.monthInflight = new Map();
    state.warningVisible = false;
    state.facet = null;
    state.currentMax = 0;
    parsedDayInitialized = false;
  }

  function renderShell() {
    const stepLabel = isWeekStep() ? 'week' : 'day';
    state.host.innerHTML =
      '<div class="date-nav-content" data-date-nav-root>' +
      '<div class="date-nav-content__bar">' +
      `<button class="date-nav-content__arrow" type="button" data-date-nav-prev aria-label="previous ${stepLabel}">‹</button>` +
      '<button class="date-nav-content__trigger" type="button" data-date-nav-trigger aria-haspopup="dialog" aria-expanded="false">' +
      '<span data-date-nav-label></span>' +
      '<svg class="date-nav-content__icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
      '<rect x="3" y="5" width="18" height="16" rx="3"></rect><path d="M3 10h18M8 3v4M16 3v4"></path></svg>' +
      '<span class="date-nav-content__warning" data-date-nav-warning aria-hidden="true" hidden>!</span>' +
      '</button>' +
      `<button class="date-nav-content__arrow" type="button" data-date-nav-next aria-label="next ${stepLabel}">›</button>` +
      '</div>' +
      '<div class="date-nav-content__popover" data-date-nav-popover role="dialog" hidden>' +
      '<div class="date-nav-content__panelbar">' +
      '<button class="date-nav-content__panel-arrow" type="button" data-date-nav-panel-prev aria-label="previous">‹</button>' +
      '<button class="date-nav-content__panel-title" type="button" data-date-nav-title aria-label="change date level"></button>' +
      '<button class="date-nav-content__today" type="button" data-date-nav-today aria-label="go to today">' +
      '<span class="date-nav-content__today-full">today</span>' +
      '<span class="date-nav-content__today-short" aria-hidden="true">T</span>' +
      '</button>' +
      '<button class="date-nav-content__panel-arrow" type="button" data-date-nav-panel-next aria-label="next">›</button>' +
      '</div>' +
      '<div class="date-nav-content__grid" data-date-nav-grid role="grid"></div>' +
      '</div>' +
      '</div>';

    state.root = state.host.querySelector('[data-date-nav-root]');
    state.trigger = state.host.querySelector('[data-date-nav-trigger]');
    state.warning = state.host.querySelector('[data-date-nav-warning]');
    state.popover = state.host.querySelector('[data-date-nav-popover]');
    state.grid = state.host.querySelector('[data-date-nav-grid]');
    state.title = state.host.querySelector('[data-date-nav-title]');
    state.prev = state.host.querySelector('[data-date-nav-prev]');
    state.next = state.host.querySelector('[data-date-nav-next]');
    state.panelPrev = state.host.querySelector('[data-date-nav-panel-prev]');
    state.panelNext = state.host.querySelector('[data-date-nav-panel-next]');
    updateLabels();
  }

  function updateLabels() {
    const dayless = !state.day;
    if (state.root) state.root.classList.toggle('date-nav-content--dayless', dayless);
    if (state.prev) state.prev.hidden = dayless;
    if (state.next) state.next.hidden = dayless;
    if (state.heading) {
      const heading = state.day ? (isWeekStep() ? weekHeadingLabel(state.day) : headingLabel(state.day)) : '';
      state.heading.textContent = heading;
      state.heading.hidden = !heading;
    }
    const label = state.host?.querySelector('[data-date-nav-label]');
    if (label) {
      label.textContent = dayless
        ? 'pick a day'
        : isWeekStep()
        ? weekControlLabel(state.day)
        : controlLabel(state.day);
    }
    if (state.warning) state.warning.hidden = !state.warningVisible;
    if (state.trigger) {
      state.trigger.setAttribute('aria-expanded', String(state.open));
      state.trigger.classList.toggle('date-nav-content__trigger--warning', state.warningVisible);
    }
    updateArrowState();
  }

  function compareDay(left, right) {
    if (!left || !right) return 0;
    return left.localeCompare(right);
  }

  function compareMonth(left, right) {
    if (!left || !right) return 0;
    return left.localeCompare(right);
  }

  function canNavigateDay(delta) {
    if (!state.day) return false;
    if (delta < 0) {
      if (!state.coverage) return false;
      return compareDay(state.day, state.coverage.start) > 0;
    }
    if (delta > 0) {
      if (allowFutureDates()) return true;
      if (!state.coverage) return false;
      return compareDay(state.day, state.coverage.end) < 0;
    }
    return false;
  }

  function canNavigatePanel(delta) {
    if (state.zoom === 'days' || state.zoom === 'weeks') {
      if (delta > 0 && allowFutureDates()) return true;
      if (!state.coverage) return false;
      const startMonth = state.coverage.start.slice(0, 6);
      const endMonth = state.coverage.end.slice(0, 6);
      if (delta < 0) return compareMonth(state.month, startMonth) > 0;
      if (delta > 0) return compareMonth(state.month, endMonth) < 0;
    }
    if (state.zoom === 'months') {
      if (delta > 0 && allowFutureDates()) return true;
      if (!state.coverage) return false;
      const startYear = Number(state.coverage.start.slice(0, 4));
      const endYear = Number(state.coverage.end.slice(0, 4));
      if (delta < 0) return state.year > startYear;
      if (delta > 0) return state.year < endYear;
    }
    return false;
  }

  function updateArrowState() {
    if (!state.prev || !state.next) return;
    const usePanel = state.open;
    state.prev.disabled = usePanel ? !canNavigatePanel(-1) : !canNavigateDay(-1);
    state.next.disabled = usePanel ? !canNavigatePanel(1) : !canNavigateDay(1);
    if (state.panelPrev && state.panelNext) {
      state.panelPrev.disabled = !canNavigatePanel(-1);
      state.panelNext.disabled = !canNavigatePanel(1);
    }
  }

  function navigateTo(day) {
    const normalized = parseDayString(day);
    if (!normalized || !state.appName) return;
    window.location.href = `/app/${state.appName}/${normalized}`;
  }

  function navigateDay(delta) {
    if (!canNavigateDay(delta)) return;
    const nextDay = stepDay(state.day, delta, state.config);
    if (nextDay) navigateTo(nextDay);
  }

  function movePanel(delta) {
    if (!canNavigatePanel(delta)) return;
    if (state.zoom === 'days' || state.zoom === 'weeks') {
      state.month = addMonths(state.month, delta);
      state.year = Number(state.month.slice(0, 4));
    } else if (state.zoom === 'months') {
      state.year += delta;
      state.month = `${state.year}${String(Number(state.month.slice(4, 6))).padStart(2, '0')}`;
    }
    renderPanel();
  }

  function showPanel() {
    state.open = true;
    state.zoom = leafZoom();
    fetchIndex().then((result) => {
      const payload = result.data || indexPayload();
      state.month = state.day ? state.day.slice(0, 6) : openingMonth(payload);
      state.year = Number(state.month.slice(0, 4));
      renderPanel();
      focusInitialCell();
    });
    renderPanel();
  }

  function hidePanel({ restoreFocus = true } = {}) {
    state.open = false;
    renderPanel();
    if (restoreFocus && state.trigger) state.trigger.focus();
  }

  function togglePanel() {
    if (state.open) hidePanel();
    else showPanel();
  }

  function titleLabel() {
    if (state.zoom === 'years') return 'Years';
    if (state.zoom === 'months') return String(state.year);
    const date = new Date(Number(state.month.slice(0, 4)), Number(state.month.slice(4, 6)) - 1, 1);
    return `${MONTHS[date.getMonth()]} ${date.getFullYear()}`;
  }

  function maxPositive(values) {
    return values.reduce((max, value) => Math.max(max, coerceCount(value)), 0);
  }

  function clearGrid() {
    state.grid.innerHTML = '';
  }

  function renderCell({ value, label, count, selected = false, disabled = false, kind }) {
    const normalized = coerceCount(count);
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'date-nav-content__cell';
    button.dataset.dateNavCell = kind;
    button.dataset.value = value;
    button.setAttribute('role', 'gridcell');
    button.setAttribute('aria-label', `${label}, ${countLabel(normalized, selectedUnit())}`);
    button.textContent = label;
    button.style.setProperty('--intensity', String(heatIntensity(normalized, state.currentMax || 0)));
    button.disabled = disabled;
    button.tabIndex = disabled ? -1 : -1;
    if (selected) button.classList.add('date-nav-content__cell--selected');
    if (normalized <= 0) button.classList.add('date-nav-content__cell--empty');
    // The count belongs in the tooltip + aria-label, not as visible text in
    // every cell — that noise buries the day number and the heat. Week rows
    // (L3) are a list, not a grid, and DO show the meta inline.
    if (kind === 'week') {
      const meta = document.createElement('span');
      meta.className = 'date-nav-content__cell-count';
      meta.textContent = countLabel(normalized, selectedUnit());
      button.appendChild(meta);
    } else {
      button.title = countLabel(normalized, selectedUnit());
    }
    return button;
  }

  function appendRows(cells, columns) {
    clearGrid();
    state.grid.className = `date-nav-content__grid date-nav-content__grid--${columns}`;
    state.grid.dataset.cols = String(columns);
    for (let index = 0; index < cells.length; index += columns) {
      const row = document.createElement('div');
      row.className = 'date-nav-content__row';
      row.setAttribute('role', 'row');
      cells.slice(index, index + columns).forEach((cell) => row.appendChild(cell));
      state.grid.appendChild(row);
    }
    normalizeRovingTabindex();
  }

  function renderYears() {
    const totals = yearTotals(state.months);
    const years = Object.keys(totals).sort();
    if (years.length === 0) years.push(String(new Date().getFullYear()));
    const max = maxPositive(years.map((year) => totals[year] || 0));
    state.currentMax = max;
    appendRows(
      years.map((year) => {
        const count = coerceCount(totals[year]);
        const selectableFuture = allowFutureDates() && isSelectableFutureYear(year);
        return renderCell({
          value: year,
          label: year,
          count,
          selected: Number(year) === state.year,
          disabled: count <= 0 && !selectableFuture,
          kind: 'year'
        });
      }),
      3
    );
  }

  function renderMonths() {
    const cells = [];
    const counts = [];
    for (let month = 1; month <= 12; month += 1) {
      const key = `${state.year}${String(month).padStart(2, '0')}`;
      counts.push(coerceCount(state.months[key]));
    }
    state.currentMax = maxPositive(counts);
    for (let month = 1; month <= 12; month += 1) {
      const key = `${state.year}${String(month).padStart(2, '0')}`;
      const count = coerceCount(state.months[key]);
      const selectableFuture = allowFutureDates() && isSelectableFutureMonth(key);
      cells.push(
        renderCell({
          value: key,
          label: MONTHS_SHORT[month - 1],
          count,
          selected: key === state.month,
          disabled: count <= 0 && !selectableFuture,
          kind: 'month'
        })
      );
    }
    appendRows(cells, 3);
  }

  function renderDays() {
    const month = state.month || openingMonth(indexPayload());
    state.month = month;
    state.year = Number(month.slice(0, 4));
    const cached = state.monthCache.get(month);
    if (!cached) {
      fetchMonth(month).then(() => {
        if (state.open && state.zoom === 'days' && state.month === month) {
          renderPanel();
          prefetchAdjacentMonths(month);
        }
      });
    } else {
      prefetchAdjacentMonths(month);
    }

    const data = cached?.data || {};
    const year = Number(month.slice(0, 4));
    const monthIndex = Number(month.slice(4, 6)) - 1;
    const daysInMonth = new Date(year, monthIndex + 1, 0).getDate();
    const values = [];
    for (let day = 1; day <= daysInMonth; day += 1) {
      values.push(coerceCount(data[`${month}${String(day).padStart(2, '0')}`]));
    }
    state.currentMax = maxPositive(values);

    const cells = [];
    for (let day = 1; day <= daysInMonth; day += 1) {
      const key = `${month}${String(day).padStart(2, '0')}`;
      const count = coerceCount(data[key]);
      const selectableFuture = isSelectableFutureDay(key, state.config);
      cells.push(
        renderCell({
          value: key,
          label: String(day),
          count,
          selected: key === state.day,
          disabled: count <= 0 && !selectableFuture,
          kind: 'day'
        })
      );
    }
    appendRows(cells, 7);
  }

  function renderWeeks() {
    const month = state.month || openingMonth(indexPayload());
    state.month = month;
    state.year = Number(month.slice(0, 4));
    const cached = state.monthCache.get(month);
    if (!cached) {
      fetchMonth(month).then(() => {
        if (state.open && state.zoom === 'weeks' && state.month === month) {
          renderPanel();
          prefetchAdjacentMonths(month);
        }
      });
    } else {
      prefetchAdjacentMonths(month);
    }

    const data = cached?.data || {};
    const year = Number(month.slice(0, 4));
    const monthIndex = Number(month.slice(4, 6)) - 1;
    const daysInMonth = new Date(year, monthIndex + 1, 0).getDate();
    const sundays = [];
    for (let day = 1; day <= daysInMonth; day += 1) {
      const date = new Date(year, monthIndex, day);
      if (date.getDay() === 0) sundays.push(dayString(date));
    }

    state.currentMax = 1;
    const cells = sundays.map((sundayKey) => {
      const date = dateFromDay(sundayKey);
      const present = coerceCount(data[sundayKey]) > 0;
      return renderCell({
        value: sundayKey,
        label: `week of ${MONTHS_SHORT[date.getMonth()]} ${date.getDate()}`,
        count: present ? 1 : 0,
        selected: sundayKey === state.day,
        disabled: !present,
        kind: 'week'
      });
    });
    appendRows(cells, 1);
  }

  function renderPanel() {
    if (!state.popover || !state.grid) return;
    state.popover.hidden = !state.open;
    if (state.trigger) state.trigger.setAttribute('aria-expanded', String(state.open));
    updateArrowState();
    if (!state.open) return;
    if (!state.month) state.month = state.day ? state.day.slice(0, 6) : openingMonth(indexPayload());
    if (!state.year) state.year = Number(state.month.slice(0, 4));
    state.title.textContent = titleLabel();
    if (state.zoom === 'years') renderYears();
    else if (state.zoom === 'months') renderMonths();
    else if (state.zoom === 'weeks') renderWeeks();
    else renderDays();
  }

  function focusableCells() {
    return Array.from(state.grid?.querySelectorAll('[data-date-nav-cell]:not(:disabled)') || []);
  }

  function normalizeRovingTabindex() {
    const cells = focusableCells();
    if (cells.length === 0) return;
    const selected = cells.find((cell) => cell.classList.contains('date-nav-content__cell--selected'));
    cells.forEach((cell) => {
      cell.tabIndex = cell === (selected || cells[0]) ? 0 : -1;
    });
  }

  function focusInitialCell() {
    const selected = state.grid?.querySelector('.date-nav-content__cell--selected:not(:disabled)');
    const target = selected || focusableCells()[0];
    if (target) target.focus();
  }

  function focusGridCell(cells, index) {
    const target = cells[index];
    if (!target) return;
    cells.forEach((cell) => {
      cell.tabIndex = -1;
    });
    target.tabIndex = 0;
    target.focus();
  }

  function gridColumnCount() {
    return Number(state.grid?.dataset.cols) || 7;
  }

  function moveGridFocus(event, offset) {
    const cells = focusableCells();
    const currentIndex = cells.indexOf(event.target);
    if (currentIndex < 0 || cells.length === 0) return;
    event.preventDefault();
    const nextIndex = Math.max(0, Math.min(cells.length - 1, currentIndex + offset));
    focusGridCell(cells, nextIndex);
  }

  function handleGridKeydown(event) {
    if (!event.target.matches('[data-date-nav-cell]')) return;
    if (
      ['ArrowRight', 'ArrowLeft', 'ArrowDown', 'ArrowUp', 'Home', 'End', 'Enter', ' ', 'Escape'].includes(event.key)
    ) {
      event.stopPropagation();
    }
    if (event.key === 'ArrowRight') moveGridFocus(event, 1);
    if (event.key === 'ArrowLeft') moveGridFocus(event, -1);
    if (event.key === 'ArrowDown') moveGridFocus(event, gridColumnCount());
    if (event.key === 'ArrowUp') moveGridFocus(event, -gridColumnCount());
    if (event.key === 'Home') {
      const cells = focusableCells();
      if (cells.length > 0) {
        event.preventDefault();
        focusGridCell(cells, 0);
      }
    }
    if (event.key === 'End') {
      const cells = focusableCells();
      if (cells.length > 0) {
        event.preventDefault();
        focusGridCell(cells, cells.length - 1);
      }
    }
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      event.target.click();
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      hidePanel();
    }
  }

  function handleRootClick(event) {
    const trigger = event.target.closest('[data-date-nav-trigger]');
    if (trigger) {
      togglePanel();
      return;
    }

    if (event.target.closest('[data-date-nav-prev]')) {
      if (state.open) movePanel(-1);
      else navigateDay(-1);
      return;
    }
    if (event.target.closest('[data-date-nav-next]')) {
      if (state.open) movePanel(1);
      else navigateDay(1);
      return;
    }
    if (event.target.closest('[data-date-nav-panel-prev]')) {
      movePanel(-1);
      return;
    }
    if (event.target.closest('[data-date-nav-panel-next]')) {
      movePanel(1);
      return;
    }
    if (event.target.closest('[data-date-nav-today]')) {
      navigateTo(dayString(new Date()));
      return;
    }
    if (event.target.closest('[data-date-nav-title]')) {
      if (state.zoom === 'days' || state.zoom === 'weeks') state.zoom = 'months';
      else if (state.zoom === 'months') state.zoom = 'years';
      renderPanel();
      focusInitialCell();
      return;
    }

    const cell = event.target.closest('[data-date-nav-cell]');
    if (!cell || cell.disabled) return;
    const value = cell.dataset.value;
    if (cell.dataset.dateNavCell === 'year') {
      state.year = Number(value);
      state.month = `${value}01`;
      state.zoom = 'months';
      renderPanel();
      focusInitialCell();
      return;
    }
    if (cell.dataset.dateNavCell === 'month') {
      state.month = value;
      state.year = Number(value.slice(0, 4));
      state.zoom = leafZoom();
      renderPanel();
      focusInitialCell();
      return;
    }
    if (cell.dataset.dateNavCell === 'week') {
      navigateTo(value);
      return;
    }
    if (cell.dataset.dateNavCell === 'day') {
      navigateTo(value);
    }
  }

  function handleDocumentClick(event) {
    if (!state.open || !state.root) return;
    if (!state.root.contains(event.target)) hidePanel({ restoreFocus: false });
  }

  function isTypingTarget(target) {
    if (!target || !target.matches) return false;
    return target.matches('input, textarea, select, [contenteditable="true"]');
  }

  function handleDocumentKeydown(event) {
    if (!state.config || isTypingTarget(event.target)) return;
    if (state.root && state.root.contains(event.target) && event.target.matches('[data-date-nav-cell]')) return;
    if (event.key === 'ArrowLeft') {
      event.preventDefault();
      if (state.open) movePanel(-1);
      else navigateDay(-1);
    }
    if (event.key === 'ArrowRight') {
      event.preventDefault();
      if (state.open) movePanel(1);
      else navigateDay(1);
    }
    if (event.key === 't' || event.key === 'T') {
      event.preventDefault();
      navigateTo(dayString(new Date()));
    }
  }


  function mountContentDateNav(shell, appName) {
    const app = getApp(shell, appName);
    const host = document.querySelector('[data-date-nav]');
    const heading = document.querySelector('[data-date-nav-heading]');
    const config = app?.date_nav || null;

    if (!config) return;
    if (!host) {
      logDateNavError(new Error('content date_nav config missing workspace host'), {
        context: 'date-nav:missing-host',
        app: appName
      });
      return;
    }

    mountAbort = new AbortController();
    state.appName = appName;
    state.config = config;
    state.day = parseDayFromPath(window.location.pathname);
    if (isWeekStep() && state.day) state.day = sundayOf(state.day) || state.day;
    parsedDayInitialized = true;
    state.host = host;
    state.heading = heading;
    state.month = state.day ? state.day.slice(0, 6) : null;
    state.year = state.month ? Number(state.month.slice(0, 4)) : null;

    renderShell();
    state.root.addEventListener('click', handleRootClick, { signal: mountAbort.signal });
    state.root.addEventListener('keydown', handleGridKeydown, { signal: mountAbort.signal });
    document.addEventListener('click', handleDocumentClick, { signal: mountAbort.signal });
    fetchIndex(true).then(() => {
      updateLabels();
      if (state.open) renderPanel();
    });
  }

  function handleWorkspaceMounted(event) {
    const appName = event.detail?.appName || null;
    resetMountState();
    if (!appName) return;
    if (window.solShellData) {
      mountContentDateNav(window.solShellData, appName);
      return;
    }
    if (window.whenShellReady) {
      window.whenShellReady((shell) => {
        mountContentDateNav(shell, appName);
      });
    }
  }

  // Scoped consumers (stats) own a controller. They deliberately do not use
  // the legacy module singleton above, parse a path day, or hard-navigate.
  function createScopedController(options) {
    const apiBase = String(options.apiBase || '').replace(/\/?$/, '/');
    const local = {
      host: options.host,
      apiBase,
      day: parseDayString(options.initialDay),
      onSelect: options.onSelect,
      abort: new AbortController()
    };
    if (!local.host || !local.day || typeof local.onSelect !== 'function') return null;
    local.host.innerHTML =
      '<div class="date-nav-content" data-date-nav-root>' +
      '<button type="button" data-date-nav-prev aria-label="previous day">‹</button>' +
      '<span data-date-nav-label></span>' +
      '<button type="button" data-date-nav-next aria-label="next day">›</button>' +
      '</div>';
    const label = local.host.querySelector('[data-date-nav-label]');
    const render = () => { label.textContent = controlLabel(local.day); };
    const requestJson = path => window.apiJson
      ? window.apiJson(path)
      : fetch(path).then(response => response.ok ? response.json() : Promise.reject(new Error('date-nav request failed')));
    const refreshMonth = () => requestJson(`${local.apiBase}api/index`)
      .then(() => requestJson(`${local.apiBase}api/stats/${local.day.slice(0, 6)}`))
      .catch(() => {});
    const setDay = day => {
      const normalized = parseDayString(day);
      if (!normalized) return null;
      local.day = normalized;
      render();
      refreshMonth();
      return normalized;
    };
    const select = day => {
      const normalized = setDay(day);
      if (!normalized) return;
      local.onSelect(normalized);
    };
    local.host.querySelector('[data-date-nav-prev]').addEventListener('click', () => select(addDays(local.day, -1)), { signal: local.abort.signal });
    local.host.querySelector('[data-date-nav-next]').addEventListener('click', () => select(addDays(local.day, 1)), { signal: local.abort.signal });
    render();
    refreshMonth();
    return { setDay, unmount: () => { local.abort.abort(); local.host.replaceChildren(); } };
  }

  function mountScoped(options) {
    if (scopedMount) return null;
    const controller = createScopedController(options || {});
    if (!controller) return null;
    scopedMount = controller;
    return {
      unmount() {
        controller.unmount();
        if (scopedMount === controller) scopedMount = null;
      },
      setDay(day) {
        controller.setDay(day);
      }
    };
  }

  document.addEventListener('keydown', handleDocumentKeydown);
  document.addEventListener('workspace:mounted', handleWorkspaceMounted);

  window.DateNav = {
    day: DateNavDay,
    parseDayFromPath,
    headingLabel,
    controlLabel,
    heatIntensity,
    yearTotals,
    countLabel,
    coerceCount,
    openingMonth,
    isSelectableFutureDay,
    weekControlLabel,
    weekHeadingLabel,
    stepDay,
    sundayOf,
    mountScoped
  };
})();
