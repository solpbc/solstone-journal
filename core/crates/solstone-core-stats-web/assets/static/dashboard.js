// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Final card states: loading, ready, empty, usage-error, partial, index-error.
(function () {
  const card = document.querySelector('.token-card');
  if (!card) return;
  const rollup = document.querySelector('#tokenRollup');
  const heading = document.querySelector('#tokenDayHeading');
  const summary = document.querySelector('#tokenSummary');
  const dateNavHost = document.querySelector('#statsDateNav');
  const validDay = value => /^\d{8}$/.test(value || '');
  const today = () => { const now = new Date(); return `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`; };
  let selected = validDay(new URLSearchParams(location.search).get('cost')) ? new URLSearchParams(location.search).get('cost') : today();
  const state = value => { card.dataset.tokenCardState = value; };
  const rows = (target, values, label) => { target.replaceChildren(...values.map(item => { const row = document.createElement('tr'); row.innerHTML = `<td>${item[label]}</td><td>${item.requests}</td><td>${item.tokens}</td><td>$${Number(item.cost).toFixed(2)}</td>`; return row; })); };
  let scopedDateNav = null;
  function select(day, push) {
    if (!validDay(day)) return;
    selected = day;
    scopedDateNav?.setDay(day);
    if (push) history.pushState({}, '', `?cost=${day}#cost`);
    fetch(`/app/stats/api/usage?day=${day}`).then(response => { if (!response.ok) throw Error('usage'); return response.json(); }).then(data => {
      heading.textContent = day;
      summary.textContent = `$${Number(data.total.cost).toFixed(2)} · ${data.total.requests} requests`;
      rows(document.querySelector('#tokenProviders'), data.by_provider, 'provider');
      rows(document.querySelector('#tokenModels'), data.by_model, 'model');
      state(data.total.requests === 0 ? 'empty' : data.total.skipped_unknown ? 'partial' : 'ready');
      heading.focus();
      document.querySelectorAll('[data-cost-day]').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.costDay === day)));
    }).catch(() => state('usage-error'));
  }
  scopedDateNav = window.DateNav && window.DateNav.mountScoped({
    host: dateNavHost,
    apiBase: '/app/stats/',
    initialDay: selected,
    onSelect: day => select(day, true)
  });
  function loadMonth(month) { return fetch(`/app/stats/api/stats/${month}`).then(response => response.ok ? response.json() : Promise.reject(Error('month'))); }
  state('loading');
  fetch('/app/stats/api/index').then(response => { if (!response.ok) throw Error('index'); return response.json(); }).then(index => {
    const months = Object.keys(index.months || {});
    return Promise.all(months.map(loadMonth));
  }).then(months => {
    const costs = Object.assign({}, ...months);
    const days = Object.keys(costs).sort().slice(-30);
    const max = Math.max(1, ...days.map(day => Number(costs[day])));
    days.forEach(day => { const button = document.createElement('button'); button.type = 'button'; button.className = 'token-rollup__bar'; button.dataset.costDay = day; button.setAttribute('aria-label', day); button.style.height = `${Math.max(2, Number(costs[day]) / max * 100)}%`; button.addEventListener('click', () => select(day, true)); rollup.append(button); });
    select(selected, false);
  }).catch(() => { state('index-error'); select(selected, false); });
  addEventListener('popstate', () => { const day = new URLSearchParams(location.search).get('cost'); if (validDay(day)) select(day, false); });
}());
