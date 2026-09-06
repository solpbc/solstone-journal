// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The selected-day token detail is scoped to stats. It never changes the
// page-wide journal day or navigates to the retired token app.
(function () {
  'use strict';

  const card = document.querySelector('.token-card');
  if (!card) return;

  const rollup = document.querySelector('#tokenRollup');
  const heading = document.querySelector('#tokenDayHeading');
  const summary = document.querySelector('#tokenSummary');
  const status = document.querySelector('#tokenCardStatus');
  const comparison = document.querySelector('#tokenTypeComparison');
  const dateNavHost = document.querySelector('#statsDateNav');
  const tableBodies = {
    providers: document.querySelector('#tokenProviders'),
    models: document.querySelector('#tokenModels')
  };
  const tableData = { providers: [], models: [] };
  const sorts = {
    providers: { key: 'tokens', direction: 'descending' },
    models: { key: 'tokens', direction: 'descending' }
  };
  let coverageFailed = false;
  let selected = initialDay();
  let scopedDateNav = null;
  let usageSequence = 0;

  function validDay(value) {
    return /^\d{8}$/.test(value || '');
  }

  function today() {
    const now = new Date();
    return `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
  }

  function initialDay() {
    const query = new URLSearchParams(location.search).get('tokens');
    return validDay(query) ? query : today();
  }

  function state(value, message) {
    card.dataset.tokenCardState = value;
    status.hidden = !message;
    status.textContent = message || '';
  }

  function number(value) {
    return Number(value || 0).toLocaleString();
  }

  function requestCount(value) {
    const count = Number(value || 0);
    return `${number(count)} ${count === 1 ? 'request' : 'requests'}`;
  }

  function dayLabel(day) {
    return window.JournalFormat.day(day);
  }

  function cell(value) {
    const node = document.createElement('td');
    node.textContent = String(value);
    return node;
  }

  function compareRows(left, right, key, direction) {
    const a = left[key];
    const b = right[key];
    const result = typeof a === 'number' || typeof b === 'number'
      ? Number(a || 0) - Number(b || 0)
      : String(a || '').localeCompare(String(b || ''));
    return direction === 'ascending' ? result : -result;
  }

  function renderTable(name) {
    const body = tableBodies[name];
    const { key, direction } = sorts[name];
    body.replaceChildren();
    [...tableData[name]].sort((left, right) => compareRows(left, right, key, direction)).forEach(item => {
      const row = document.createElement('tr');
      const values = name === 'providers'
        ? [item.provider, item.requests, number(item.tokens), number(item.cached_tokens), `${Number(item.percent || 0).toFixed(1)}%`]
        : [item.model, item.provider, item.requests, number(item.tokens), item.cached_tokens === null ? '—' : number(item.cached_tokens), `${Number(item.percent || 0).toFixed(1)}%`];
      row.append(...values.map(cell));
      body.append(row);
    });
  }

  function renderComparison(byType) {
    comparison.replaceChildren();
    ['generate', 'cogitate'].forEach(kind => {
      const data = byType[kind] || {};
      const section = document.createElement('section');
      section.className = 'token-comparison-card';
      const title = document.createElement('h4');
      title.textContent = kind;
      const detail = document.createElement('p');
      detail.textContent = `${requestCount(data.requests)} · ${number(data.tokens)} tokens`;
      section.append(title, detail);
      comparison.append(section);
    });
  }

  // Tracks whether the last-rendered day had any activity, so the state can
  // be re-evaluated once the (much slower) index coverage check resolves,
  // without re-fetching or re-rendering the day's own data (X-07).
  let lastUsageEmpty = null;

  function applyCardState() {
    if (lastUsageEmpty === null) return; // usage hasn't rendered yet
    if (coverageFailed) {
      state('index-error', "30-day history isn't available. the selected day is still available.");
    } else if (lastUsageEmpty) {
      state('empty', 'no token activity was recorded for this day.');
    } else {
      state('ready', '');
    }
  }

  function renderUsage(data) {
    heading.textContent = window.JournalFormat.day(data.day);
    summary.textContent = `${requestCount(data.total.requests)} · ${number(data.total.tokens)} tokens`;
    tableData.providers = data.by_provider || [];
    tableData.models = data.by_model || [];
    renderTable('providers');
    renderTable('models');
    renderComparison(data.by_type || {});
    lastUsageEmpty = Number(data.total.requests || 0) === 0;
    applyCardState();
  }

  function select(day, options = {}) {
    if (!validDay(day)) return Promise.resolve();
    const sequence = ++usageSequence;
    selected = day;
    if (options.syncNav !== false) scopedDateNav?.setDay(day);
    if (options.push) history.pushState({}, '', `?tokens=${day}#tokens`);
    return fetch(`/app/stats/api/usage?day=${day}`)
      .then(response => {
        if (!response.ok) throw new Error('usage');
        return response.json();
      })
      .then(data => {
        if (sequence !== usageSequence) return;
        renderUsage(data);
        if (options.focus) heading.focus();
        document.querySelectorAll('[data-tokens-day]').forEach(button => {
          button.setAttribute('aria-pressed', String(button.dataset.tokensDay === day));
        });
      })
      .catch(() => {
        if (sequence === usageSequence) {
          state('usage-error', "token activity isn't available for this day.");
        }
      });
  }

  function rollupTotal(models) {
    return Object.values(models || {}).reduce((total, counts) => {
      return total + Number(counts.total_tokens || 0);
    }, 0);
  }

  function renderRollup(byDay) {
    rollup.replaceChildren();
    const days = Object.keys(byDay || {}).sort().slice(-30);
    const totals = Object.fromEntries(days.map(day => [day, rollupTotal(byDay[day])]));
    const max = Math.max(1, ...days.map(day => totals[day]));
    days.forEach(day => {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'token-rollup__bar';
      button.dataset.tokensDay = day;
      button.setAttribute('aria-label', `${dayLabel(day)}: ${number(totals[day])} tokens`);
      button.setAttribute('aria-pressed', String(day === selected));
      // The bar element itself stays full-height (a 44px+ hit target); only
      // the visible fill (--bar-fill, consumed by the CSS gradient) tracks
      // the day's share of the 30-day max (G2-10).
      button.style.setProperty('--bar-fill', `${Math.max(2, totals[day] / max * 100)}%`);
      button.addEventListener('click', () => select(day, { push: true, focus: true }));
      rollup.append(button);
    });
  }

  document.querySelectorAll('.token-sort').forEach(button => {
    button.addEventListener('click', () => {
      const name = button.dataset.tokenTable;
      const key = button.dataset.tokenKey;
      const prior = sorts[name];
      sorts[name] = {
        key,
        direction: prior.key === key && prior.direction === 'ascending' ? 'descending' : 'ascending'
      };
      document.querySelectorAll(`[data-token-table="${name}"]`).forEach(other => {
        other.closest('th').setAttribute('aria-sort', other === button ? sorts[name].direction : 'none');
      });
      renderTable(name);
    });
  });

  scopedDateNav = window.DateNav && window.DateNav.mountScoped({
    host: dateNavHost,
    apiBase: '/app/stats/',
    initialDay: selected,
    onSelect: day => select(day, { push: true, focus: true, syncNav: false })
  });

  document.addEventListener('stats:token-rollup', event => renderRollup(event.detail));

  // The selected day's own data (api/usage) is typically an order of
  // magnitude faster than the 30-day coverage probe (api/index) — fetch them
  // in parallel instead of gating the day's detail behind the slower one, so
  // the card renders as soon as its own data is ready (X-07). The coverage
  // probe only ever narrows the state afterward (to "index-error"); it can't
  // undo a day that already rendered successfully.
  state('loading', 'loading token activity…');
  select(selected);
  fetch('/app/stats/api/index')
    .then(response => {
      if (!response.ok) throw new Error('index');
      return response.json();
    })
    .catch(() => { coverageFailed = true; })
    .then(() => applyCardState());

  addEventListener('popstate', () => {
    const day = new URLSearchParams(location.search).get('tokens');
    select(validDay(day) ? day : today(), { focus: true });
  });
}());
