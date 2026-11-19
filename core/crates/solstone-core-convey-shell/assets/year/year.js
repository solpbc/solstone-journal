// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// One calendar year, two ways: each day, and the places and names that run
// through it. Everything drawn is a count of stored records or a date. Rows
// keep the order the read gives them (first appearance), and nothing here
// sorts by size, scores or picks.

(function (global) {
  'use strict';

  const COPY = {
    title: global.JournalI18n.render('journal.window.year.copy.title.year'),
    days: global.JournalI18n.render('journal.window.year.copy.days.days'),
    threads: global.JournalI18n.render('journal.window.year.copy.threads.threads'),
    switchLabel: global.JournalI18n.render('journal.window.year.copy.switch.label.how.to.see.this.year'),
    stepLabel: global.JournalI18n.render('journal.window.year.copy.step.label.other.years'),
    daysLede: global.JournalI18n.render('journal.window.year.copy.days.lede.each.square.is.a.day.colored'),
    layerLabel: global.JournalI18n.render('journal.window.year.copy.layer.label.what.to.count'),
    daysNone: global.JournalI18n.render('journal.window.year.copy.days.none.nothing.of.this.kind.in.year'),
    threadsLede: global.JournalI18n.render('journal.window.year.copy.threads.lede.each.row.is.a.place.or'),
    find: global.JournalI18n.render('journal.window.year.copy.find.find.a.place.or.a.name'),
    findNone: global.JournalI18n.render('journal.window.year.copy.find.none.nothing.in.year.has.that.name'),
    threadsNone: global.JournalI18n.render('journal.window.year.copy.threads.none.no.places.or.names.in.year'),
    threadsNoneListed: global.JournalI18n.render('journal.window.year.copy.threads.none.listed.nothing.listed.for.year'),
    places: global.JournalI18n.render('journal.window.year.copy.places.places'),
    otherNames: global.JournalI18n.render('journal.window.year.copy.other.names.other.names'),
    sectionCount: global.JournalI18n.render('journal.window.year.copy.section.count.count'),
    namesUnavailable: global.JournalI18n.render('journal.window.year.copy.names.unavailable.names.aren.t.listed.your.journal'),
    placesUnavailable: global.JournalI18n.render('journal.window.year.copy.places.unavailable.places.aren.t.listed.the.place'),
    peekHint: global.JournalI18n.render('journal.window.year.copy.peek.hint.choose.a.row.or.a.week'),
    rowSummary: global.JournalI18n.render('journal.window.year.copy.row.summary.days.in.year.first.to.last'),
    rowSummaryOne: global.JournalI18n.render('journal.window.year.copy.row.summary.one.day.in.year.first'),
    rowStart: global.JournalI18n.render('journal.window.year.copy.row.start.start.at.first'),
    weekOf: global.JournalI18n.render('journal.window.year.copy.week.of.week.of.from.to.to'),
    weekDays: global.JournalI18n.render('journal.window.year.copy.week.days.days.of.this.week'),
    weekBefore: global.JournalI18n.render('journal.window.year.copy.week.before.the.earlier.week.with.days.on'),
    weekAfter: global.JournalI18n.render('journal.window.year.copy.week.after.the.later.week.with.days.on'),
    cell: global.JournalI18n.render('journal.window.year.copy.cell.label.week.of.from.days'),
    day: [global.JournalI18n.render('journal.window.year.copy.day.day'), global.JournalI18n.render('journal.window.year.copy.day.days')],
    week: [global.JournalI18n.render('journal.window.year.copy.week.week'), global.JournalI18n.render('journal.window.year.copy.week.weeks')],
    place: [global.JournalI18n.render('journal.window.year.copy.place.place'), global.JournalI18n.render('journal.window.year.copy.place.places')],
    name: [global.JournalI18n.render('journal.window.year.copy.name.name'), global.JournalI18n.render('journal.window.year.copy.name.names')],
    yearNone: global.JournalI18n.render('journal.window.year.copy.year.none.nothing.to.show.here.for.year'),
    empty: global.JournalI18n.render('journal.window.year.copy.empty.nothing.here.yet'),
    emptyDetail: global.JournalI18n.render('journal.window.year.copy.empty.detail.once.your.journal.has.activities.places'),
    emptyAction: global.JournalI18n.render('journal.window.year.copy.empty.action.see.your.sources'),
    error: global.JournalI18n.render('journal.window.year.copy.error.this.didn.t.load'),
    retry: global.JournalI18n.render('journal.window.year.copy.retry.try.again'),
  };

  const PATH = '/app/year/';
  const READ = '/app/year/api/view';
  const SOURCES = '/app/sources/';

  // What a day cell can count. Each is a count its own app already shows,
  // with that app's nouns; a day opens on that app's page for it.
  const LAYERS = [
    { key: 'activities', label: global.JournalI18n.render('journal.window.year.source.or.date.unit.label.activities'), path: '/app/activities', unit: { one: global.JournalI18n.render('journal.window.year.source.or.date.unit.label.activity'), other: global.JournalI18n.render('journal.window.year.source.or.date.unit.label.activities'), none: global.JournalI18n.render('journal.window.year.source.or.date.unit.label.no.activities') } },
    { key: 'location', label: global.JournalI18n.render('journal.window.year.source.or.date.unit.label.location'), path: '/app/map', unit: { one: global.JournalI18n.render('journal.window.year.source.or.date.unit.label.record'), other: global.JournalI18n.render('journal.window.year.source.or.date.unit.label.records'), none: global.JournalI18n.render('journal.window.year.source.or.date.unit.label.no.location') } },
  ];

  const TYPE_LABELS = {
    person: global.JournalI18n.render('journal.window.year.type.labels.person.people'),
    company: global.JournalI18n.render('journal.window.year.type.labels.company.companies'),
    organization: global.JournalI18n.render('journal.window.year.type.labels.organization.organizations'),
    project: global.JournalI18n.render('journal.window.year.type.labels.project.projects'),
    tool: global.JournalI18n.render('journal.window.year.type.labels.tool.tools'),
    place: global.JournalI18n.render('journal.window.year.type.labels.place.named.places'),
  };

  const MONTHS = [global.JournalI18n.render('journal.window.year.months.label.jan'), global.JournalI18n.render('journal.window.year.months.label.feb'), global.JournalI18n.render('journal.window.year.months.label.mar'), global.JournalI18n.render('journal.window.year.months.label.apr'), global.JournalI18n.render('journal.window.year.months.label.may'), global.JournalI18n.render('journal.window.year.months.label.jun'), global.JournalI18n.render('journal.window.year.months.label.jul'), global.JournalI18n.render('journal.window.year.months.label.aug'), global.JournalI18n.render('journal.window.year.months.label.sep'), global.JournalI18n.render('journal.window.year.months.label.oct'), global.JournalI18n.render('journal.window.year.months.label.nov'), global.JournalI18n.render('journal.window.year.months.label.dec')];
  const WEEKDAYS = [
    global.JournalI18n.render('journal.window.date.weekdays.short.sun'),
    global.JournalI18n.render('journal.window.date.weekdays.short.mon'),
    global.JournalI18n.render('journal.window.date.weekdays.short.tue'),
    global.JournalI18n.render('journal.window.date.weekdays.short.wed'),
    global.JournalI18n.render('journal.window.date.weekdays.short.thu'),
    global.JournalI18n.render('journal.window.date.weekdays.short.fri'),
    global.JournalI18n.render('journal.window.date.weekdays.short.sat')
  ];

  function fill(template, values) {
    return String(template).replace(/\{(\w+)\}/g, (_, key) => (key in values ? String(values[key]) : ''));
  }

  function plural(count, words) {
    return `${count} ${count === 1 ? words[0] : words[1]}`;
  }

  function isYear(value) {
    return typeof value === 'string' && /^\d{4}$/.test(value);
  }

  // Day keys are plain calendar dates: UTC arithmetic keeps them free of the
  // browser's zone and of daylight saving.
  function utc(day) {
    return Date.UTC(Number(day.slice(0, 4)), Number(day.slice(4, 6)) - 1, Number(day.slice(6, 8)));
  }

  function dayKey(ms) {
    const date = new Date(ms);
    return String(date.getUTCFullYear()).padStart(4, '0') +
      String(date.getUTCMonth() + 1).padStart(2, '0') +
      String(date.getUTCDate()).padStart(2, '0');
  }

  function weekday(day) {
    return new Date(utc(day)).getUTCDay();
  }

  // Weeks start on Sunday, as the day grid's columns do. Column 0 holds
  // January 1.
  function weekIndex(day) {
    const first = `${day.slice(0, 4)}0101`;
    const offset = Math.round((utc(day) - utc(first)) / 86400000);
    return Math.floor((offset + weekday(first)) / 7);
  }

  function weekCount(year) {
    return weekIndex(`${year}1231`) + 1;
  }

  // The first and last day of a week column that fall inside the year.
  function weekSpan(year, week) {
    const first = `${year}0101`;
    const start = utc(first) + (week * 7 - weekday(first)) * 86400000;
    const from = Math.max(start, utc(first));
    const to = Math.min(start + 6 * 86400000, utc(`${year}1231`));
    return { from: dayKey(from), to: dayKey(to) };
  }

  function shortDate(day) {
    return `${MONTHS[Number(day.slice(4, 6)) - 1]} ${Number(day.slice(6, 8))}`;
  }

  function weekdayDate(day) {
    return `${WEEKDAYS[weekday(day)]} ${Number(day.slice(6, 8))}`;
  }

  function heat(count) {
    if (count <= 0) return 0;
    return 0.15 + 0.85 * (Math.log1p(count) / Math.log1p(7));
  }

  // The view's state is its address's query. Defaults are left out of it.
  function stateFromSearch(search) {
    const params = new URLSearchParams(search || '');
    const year = params.get('year');
    const layer = params.get('layer');
    return {
      year: isYear(year) ? year : null,
      view: params.get('view') === 'threads' ? 'threads' : 'days',
      layer: LAYERS.some((item) => item.key === layer) ? layer : LAYERS[0].key,
    };
  }

  function urlFor(state) {
    const params = new URLSearchParams();
    if (state.year) params.set('year', state.year);
    if (state.view === 'threads') params.set('view', 'threads');
    if (state.layer && state.layer !== LAYERS[0].key) params.set('layer', state.layer);
    const query = params.toString();
    return query ? `${PATH}?${query}` : PATH;
  }

  function readUrl(year) {
    return year ? `${READ}?year=${year}` : READ;
  }

  function thread(kind, id, label, days, year) {
    const weeks = {};
    days.forEach((day) => {
      const week = weekIndex(day);
      (weeks[week] = weeks[week] || []).push(day);
    });
    const held = Object.keys(weeks).map(Number).sort((a, b) => a - b);
    return { kind, id, label, days, weeks, held, year };
  }

  // Places, then one section per kind of name in alphabetical order. The
  // rows keep the order the read gave them.
  function sections(payload) {
    const year = payload.year;
    const out = [];
    const places = (payload.places || []).map((place) => thread('place', `place:${place.id}`, place.label, place.days, year));
    if (places.length) out.push({ key: 'places', title: COPY.places, noun: COPY.place, rows: places });
    const byType = {};
    (payload.names || []).forEach((name) => {
      const key = String(name.type || '').toLowerCase();
      (byType[key] = byType[key] || []).push(thread('name', `name:${key}:${name.name}`, name.name, name.days, year));
    });
    const named = Object.keys(byType).map((key) => ({
      key: `names:${key}`,
      title: TYPE_LABELS[key] || (key ? key : COPY.otherNames),
      noun: COPY.name,
      rows: byType[key],
    }));
    // Kinds of name go by their heading, alphabetically.
    named.sort((a, b) => (a.title < b.title ? -1 : a.title > b.title ? 1 : 0));
    return out.concat(named);
  }

  function gridData(payload, layer) {
    const year = payload.year;
    const end = `${year}1231`;
    return {
      coverage: { start: `${year}0101`, end: payload.today && payload.today < end && payload.today.slice(0, 4) === year ? payload.today : end },
      days: (payload.layers && payload.layers[layer]) || {},
    };
  }

  function el(doc, tag, className, text) {
    const node = doc.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  }

  function setHidden(node, hidden) {
    if (hidden) node.setAttribute('hidden', '');
    else node.removeAttribute('hidden');
  }

  // `key` names the control across redraws, so focus can return to it.
  function link(doc, className, text, href, onGo, key) {
    const node = el(doc, 'a', className, text);
    node.setAttribute('href', href);
    node.setAttribute('data-year-focus', key);
    node.addEventListener('click', (event) => {
      if (event && (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button > 0)) return;
      if (event && typeof event.preventDefault === 'function') event.preventDefault();
      onGo(href);
    });
    return node;
  }

  function drawFailure(doc, root, opts) {
    const box = el(doc, 'div', 'year-state year-error');
    box.setAttribute('role', 'alert');
    box.appendChild(el(doc, 'p', null, COPY.error));
    const retry = el(doc, 'button', 'year-retry', COPY.retry);
    retry.setAttribute('type', 'button');
    retry.addEventListener('click', () => opts.reload());
    box.appendChild(retry);
    root.appendChild(box);
  }

  function drawHead(doc, root, payload, state, opts) {
    const head = el(doc, 'header', 'year-head');
    const title = el(doc, 'h1', 'year-title', payload.year);
    head.appendChild(title);

    const years = payload.years || [];
    const at = years.indexOf(payload.year);
    const before = at > 0 ? years[at - 1] : years.filter((year) => year < payload.year).pop();
    const after = at >= 0 ? years[at + 1] : years.find((year) => year > payload.year);
    if (before || after) {
      const step = el(doc, 'nav', 'year-step');
      step.setAttribute('aria-label', COPY.stepLabel);
      if (before) step.appendChild(link(doc, 'year-step-link', `‹ ${before}`, urlFor({ year: before, view: state.view, layer: state.layer }), opts.go, 'step:before'));
      if (after) step.appendChild(link(doc, 'year-step-link', `${after} ›`, urlFor({ year: after, view: state.view, layer: state.layer }), opts.go, 'step:after'));
      head.appendChild(step);
    }

    const tabs = el(doc, 'nav', 'year-switch');
    tabs.setAttribute('aria-label', COPY.switchLabel);
    [['days', COPY.days], ['threads', COPY.threads]].forEach(([view, label]) => {
      const tab = link(doc, 'year-switch-item', label, urlFor({ year: state.year, view, layer: state.layer }), opts.go, `view:${view}`);
      tab.setAttribute('data-view', view);
      if (state.view === view) tab.setAttribute('aria-current', 'page');
      tabs.appendChild(tab);
    });
    head.appendChild(tabs);
    root.appendChild(head);
  }

  function drawDays(doc, root, payload, state, opts) {
    const section = el(doc, 'section', 'year-days');
    section.appendChild(el(doc, 'p', 'year-lede', COPY.daysLede));

    const chips = el(doc, 'div', 'year-chips');
    chips.setAttribute('role', 'group');
    chips.setAttribute('aria-label', COPY.layerLabel);
    LAYERS.forEach((layer) => {
      const chip = link(doc, 'year-chip', layer.label, urlFor({ year: state.year, view: 'days', layer: layer.key }), opts.go, `layer:${layer.key}`);
      chip.setAttribute('data-layer', layer.key);
      if (state.layer === layer.key) chip.setAttribute('aria-current', 'true');
      chips.appendChild(chip);
    });
    section.appendChild(chips);

    const layer = LAYERS.find((item) => item.key === state.layer) || LAYERS[0];
    const data = gridData(payload, layer.key);
    if (Object.keys(data.days).length === 0) {
      section.appendChild(el(doc, 'p', 'year-none', fill(COPY.daysNone, { year: payload.year })));
    } else {
      // One host for the life of the view: mounting on it again ends the
      // grid that was there, listeners and all.
      const host = opts.gridHost || el(doc, 'div', 'year-grid');
      const legend = el(doc, 'div', 'year-legend');
      section.appendChild(host);
      section.appendChild(legend);
      // The grid needs its host in the page to size and scroll itself.
      root.appendChild(section);
      if (opts.dayGrid) {
        opts.dayGrid.mount(host, { data, mode: 'navigate', appPath: layer.path, unit: layer.unit, today: payload.today });
        opts.dayGrid.legend(legend, { unit: layer.unit, data });
      }
      return;
    }
    root.appendChild(section);
  }

  function cellLabel(row, week) {
    const span = weekSpan(row.year, week);
    return fill(COPY.cell, { label: row.label, from: shortDate(span.from), days: plural(row.weeks[week].length, COPY.day) });
  }

  function drawPeek(doc, peek, row, week, select) {
    peek.textContent = '';
    if (week == null) {
      peek.appendChild(el(doc, 'h3', 'year-peek-title', row.label));
      peek.appendChild(el(doc, 'p', 'year-peek-meta', fill(row.days.length === 1 ? COPY.rowSummaryOne : COPY.rowSummary, {
        days: plural(row.days.length, COPY.day),
        year: row.year,
        first: shortDate(row.days[0]),
        last: shortDate(row.days[row.days.length - 1]),
        weeks: plural(row.held.length, COPY.week),
      })));
      const start = el(doc, 'button', 'year-peek-start', fill(COPY.rowStart, { first: shortDate(row.days[0]) }));
      start.setAttribute('type', 'button');
      start.addEventListener('click', () => select(row, row.held[0], 1));
      peek.appendChild(start);
      return;
    }
    const at = row.held.indexOf(week);
    const step = el(doc, 'span', 'year-peek-step');
    [[-1, '‹', COPY.weekBefore], [1, '›', COPY.weekAfter]].forEach(([delta, glyph, label]) => {
      const button = el(doc, 'button', 'year-peek-step-button', glyph);
      button.setAttribute('type', 'button');
      button.setAttribute('aria-label', label);
      const target = row.held[at + delta];
      button.setAttribute('data-step', String(delta));
      if (target == null) button.setAttribute('disabled', '');
      else button.addEventListener('click', () => select(row, target, delta));
      step.appendChild(button);
    });
    peek.appendChild(step);
    const span = weekSpan(row.year, week);
    peek.appendChild(el(doc, 'p', 'year-peek-when', fill(COPY.weekOf, { from: shortDate(span.from), to: shortDate(span.to) })));
    peek.appendChild(el(doc, 'h3', 'year-peek-title', row.label));
    const days = row.weeks[week];
    const line = el(doc, 'p', 'year-peek-meta', `${fill(COPY.weekDays, { days: plural(days.length, COPY.day) })} `);
    const base = row.kind === 'place' ? '/app/map' : '/app/activities';
    days.forEach((day, index) => {
      if (index) line.appendChild(el(doc, 'span', null, ', '));
      const open = el(doc, 'a', 'year-peek-day', weekdayDate(day));
      open.setAttribute('href', `${base}/${day}`);
      open.setAttribute('data-day', day);
      line.appendChild(open);
    });
    peek.appendChild(line);
  }

  function drawThreads(doc, root, payload, state, opts) {
    const section = el(doc, 'section', 'year-threads');
    section.appendChild(el(doc, 'p', 'year-lede', COPY.threadsLede));
    if (payload.places_unavailable) section.appendChild(el(doc, 'p', 'year-notice', COPY.placesUnavailable));
    if (payload.names_unavailable) section.appendChild(el(doc, 'p', 'year-notice', COPY.namesUnavailable));

    const groups = sections(payload);
    if (!groups.length) {
      const none = payload.places_unavailable || payload.names_unavailable ? COPY.threadsNoneListed : COPY.threadsNone;
      section.appendChild(el(doc, 'p', 'year-none', fill(none, { year: payload.year })));
      root.appendChild(section);
      return;
    }

    const findLabel = el(doc, 'label', 'year-find-label', COPY.find);
    findLabel.setAttribute('for', 'year-find');
    const find = el(doc, 'input', 'year-find');
    find.setAttribute('id', 'year-find');
    find.setAttribute('type', 'search');
    find.setAttribute('autocomplete', 'off');
    section.appendChild(findLabel);
    section.appendChild(find);
    section.appendChild(el(doc, 'p', 'year-hint', COPY.peekHint));

    const columns = weekCount(payload.year);
    const table = el(doc, 'div', 'year-th');
    table.setAttribute('style', `--year-cols:${columns}`);
    const months = el(doc, 'div', 'year-th-head');
    months.setAttribute('aria-hidden', 'true');
    MONTHS.forEach((month, index) => {
      const mark = el(doc, 'span', null, month);
      // A month is marked at the first week that starts inside it.
      const first = `${payload.year}${String(index + 1).padStart(2, '0')}01`;
      const week = weekIndex(first) + (index > 0 && weekday(first) > 0 ? 1 : 0);
      mark.setAttribute('style', `--year-w:${week}`);
      months.appendChild(mark);
    });
    table.appendChild(months);

    const peek = el(doc, 'div', 'year-peek');
    peek.setAttribute('aria-live', 'polite');
    let chosen = null;
    // `toward` is the step a peek button asked for: focus stays on that
    // button, or moves to the other one at the row's end.
    function select(row, week, toward) {
      if (chosen) {
        chosen.row.node.removeAttribute('data-selected');
        chosen.row.labelNode.setAttribute('aria-expanded', 'false');
        if (chosen.cell) chosen.cell.removeAttribute('data-selected');
      }
      row.node.setAttribute('data-selected', '');
      row.labelNode.setAttribute('aria-expanded', 'true');
      const cell = week == null ? null : row.cells[week];
      if (cell) cell.setAttribute('data-selected', '');
      chosen = { row, week, cell };
      drawPeek(doc, peek, row, week, select);
      // The days open under the row they belong to.
      row.node.appendChild(peek);
      if (toward) {
        const buttons = Array.from(peek.querySelectorAll('.year-peek-step-button'));
        const wanted = buttons.find((button) => button.getAttribute('data-step') === String(toward) && !button.hasAttribute('disabled'))
          || buttons.find((button) => !button.hasAttribute('disabled'));
        if (wanted && typeof wanted.focus === 'function') wanted.focus();
      }
    }

    groups.forEach((group) => {
      const heading = el(doc, 'h2', 'year-th-section', `${group.title} `);
      heading.appendChild(el(doc, 'small', null, fill(COPY.sectionCount, { count: plural(group.rows.length, group.noun) })));
      group.heading = heading;
      table.appendChild(heading);
      group.rows.forEach((row) => {
        const node = el(doc, 'div', 'year-th-row');
        node.setAttribute('data-thread', row.id);
        const label = el(doc, 'button', 'year-th-label', row.label);
        label.setAttribute('type', 'button');
        label.setAttribute('title', row.label);
        label.setAttribute('aria-expanded', 'false');
        row.labelNode = label;
        label.addEventListener('click', () => select(row, null));
        // Left and right walk the weeks this row holds.
        label.addEventListener('keydown', (event) => {
          if (!event || (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft')) return;
          if (typeof event.preventDefault === 'function') event.preventDefault();
          const at = chosen && chosen.row === row && chosen.week != null ? row.held.indexOf(chosen.week) : -1;
          const next = event.key === 'ArrowRight' ? Math.min(at + 1, row.held.length - 1) : Math.max(at - 1, 0);
          select(row, row.held[next]);
        });
        node.appendChild(label);
        row.cells = {};
        row.held.forEach((week) => {
          const cell = el(doc, 'button', 'year-th-cell');
          cell.setAttribute('type', 'button');
          cell.setAttribute('tabindex', '-1');
          cell.setAttribute('data-week', String(week));
          cell.setAttribute('aria-label', cellLabel(row, week));
          cell.setAttribute('style', `--year-w:${week};--year-heat:${heat(row.weeks[week].length).toFixed(3)}`);
          cell.addEventListener('click', () => select(row, week));
          row.cells[week] = cell;
          node.appendChild(cell);
        });
        row.node = node;
        table.appendChild(node);
      });
    });

    const none = el(doc, 'p', 'year-none', fill(COPY.findNone, { year: payload.year }));
    setHidden(none, true);
    table.appendChild(none);
    find.addEventListener('input', () => {
      const query = String(find.value || '').trim().toLowerCase();
      let any = false;
      groups.forEach((group) => {
        let shown = 0;
        group.rows.forEach((row) => {
          const on = !query || row.label.toLowerCase().includes(query);
          setHidden(row.node, !on);
          if (on) shown += 1;
        });
        setHidden(group.heading, shown === 0);
        if (shown) any = true;
      });
      setHidden(none, any);
    });

    section.appendChild(table);
    root.appendChild(section);
  }

  // Draws the whole view from what the read answered and the address's state.
  function render(doc, root, read, state, opts) {
    root.textContent = '';
    if (!read || read.status !== 200 || !read.body || typeof read.body !== 'object' || !isYear(read.body.year)) {
      root.appendChild(el(doc, 'h1', 'year-title', COPY.title));
      drawFailure(doc, root, opts);
      return 'error';
    }
    const payload = read.body;
    const shown = { year: payload.year, view: state.view, layer: state.layer };
    const years = payload.years || [];
    if (years.length === 0) {
      root.appendChild(el(doc, 'h1', 'year-title', COPY.title));
      const box = el(doc, 'div', 'year-state year-empty');
      box.appendChild(el(doc, 'p', 'year-empty-heading', COPY.empty));
      box.appendChild(el(doc, 'p', null, COPY.emptyDetail));
      const action = el(doc, 'a', 'year-empty-action', COPY.emptyAction);
      action.setAttribute('href', SOURCES);
      box.appendChild(action);
      root.appendChild(box);
      return 'empty';
    }
    drawHead(doc, root, payload, shown, opts);
    if (years.indexOf(payload.year) === -1) {
      root.appendChild(el(doc, 'p', 'year-none', fill(COPY.yearNone, { year: payload.year })));
      return 'empty';
    }
    if (state.view === 'threads') drawThreads(doc, root, payload, shown, opts);
    else drawDays(doc, root, payload, shown, opts);
    return 'content';
  }

  async function load(opts, year) {
    let status = 0;
    let body = null;
    try {
      const response = await opts.fetch(readUrl(year), { method: 'GET' });
      status = response.status;
      body = await response.json();
    } catch (error) {
      status = 0;
      body = null;
    }
    return { status, body };
  }

  function sameState(a, b, heldYear) {
    return (a.year || heldYear) === (b.year || heldYear) && a.view === b.view && a.layer === b.layer;
  }

  // The view on the contract. Its state is the address's query: a change of
  // view or of what to count draws again from the read it has, and a change
  // of year reads again.
  function mount(ctx) {
    const doc = global.document;
    const root = doc.getElementById('year-root');
    if (!root) throw new Error('year has no root');
    let read = null;
    let readYear;
    let reported = false;
    let asks = 0;
    const gridHost = el(doc, 'div', 'year-grid');
    const heldYear = () => (read && read.body && read.body.year) || null;
    const opts = {
      fetch: ctx.fetch,
      dayGrid: global.DayGrid,
      gridHost,
      go(href) {
        const query = href.indexOf('?') === -1 ? '' : href.slice(href.indexOf('?'));
        const next = stateFromSearch(query);
        // An address that says the same thing takes this entry's place: a
        // second entry for it would make going back look like nothing.
        if (sameState(next, stateFromSearch(global.location.search), heldYear())) {
          ctx.history.replace(href);
          return;
        }
        ctx.history.push(href);
        show();
      },
      reload() {
        read = null;
        show();
      },
    };

    function draw(state) {
      const active = doc.activeElement;
      const key = active && typeof active.getAttribute === 'function' ? active.getAttribute('data-year-focus') : null;
      // A grid on the host ends before the host is drawn into again or left out.
      if (opts.dayGrid) opts.dayGrid.mount(gridHost, { mode: 'select' });
      const readiness = render(doc, root, read, state, opts);
      if (key) {
        const again = Array.from(root.querySelectorAll('[data-year-focus]'))
          .find((node) => node.getAttribute('data-year-focus') === key);
        if (again && typeof again.focus === 'function') again.focus();
      }
      if (!reported) {
        reported = true;
        ctx.ready(readiness);
      }
    }

    function show() {
      const state = stateFromSearch(global.location.search);
      if (read && (readYear === state.year || heldYear() === state.year)) {
        draw(state);
        return;
      }
      // Only the latest ask draws.
      asks += 1;
      const ask = asks;
      load(opts, state.year).then((answer) => {
        if (ctx.disposed || ask !== asks) return;
        read = answer;
        readYear = state.year;
        draw(stateFromSearch(global.location.search));
      }).catch((error) => {
        if (!ctx.disposed) global.logError?.(error, { context: 'year' });
      });
    }

    ctx.on(global, 'popstate', show);
    ctx.onDispose(() => {
      if (opts.dayGrid) opts.dayGrid.mount(gridHost, { mode: 'select' });
    });
    show();
  }

  const api = {
    COPY,
    LAYERS,
    weekIndex,
    weekCount,
    weekSpan,
    stateFromSearch,
    urlFor,
    readUrl,
    sections,
    gridData,
    render,
    load,
    mount,
  };

  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (global.SolViews && typeof global.SolViews.register === 'function') {
    global.SolViews.register('year', { mount });
  }
})(typeof window !== 'undefined' ? window : globalThis);
