// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Stats, on the view contract. This script only registers: `mountStats(ctx)`
// draws the view each time it is mounted, and everything it adds outside its
// own markup goes through `ctx`. The functions above the mount draw only into
// the elements they are given and keep nothing between mounts.
(function() {
  'use strict';

  const STATS_URL = '/app/stats/api/stats';

  // Owner copy for the backlog section.
  const BACKLOG_COPY = Object.freeze({
    ACTION_PROCESS_NOW: window.JournalI18n.render('journal.window.stats.backlog.process.now'),
    ACTION_REDO_SCRATCH: window.JournalI18n.render('journal.window.health.backlog.action.redo.scratch'),
    CONFIRM_REDO_SCRATCH: window.JournalI18n.renderEnglish('journal.window.stats.backlog.redo.confirm'),
    QUEUED_FEEDBACK: window.JournalI18n.render('journal.window.stats.backlog.queued'),
    VERDICT_CAUGHT_UP: window.JournalI18n.render('journal.window.stats.backlog.caught.up'),
    VERDICT_CANT_TELL: window.JournalI18n.render('journal.window.stats.backlog.unknown'),
    BUCKET_HEADING: window.JournalI18n.render('journal.window.stats.backlog.stuck.heading'),
    BUCKET_DESCRIPTION: window.JournalI18n.render('journal.window.stats.backlog.stuck.description'),
    DAY_BADGE: window.JournalI18n.render('journal.window.stats.backlog.stuck.badge'),
    REASON_FAILING_STEP: window.JournalI18n.render('journal.window.stats.backlog.repair.fallback'),
    WHY_NEVER_ATTEMPTED: window.JournalI18n.render('journal.window.stats.backlog.reason.unattempted'),
    WHY_FAILED: window.JournalI18n.render('journal.window.stats.backlog.reason.retry'),
    WHY_SENSED_NOT_THOUGHT: window.JournalI18n.render('journal.window.stats.backlog.reason.sensed'),
    CATCHING_UP_DAY: window.JournalI18n.render('journal.window.stats.backlog.catching.up'),
    CATCHING_UP_TAIL: window.JournalI18n.render('journal.window.stats.backlog.automatic'),
  });

  const EXPECTED_SCHEMA_VERSION = 10;

  // Activity slugs are lowercase storage words, but 'ai' is an initialism the
  // owner reads as AI, not as a word. Only tokens listed here are re-cased;
  // everything else stays lowercase sentence case.
  const ACTIVITY_WORD_LABELS = { ai: 'AI' };

  function activityTitle(activityId) {
    return String(activityId)
      .split('_')
      .map(word => ACTIVITY_WORD_LABELS[word] || word)
      .join(' ');
  }

  // Warm replacement for the prior cool blue (#2171b5) used by the input and
  // audio series — gold, from --gold in tokens.css (X-12).
  const WARM_INPUT_COLOR = 'var(--gold)';

  // Stable warm token roles replace the prior unbranded rainbow palette.
  const WARM_CATEGORICAL_BASE = [
    'var(--gold)',
    'var(--orange)',
    'var(--danger)',
    'var(--surface-dark)',
    'var(--success)',
    'var(--ink-soft)'
  ];

  // DOM element factory
  function el(tag, attrs = {}, children = []) {
    const elem = document.createElement(tag);
    Object.entries(attrs).forEach(([k, v]) => {
      if (k === 'className') elem.className = v;
      else if (k === 'innerHTML') elem.innerHTML = v;
      else if (k === 'style' && typeof v === 'object') {
        Object.assign(elem.style, v);
      } else elem.setAttribute(k, v);
    });
    children.forEach(child => {
      if (typeof child === 'string') elem.appendChild(document.createTextNode(child));
      else if (child) elem.appendChild(child);
    });
    return elem;
  }

  function renderEmptyChart(container, { icon, heading, desc }) {
    container.innerHTML = '';
    const wrapper = el('div', {className: 'empty-chart'});
    wrapper.innerHTML = window.SurfaceState.empty({ icon, heading, desc });
    container.appendChild(wrapper);
  }

  // Format byte counts with binary (GiB/MiB/KiB) suffixes, matching
  // Backup/Settings' units for the same on-disk figure (G2-12).
  function fmtBytes(num) {
    const value = Number(num);
    if (value >= 1099511627776) return (value / 1099511627776).toFixed(1) + ' TiB';
    if (value >= 1073741824) return (value / 1073741824).toFixed(1) + ' GiB';
    if (value >= 1048576) return (value / 1048576).toFixed(1) + ' MiB';
    if (value >= 1024) return (value / 1024).toFixed(1) + ' KiB';
    return String(Math.round(value)) + ' B';
  }

  // Format token counts with Bil/Mil suffixes
  function fmtTokens(num) {
    const value = Number(num);
    if (value >= 1e9) {
      return (value / 1e9).toFixed(1) + '\u2009B';
    }
    if (value >= 1e6) {
      return (value / 1e6).toFixed(1) + '\u2009M';
    }
    if (value >= 1e3) {
      return (value / 1e3).toFixed(1) + '\u2009K';
    }
    return String(Math.round(value));
  }

  const SHORT_MONTHS = [
    window.JournalI18n.render('journal.window.date.months.short.jan'),
    window.JournalI18n.render('journal.window.date.months.short.feb'),
    window.JournalI18n.render('journal.window.date.months.short.mar'),
    window.JournalI18n.render('journal.window.date.months.short.apr'),
    window.JournalI18n.render('journal.window.date.months.short.may'),
    window.JournalI18n.render('journal.window.date.months.short.jun'),
    window.JournalI18n.render('journal.window.date.months.short.jul'),
    window.JournalI18n.render('journal.window.date.months.short.aug'),
    window.JournalI18n.render('journal.window.date.months.short.sep'),
    window.JournalI18n.render('journal.window.date.months.short.oct'),
    window.JournalI18n.render('journal.window.date.months.short.nov'),
    window.JournalI18n.render('journal.window.date.months.short.dec'),
  ];

  function fmtDay(raw) {
    // YYYYMMDD or MM/DD.
    const month = raw.length === 8 ? raw.slice(4, 6) : raw.slice(0, 2);
    const day = raw.length === 8 ? raw.slice(6, 8) : raw.slice(3, 5);
    return window.JournalI18n.render('journal.window.stats.chart.day', [
      ['month', String(SHORT_MONTHS[parseInt(month, 10) - 1])],
      ['day', String(parseInt(day, 10))]
    ]);
  }

  function shouldLabel(i, len) {
    return i === 0 || i === len - 1 || (i % 7 === 0 && len - 1 - i >= 4);
  }

  // These daily charts share inspection controls without changing their data.
  function addDailyInspection(container, chart, days) {
    container.classList.add('chart-inspectable');
    chart.setAttribute('role', 'group');
    const bars = Array.from(chart.children);
    const select = el('select', {'aria-label': window.JournalI18n.render('journal.window.stats.chart.inspection.label')});
    days.forEach((day, i) => select.appendChild(el('option', {value: String(i)}, [fmtDay(day)])));
    const readout = el('div', {className: 'chart-day-values', 'aria-live': 'polite', 'aria-atomic': 'true'});
    const inspect = index => {
      bars.forEach((bar, i) => bar.setAttribute('tabindex', i === index ? '0' : '-1'));
      select.value = String(index);
      readout.textContent = bars[index].getAttribute('aria-label');
    };
    bars.forEach((bar, i) => {
      const stack = bar.querySelector('.bar-stack');
      bar.setAttribute('role', 'button');
      bar.setAttribute('aria-label', stack.getAttribute('aria-label') || `${fmtDay(days[i])}: 0`);
      stack.setAttribute('aria-hidden', 'true');
      bar.addEventListener('focus', () => inspect(i));
      bar.addEventListener('click', () => inspect(i));
      bar.addEventListener('mouseenter', () => inspect(i));
      bar.addEventListener('keydown', event => {
        let next = i;
        if (event.key === 'ArrowRight') next = Math.min(i + 1, bars.length - 1);
        else if (event.key === 'ArrowLeft') next = Math.max(i - 1, 0);
        else if (event.key === 'Home') next = 0;
        else if (event.key === 'End') next = bars.length - 1;
        else if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        inspect(next);
        bars[next].focus();
      });
    });
    select.addEventListener('change', () => inspect(Number(select.value)));
    container.appendChild(el('div', {className: 'chart-inspection'}, [el('label', {}, [window.JournalI18n.render('journal.window.stats.chart.inspection.caption'), select]), readout]));
    inspect(bars.length - 1);
  }

  // Create a stat card
  function statCard(title, value, subtitle, color) {
    return el('div', {className: 'stat-card'}, [
      el('h3', {}, [title]),
      el('p', {className: 'stat-value', style: color ? {color} : {}}, [String(value)]),
      el('p', {className: 'stat-subtitle'}, [subtitle])
    ]);
  }

  // Create a progress card
  function progressCard(title, processed, repairable) {
    const total = processed + repairable;
    const pct = total > 0 ? Math.round((processed / total) * 100) : 100;
    return el('div', {className: 'progress-card'}, [
      el('h3', {}, [title]),
      // These are all-time totals, not the 30-day backlog window above —
      // scope the label so the two figures don't read as contradictory (G2-09).
      el('p', {className: 'stat-subtitle'}, [window.JournalI18n.render('journal.window.stats.visible.text.since.your.journal.began')]),
      el('div', {className: 'progress-bar'}, [
        el('div', {
          className: 'progress-fill',
          style: {width: `${pct}%`}
        }, [`${pct}%`])
      ]),
      el('div', {className: 'progress-stats'}, [
        el('span', {}, [window.JournalI18n.render('journal.window.stats.progress.processed', [['count', processed.toLocaleString()]])]),
        el('span', {}, [window.JournalI18n.render('journal.window.stats.progress.pending', [['count', repairable.toLocaleString()]])])
      ])
    ]);
  }

  // Build stacked hours chart (audio + screen)
  function buildStackedHoursChart(container, data) {
    container.innerHTML = ''; // Clear existing content

    if (!data || !data.length) {
      renderEmptyChart(container, {
        icon: window.ConveyIcons.svg('mic-vocal'),
        heading: window.JournalI18n.render('journal.window.stats.chart.hours.empty.heading'),
        desc: window.JournalI18n.render('journal.window.stats.chart.hours.empty.detail')
      });
      return;
    }

    // Calculate max total for scaling
    const maxTotal = Math.max(...data.map(d => d.audio + d.screen)) || 1;

    const chart = el('div', {className: 'bar-chart', role: 'img', 'aria-label': window.JournalI18n.render('journal.window.stats.chart.hours.accessible')});

    data.forEach((d, i) => {
      const total = d.audio + d.screen;
      const height = (total / maxTotal) * 100;
      const bar = el('div', {
        className: 'bar',
        style: {height: `${height}%`, background: 'transparent', overflow: 'visible'}
      });

      // Create stacked segments
      const disk = d.bytes
        ? window.JournalI18n.render('journal.window.stats.chart.hours.disk', [['bytes', fmtBytes(d.bytes)]])
        : '';
      const stackLabel = window.JournalI18n.render('journal.window.stats.chart.hours.day', [
        ['day', fmtDay(d.day)], ['total', total.toFixed(1)], ['audio', d.audio.toFixed(1)],
        ['screen', d.screen.toFixed(1)], ['disk', disk]
      ]);
      const stack = el('div', {className: 'bar-stack', style: {height: '100%'}, 'aria-label': stackLabel});

      // Calculate segment heights as percentages of the bar
      if (total > 0) {
        const audioPct = (d.audio / total) * 100;
        const screenPct = (d.screen / total) * 100;

        // Screen on top
        if (d.screen > 0) {
          stack.appendChild(el('div', {
            className: 'stack-segment screen',
            style: {height: `${screenPct}%`}
          }));
        }
        // Audio on bottom
        if (d.audio > 0) {
          stack.appendChild(el('div', {
            className: 'stack-segment audio',
            style: {height: `${audioPct}%`}
          }));
        }
      }

      bar.appendChild(stack);

      if (total > 0) {
        const formatted = total > 10 ? Math.round(total) : total.toFixed(1);
        bar.appendChild(el('div', {className: 'bar-value'}, [`${formatted}h`]));
        const titleParts = [window.JournalI18n.render('journal.window.stats.chart.hours.tooltip', [
          ['day', d.day], ['audio', d.audio.toFixed(1)], ['screen', d.screen.toFixed(1)]
        ])];
        if (d.bytes) {
          titleParts.push(window.JournalI18n.render('journal.window.stats.chart.hours.tooltip.disk', [['bytes', fmtBytes(d.bytes)]]));
        }
        bar.dataset.tip = titleParts.join(', ');
      }

      if (shouldLabel(i, data.length)) {
        bar.appendChild(el('div', {className: 'bar-label'}, [fmtDay(d.day)]));
      }

      chart.appendChild(bar);
    });

    container.appendChild(chart);

    // Add legend
    const legend = el('div', {className: 'token-legend'}, [
      el('div', {className: 'legend-item'}, [
        el('div', {className: 'legend-color', style: {background: WARM_INPUT_COLOR}, 'aria-hidden': 'true'}),
        window.JournalI18n.render('journal.window.stats.chart.hours.audio')
      ]),
      el('div', {className: 'legend-item'}, [
        el('div', {
          className: 'legend-color',
          style: {
            background: 'var(--orange)',
            backgroundImage: 'repeating-linear-gradient(45deg, transparent, transparent 3px, color-mix(in srgb, var(--paper) 30%, transparent) 3px, color-mix(in srgb, var(--paper) 30%, transparent) 6px)'
          },
          'aria-hidden': 'true'
        }),
        window.JournalI18n.render('journal.window.stats.chart.hours.screen')
      ])
    ]);
    container.appendChild(legend);
    addDailyInspection(container, chart, data.map(d => d.day));
  }

  const HEATMAP_DAYS = [
    window.JournalI18n.render('journal.window.date.weekdays.short.mon'),
    window.JournalI18n.render('journal.window.date.weekdays.short.tue'),
    window.JournalI18n.render('journal.window.date.weekdays.short.wed'),
    window.JournalI18n.render('journal.window.date.weekdays.short.thu'),
    window.JournalI18n.render('journal.window.date.weekdays.short.fri'),
    window.JournalI18n.render('journal.window.date.weekdays.short.sat'),
    window.JournalI18n.render('journal.window.date.weekdays.short.sun'),
  ];

  // The four anchors an owner reads a 24-column axis by. Every other column
  // stays a blank tick: 24 labels don't fit, and a bare 0-23 grid is the one
  // place on the page the owner has to translate a clock (G2-B14).
  const HEATMAP_HOUR_LABELS = {0: window.JournalI18n.render('journal.window.stats.heatmap.midnight'), 6: window.JournalI18n.render('journal.window.stats.heatmap.six.am'), 12: window.JournalI18n.render('journal.window.stats.heatmap.noon'), 18: window.JournalI18n.render('journal.window.stats.heatmap.six.pm')};

  function heatmapHourSpoken(hour) {
    if (hour === 0) return window.JournalI18n.render('journal.window.stats.heatmap.midnight');
    if (hour === 12) return window.JournalI18n.render('journal.window.stats.heatmap.noon');
    return hour < 12 ? window.JournalI18n.render('journal.window.stats.heatmap.hour.am', [['hour', String(hour)]]) : window.JournalI18n.render('journal.window.stats.heatmap.hour.pm', [['hour', String(hour - 12)]]);
  }

  // One line of the same information the grid carries, for the widths where
  // the grid is hidden (G2-B06). Only claims what the data says: the single
  // busiest hour on weekdays and on weekends.
  function heatmapNarrowNote(data) {
    function peak(rows) {
      let best = -1;
      let hour = null;
      for (let h = 0; h < 24; h++) {
        const total = rows.reduce((sum, d) => sum + (Number(data[d][h]) || 0), 0);
        if (total > best) {
          best = total;
          hour = h;
        }
      }
      return best > 0 ? hour : null;
    }
    const weekday = peak([0, 1, 2, 3, 4]);
    const weekend = peak([5, 6]);
    if (weekday === null && weekend === null) {
      return window.JournalI18n.render('journal.window.stats.heatmap.narrow.empty');
    }
    const parts = [];
    if (weekday !== null) parts.push(window.JournalI18n.render('journal.window.stats.heatmap.narrow.weekday', [['hour', String(heatmapHourSpoken(weekday))]]));
    if (weekend !== null) parts.push(window.JournalI18n.render('journal.window.stats.heatmap.narrow.weekend', [['hour', String(heatmapHourSpoken(weekend))]]));
    return window.JournalI18n.render('journal.window.stats.heatmap.narrow.peak', [['hours', String(parts.join(window.JournalI18n.render('journal.window.stats.heatmap.narrow.join')))]]);
  }

  // Arrow-key roving across the 168 cells. Before this, every cell carried
  // tabindex -1 and nothing in the section was focusable, so the values were
  // reachable by mouse only and the stylesheet's :focus-visible tooltip rule
  // could never fire (G2-B06).
  function bindHeatmapKeys(heatmap) {
    heatmap.addEventListener('keydown', event => {
      const cell = event.target.closest && event.target.closest('.heatmap-cell');
      if (!cell) return;
      const cells = Array.from(heatmap.querySelectorAll('.heatmap-cell'));
      const index = cells.indexOf(cell);
      if (index < 0) return;
      let day = Math.floor(index / 24);
      let hour = index % 24;
      switch (event.key) {
        case 'ArrowLeft': hour = Math.max(0, hour - 1); break;
        case 'ArrowRight': hour = Math.min(23, hour + 1); break;
        case 'ArrowUp': day = Math.max(0, day - 1); break;
        case 'ArrowDown': day = Math.min(6, day + 1); break;
        case 'Home': hour = 0; break;
        case 'End': hour = 23; break;
        default: return;
      }
      const next = cells[day * 24 + hour];
      if (!next || next === cell) {
        event.preventDefault();
        return;
      }
      event.preventDefault();
      cells.forEach(other => { other.tabIndex = -1; });
      next.tabIndex = 0;
      next.focus();
    });
  }

  // Build heatmap
  function buildHeatmap(container, data) {
    const days = HEATMAP_DAYS;
    const maxVal = Math.max(...data.flat()) || 1;

    const heatmap = el('div', {className: 'heatmap', role: 'grid', 'aria-label': window.JournalI18n.render('journal.window.stats.heatmap.accessible')});

    // Hour headers, in their own row so the grid's rows are real elements
    const header = el('div', {className: 'heatmap-row heatmap-header', role: 'row'});
    header.appendChild(el('div'));
    for (let h = 0; h < 24; h++) {
      const label = HEATMAP_HOUR_LABELS[h] || '';
      header.appendChild(el('div', {
        className: 'heatmap-hour',
        role: 'columnheader',
        'aria-label': heatmapHourSpoken(h)
      }, [label]));
    }
    heatmap.appendChild(header);

    // Days with cells
    for (let d = 0; d < 7; d++) {
      const row = el('div', {className: 'heatmap-row', role: 'row'});
      row.appendChild(el('div', {className: 'heatmap-label', role: 'rowheader'}, [days[d]]));

      for (let h = 0; h < 24; h++) {
        const intensity = data[d][h] / maxVal;
        const percent = (intensity * 100).toFixed(2);
        // One duration ladder for the whole app; it grew an hours rung for
        // these buckets, which run to thousands of minutes (G2-B14, S-3). An
        // empty hour is not a duration, so it says so rather than reading
        // "0 sec" (S-8).
        const bucketMinutes = Number(data[d][h]) || 0;
        const activity = bucketMinutes > 0
          ? window.JournalFormat.duration(bucketMinutes * 60)
          : window.JournalI18n.render('journal.window.stats.heatmap.empty');
        const cellTitle = window.JournalI18n.render('journal.window.stats.heatmap.cell', [
          ['day', days[d]], ['hour', heatmapHourSpoken(h)], ['activity', activity]
        ]);
        row.appendChild(el('div', {
          className: 'heatmap-cell',
          style: {background: `color-mix(in srgb, var(--orange) ${percent}%, transparent)`},
          'data-tip': cellTitle,
          'aria-label': cellTitle,
          role: 'gridcell',
          tabindex: d === 0 && h === 0 ? '0' : '-1'
        }));
      }
      heatmap.appendChild(row);
    }

    bindHeatmapKeys(heatmap);
    // A live refresh calls this again with the same container: replace, never
    // append, or the page stacks grids and every stale one keeps a tab stop
    // (F-11).
    container.replaceChildren(heatmap);

    const note = document.getElementById('heatmapNarrowNote');
    if (note) note.textContent = heatmapNarrowNote(data);
  }

  // Generate stable CSS colors for repeated cycles of the warm categorical
  // palette while keeping every paint tied to an existing token.
  function getCategoryColor(index, total) {
    const cycle = Math.floor(index / WARM_CATEGORICAL_BASE.length);
    const base = WARM_CATEGORICAL_BASE[index % WARM_CATEGORICAL_BASE.length];
    if (cycle === 0) return base;
    const percent = Math.max(25, 100 - Math.ceil(cycle / 2) * 18);
    const target = cycle % 2 === 1 ? 'var(--surface-dark)' : 'var(--paper)';
    return `color-mix(in srgb, ${base} ${percent}%, ${target})`;
  }

  // Reserved runtime placeholders (e.g. __RUNTIME_FACETS__) that a producer
  // occasionally writes onto a record when prompt substitution didn't run.
  // They are not a facet the owner made; fold their counts into an
  // "unassigned" bucket instead of charting the raw token (G2-30). The
  // bucket's own key is a leading-underscore form the write path cannot
  // produce, so an owner facet genuinely named "unassigned" keeps its own
  // counts and its own title instead of silently absorbing placeholders.
  const RESERVED_FACET_KEY = /^__.*__$/;
  const UNASSIGNED_FACET_KEY = '__unassigned';

  function foldReservedFacetKeys(countsByDay) {
    const result = {};
    Object.entries(countsByDay || {}).forEach(([day, counts]) => {
      const dayCounts = {};
      Object.entries(counts || {}).forEach(([key, value]) => {
        const target = RESERVED_FACET_KEY.test(key) ? UNASSIGNED_FACET_KEY : key;
        dayCounts[target] = (dayCounts[target] || 0) + value;
      });
      result[day] = dayCounts;
    });
    return result;
  }

  // Build stacked category chart (for Activities or Facets)
  function buildStackedCategoryChart(container, countsByDay, meta = {}) {
    container.innerHTML = '';

    if (!countsByDay || Object.keys(countsByDay).length === 0) {
      renderEmptyChart(container, {
        icon: meta.emptyIcon,
        heading: window.JournalI18n.render('journal.window.stats.chart.empty'),
        desc: meta.emptyText || window.JournalI18n.render('journal.window.stats.chart.empty')
      });
      return;
    }

    // Get last 30 days sorted
    const days = Object.keys(countsByDay).sort().slice(-30);
    if (!days.length) {
      renderEmptyChart(container, {
        icon: meta.emptyIcon,
        heading: window.JournalI18n.render('journal.window.stats.chart.empty'),
        desc: meta.emptyText || window.JournalI18n.render('journal.window.stats.chart.empty')
      });
      return;
    }

    // Collect all unique categories across all days
    const allCategories = new Set();
    days.forEach(day => {
      Object.keys(countsByDay[day] || {}).forEach(cat => allCategories.add(cat));
    });
    const categories = Array.from(allCategories).sort();

    if (!categories.length) {
      renderEmptyChart(container, {
        icon: meta.emptyIcon,
        heading: window.JournalI18n.render('journal.window.stats.chart.empty'),
        desc: meta.emptyText || window.JournalI18n.render('journal.window.stats.chart.empty')
      });
      return;
    }

    // Assign colors to categories
    const categoryColors = {};
    categories.forEach((cat, i) => {
      const info = meta[cat] || {};
      categoryColors[cat] = info.color || getCategoryColor(i, categories.length);
    });

    // Calculate max total for scaling
    let maxTotal = 0;
    const chartData = days.map(day => {
      const dayCounts = countsByDay[day] || {};
      const total = Object.values(dayCounts).reduce((sum, c) => sum + c, 0);
      maxTotal = Math.max(maxTotal, total);
      return { day, counts: dayCounts, total };
    });

    if (maxTotal === 0) {
      renderEmptyChart(container, {
        icon: meta.emptyIcon,
        heading: window.JournalI18n.render('journal.window.stats.chart.empty'),
        desc: meta.emptyText || window.JournalI18n.render('journal.window.stats.chart.empty')
      });
      return;
    }

    const chart = el('div', {className: 'bar-chart', role: 'img', 'aria-label': meta.ariaLabel || ''});

    chartData.forEach((d, i) => {
      const height = (d.total / maxTotal) * 100;
      const bar = el('div', {
        className: 'bar',
        style: {height: `${height}%`, background: 'transparent', overflow: 'visible'}
      });

      // Create stacked segments
      const stack = el('div', {className: 'bar-stack', style: {height: '100%'}});

      // Build tooltip showing breakdown
      const tooltipParts = [`${d.day.slice(4, 6)}/${d.day.slice(6, 8)}`];

      if (d.total > 0) {
        // Stack segments from top to bottom (reverse order for visual stacking)
        categories.slice().reverse().forEach(cat => {
          const count = d.counts[cat] || 0;
          if (count > 0) {
            const pct = (count / d.total) * 100;
            const info = meta[cat] || {};
            const title = info.title || cat;
            stack.appendChild(el('div', {
              className: 'stack-segment',
              style: {
                height: `${pct}%`,
                background: categoryColors[cat]
              }
            }));
            tooltipParts.push(`${title}: ${count}`);
          }
        });
        const catParts = categories.filter(cat => (d.counts[cat] || 0) > 0).map(cat => {
          const info = meta[cat] || {};
          return `${info.title || cat}: ${d.counts[cat]}`;
        });
        stack.setAttribute('aria-label', `${fmtDay(d.day)}: ${d.total} (${catParts.join(', ')})`);
      }

      bar.appendChild(stack);

      if (d.total > 0) {
        bar.appendChild(el('div', {className: 'bar-value'}, [String(d.total)]));
        bar.dataset.tip = tooltipParts.join('\n');
      }

      if (shouldLabel(i, chartData.length)) {
        bar.appendChild(el('div', {className: 'bar-label'}, [fmtDay(d.day)]));
      }

      chart.appendChild(bar);
    });

    container.appendChild(chart);

    // Add legend
    const legend = el('div', {className: 'token-legend'});
    categories.forEach(cat => {
      const info = meta[cat] || {};
      const title = info.title || cat;
      legend.appendChild(el('div', {className: 'legend-item'}, [
        el('div', {className: 'legend-color', style: {background: categoryColors[cat]}, 'aria-hidden': 'true'}),
        title
      ]));
    });
    container.appendChild(legend);
    addDailyInspection(container, chart, days);
  }

  function backlogCopy() {
    return BACKLOG_COPY;
  }

  function count(value) {
    const num = Number(value);
    return Number.isFinite(num) && num > 0 ? num : 0;
  }

  function backlogCounts(stats) {
    const bl = stats.backlog;
    const totals = stats.totals || {};
    return {
      pending: count(bl ? bl.pending_days : totals.backlog_pending_days),
      stuck: count(bl ? bl.stuck_days : totals.backlog_stuck_days)
    };
  }

  function backlogDepth(day) {
    return count(day.segments) + count(day.units);
  }

  function whyLabel(why, C) {
    if (why === 'failed') return C.WHY_FAILED;
    if (why === 'never_attempted') return C.WHY_NEVER_ATTEMPTED;
    if (why === 'sensed_not_thought') return C.WHY_SENSED_NOT_THOUGHT;
    return null;
  }

  // Per-unit reason_code copy, more specific than the generic why-bucket
  // text above. Falls back to whyLabel when the reason_code is missing or
  // unrecognized (G2-14).
  const UNIT_REASON_COPY = {
    context_window_exceeded: window.JournalI18n.render('journal.window.stats.backlog.reason.context.window'),
  };

  function unitReasonLabel(unit, C) {
    if (unit && unit.reason_code && UNIT_REASON_COPY[unit.reason_code]) {
      return UNIT_REASON_COPY[unit.reason_code];
    }
    return whyLabel(unit && unit.why, C);
  }

  // The single most useful reason to surface directly on a collapsed backlog
  // row, without expanding "work remaining" (G2-14). Real backlog days can
  // carry several why[] units where only some have a specific, mapped
  // reason_code (e.g. one "failed" unit with no code alongside another with
  // reason_code: "context_window_exceeded") — prefer the most specific one
  // over always taking why[0], which may just be the generic case.
  function topReasonLabel(day, C) {
    if (!Array.isArray(day.why) || day.why.length === 0) return null;
    const specific = day.why.find(unit => unit && unit.reason_code && UNIT_REASON_COPY[unit.reason_code]);
    return unitReasonLabel(specific || day.why[0], C);
  }

  // The server gives each backlog day the same reason /app/health shows.
  function reasonCopy(day, C) {
    return (typeof day.reason_copy === 'string' && day.reason_copy) || C.REASON_FAILING_STEP;
  }

  function backlogErrorForDay(day, bl) {
    if (day.error) return day.error;
    const errors = Array.isArray(bl && bl.errors) ? bl.errors : [];
    return errors.find(error => error.day === day.day) || null;
  }

  function dayCopy(day, C) {
    if (day.state === 'stuck') return C.DAY_BADGE;
    if (day.state === 'pending' || day.state === 'unknown') return C.CATCHING_UP_DAY;
    return C.VERDICT_CAUGHT_UP;
  }

  function needsHandDay(day, bl) {
    return day.state === 'stuck' || backlogErrorForDay(day, bl) !== null;
  }

  function catchingUpDay(day, bl) {
    return day.state === 'pending' && backlogErrorForDay(day, bl) === null;
  }

  function dayHref(day) {
    return `/app/transcripts/${encodeURIComponent(day.day)}`;
  }

  function backlogRow(day, C, bl, options = {}) {
    const depth = backlogDepth(day);
    const copy = options.copy || dayCopy(day, C);
    const mainChildren = [
      el('span', {className: 'backlog-row-day'}, [fmtDay(day.day)])
    ];
    if (options.badge) {
      mainChildren.push(el('span', {className: 'backlog-badge'}, [options.badge]));
    }
    mainChildren.push(el('span', {className: 'backlog-row-copy'}, [copy]));
    const whyLabels = Array.isArray(day.why)
      ? day.why.map(unit => unitReasonLabel(unit, C)).filter(Boolean)
      : [];
    // Surface the top reason on the row itself for a collapsed (non-expanded)
    // day, instead of hiding every reason behind "work remaining" (G2-14).
    const topReason = !options.expanded ? topReasonLabel(day, C) : null;
    if (topReason) {
      mainChildren.push(el('span', {className: 'backlog-row-reason'}, [topReason]));
    }
    const children = [
      el('a', {className: 'backlog-row-link', href: dayHref(day)}, [
        el('div', {className: 'backlog-row-main'}, mainChildren),
        depth > 0 ? el('span', {className: 'backlog-depth'}, [window.JournalI18n.render('journal.window.stats.backlog.remaining', [['count', depth.toLocaleString()]])]) : null
      ])
    ];

    if (!options.expanded && whyLabels.length > 0) {
      children.push(
        el('details', {}, [
          el('summary', {}, [window.JournalI18n.render('journal.window.stats.backlog.work.remaining')]),
          el('ul', {className: 'backlog-why-list'}, whyLabels.map(label => el('li', {}, [label])))
        ])
      );
    }

    // The view's one click handler takes these presses (mountStats); each
    // button names its day and what it asks for.
    const processButton = el('button', {type: 'button', className: 'backlog-action', 'data-backlog-day': day.day, 'data-backlog-flavor': 'process-now'}, [C.ACTION_PROCESS_NOW]);
    const redoButton = el('button', {type: 'button', className: 'backlog-action', 'data-backlog-day': day.day, 'data-backlog-flavor': 'from-scratch'}, [C.ACTION_REDO_SCRATCH]);
    const statusEl = el('span', {className: 'backlog-action-status', 'data-backlog-status': day.day});
    children.push(
      el('div', {className: 'backlog-row-actions'}, [
        processButton,
        redoButton,
        statusEl
      ])
    );

    return el('div', {className: 'backlog-row', 'data-backlog-row': day.day}, children);
  }

  function stuckBucket(bl, C) {
    const days = (Array.isArray(bl.days) ? bl.days : []).filter(day => needsHandDay(day, bl));
    if (!days.length) return null;

    return el('section', {className: 'backlog-needs-hand'}, [
      el('h2', {}, [C.BUCKET_HEADING]),
      el('p', {className: 'backlog-description'}, [C.BUCKET_DESCRIPTION]),
      el('div', {className: 'backlog-rows'}, days.map(day => backlogRow(day, C, bl, {
        badge: C.DAY_BADGE,
        copy: reasonCopy(day, C),
        expanded: true
      })))
    ]);
  }

  function backlogList(bl, counts, C) {
    const days = (Array.isArray(bl.days) ? bl.days : []).filter(day => catchingUpDay(day, bl));
    if (!days.length) return null;

    const children = [el('summary', {}, [window.JournalI18n.render('journal.window.stats.backlog.details')])];
    children.push(el('p', {className: 'backlog-routine-note'}, [C.CATCHING_UP_TAIL]));
    children.push(
      el('div', {className: 'backlog-rows'}, days.map(day => backlogRow(day, C, bl)))
    );
    return el('details', {className: 'backlog-list'}, children);
  }

  function renderBacklog(stats) {
    function lowercaseRelativeDay(label) {
      const relative = ['today', 'yesterday', 'tomorrow'].some(day =>
        label === window.JournalI18n.render(`journal.window.date.${day}`));
      return relative ? label.toLowerCase() : label;
    }

    const main = document.getElementById('mainContent');
    // The hero goes above the whole tile block, not between the block's
    // "since your journal began" kicker and the tiles it scopes (G2-B10).
    const statsGrid = document.getElementById('statsGridBlock');
    if (!main || !statsGrid) return;

    const existing = document.getElementById('backlogSection');
    if (existing) existing.remove();

    const C = backlogCopy();
    const bl = stats.backlog;
    const counts = backlogCounts(stats);
    const journalStatus = stats.journal_status || {};
    const verdict = journalStatus.verdict || C.VERDICT_CANT_TELL;

    const heroChildren = [
      // The verdict below is scoped to the 30-day backlog window; label it
      // so it doesn't read as contradicting the all-time totals in the
      // tiles further down the page (G2-09).
      el('p', {className: 'backlog-hero-scope'}, [window.JournalI18n.render('journal.window.stats.backlog.scope')]),
      el('p', {className: 'backlog-hero-line'}, [verdict])
    ];

    const unfinished = journalStatus.unfinished_activities || {};
    const activities = count(unfinished.activities);
    const dayCount = count(unfinished.day_count);
    const oldestDay = unfinished.oldest_day;
    if (activities > 0 && oldestDay) {
      const copy = journalStatus.copy || {};
      const label = window.JournalFormat ? window.JournalFormat.day(oldestDay) : oldestDay;
      const formattedDay = lowercaseRelativeDay(label);
      let tmpl = '';
      if (activities === 1 && dayCount === 1) {
        tmpl = copy.unfinished_template_one || '';
      } else if (activities > 1 && dayCount === 1) {
        tmpl = copy.unfinished_template_many_one_day || '';
      } else if (activities > 0 && dayCount > 1) {
        tmpl = copy.unfinished_template_many_days || '';
      }
      if (tmpl) {
        const text = tmpl
          .replace('{day}', formattedDay)
          .replace('{n}', String(activities))
          .replace('{days}', String(dayCount));
        const link = el('a', {
          className: 'backlog-verdict-day',
          href: `/app/transcripts/${encodeURIComponent(oldestDay)}`
        }, [`${text} →`]);
        heroChildren.push(el('p', {className: 'backlog-hero-line backlog-unfinished-line'}, [link]));
      }
    }
    // The 30-day verdict can say "caught up" while a much larger all-time
    // backlog sits several screens down (X-11/G2-36) — name it here, right
    // under the verdict, instead of leaving the owner to reconcile the two
    // numbers on their own. "waiting to be transcribed" names the specific
    // population (pending_segments) so it reads as a different job from the
    // broader "items needing processing" tile further down the page, which
    // counts every processing step, not just audio (G2-41).
    const olderQueued = count((stats.totals || {}).pending_segments);
    if (bl && bl.degraded !== true && counts.pending === 0 && counts.stuck === 0 && olderQueued > 0) {
      heroChildren.push(el('p', {className: 'backlog-hero-line backlog-hero-secondary'}, [
        window.JournalI18n.render('journal.window.stats.backlog.older.audio', [['count', olderQueued.toLocaleString()]])
      ]));
    }
    const section = el('section', {className: 'backlog-section', id: 'backlogSection'}, [
      el('div', {className: 'backlog-hero'}, heroChildren)
    ]);

    if (bl && bl.degraded !== true) {
      const needsHand = stuckBucket(bl, C);
      if (needsHand) section.appendChild(needsHand);
      const list = backlogList(bl, counts, C);
      if (list) section.appendChild(list);
    }

    main.insertBefore(section, statsGrid);
  }

  function clearDashboardSections() {
    [
      'statsGrid',
      'progressSection',
      'repairSection',
      'tokenChart',
      'audioChart',
      'heatmap',
      'heatmapNarrowNote',
      'facetsChart',
      'activitiesChart'
    ].forEach(id => {
      const node = document.getElementById(id);
      if (node) node.innerHTML = '';
    });
  }

  // Main render function. `hooks` are the mount's: the freshness line it
  // keeps, and the token card's rollup.
  function render(data, hooks) {
    if (!data) return;

    const stats = data.stats || {};
    // Facet display names, keyed by the same slug the facets chart counts
    // by (G2-35). Falls back to the raw slug per-category when a facet
    // isn't in this map.
    const facetTitles = data.facet_titles || {};

    // Clear loading state and notices
    document.getElementById('loading').style.display = 'none';
    document.getElementById('notice').innerHTML = '';

    // Schema version check (non-blocking warning)
    if (stats.schema_version && stats.schema_version !== EXPECTED_SCHEMA_VERSION) {
      document.getElementById('notice').appendChild(
        el('div', {className: 'alert alert-warning'}, data.admin_terminal ? [
          window.JournalI18n.render('journal.window.stats.schema.regenerate.macos'),
          el('code', {}, ['journal journal-stats']),
          window.JournalI18n.render('journal.window.stats.schema.regenerate.end')
        ] : [
          window.JournalI18n.render('journal.window.stats.schema.regenerate'),
          el('code', {}, ['journal journal-stats']),
          window.JournalI18n.render('journal.window.stats.schema.regenerate.end')
        ])
      );
    }

    const main = document.getElementById('mainContent');
    main.style.display = 'block';
    renderBacklog(stats);

    // Required-field validation (blocking — stops rendering if fields missing)
    const requiredFields = ['days', 'totals', 'heatmap', 'tokens', 'talents', 'facets'];
    const missingFields = requiredFields.filter(f => !(f in stats));
    if (missingFields.length > 0) {
      clearDashboardSections();
      // Before the nightly run's first chance there is nothing to wait a moment
      // for: the verdict above already says when stats arrive.
      if ((stats.journal_status || {}).not_yet) return;
      document.getElementById('notice').appendChild(
        el('div', {className: 'alert alert-warning'}, [
          window.JournalI18n.render('journal.window.stats.pending.stats')
        ])
      );
      return;
    }

    // Freshness indicator, and the one "refresh" button the mount keeps
    hooks.freshness(stats.generated_at);

    // The selected-day token card draws its 30-day rollup from this read.
    hooks.rollup(stats.tokens.by_day || {});

    // Handle empty data
    if (!stats.days || Object.keys(stats.days).length === 0) {
      clearDashboardSections();
      document.getElementById('notice').appendChild(
        el('div', {className: 'alert alert-warning'}, [
          el('strong', {}, [window.JournalI18n.render('journal.window.stats.stats.empty.head')]),
          window.JournalI18n.render('journal.window.stats.stats.empty.detail'),
          el('a', {href: '/app/health/'}, [window.JournalI18n.render('journal.window.stats.stats.empty.health')]),
          '.'
        ])
      );
      return;
    }

    // Calculate derived values
    const days = Object.keys(stats.days).sort();
    const totals = stats.totals || {};
    const totalDays = days.length;
    const totalAudioHours = Math.round((stats.totals.total_transcript_duration || 0) / 3600);
    const totalScreenHours = Math.round((stats.totals.total_percept_duration || 0) / 3600);

    // Calculate total tokens across all models
    const tokenTotals = stats.tokens.by_model || {};
    const totalTokens = Object.values(tokenTotals).reduce((sum, model) => {
      return sum + (model.total_tokens || 0);
    }, 0);

    // Render stats cards
    const statsGrid = document.getElementById('statsGrid');
    statsGrid.innerHTML = ''; // Clear existing content
    statsGrid.appendChild(statCard(window.JournalI18n.render('journal.window.stats.totals.days'), totalDays.toLocaleString(), window.JournalI18n.render('journal.window.stats.totals.days.unit')));
    statsGrid.appendChild(statCard(window.JournalI18n.render('journal.window.stats.totals.audio'), totalAudioHours.toLocaleString(), window.JournalI18n.render('journal.window.stats.totals.hours.unit')));
    statsGrid.appendChild(statCard(window.JournalI18n.render('journal.window.stats.totals.screen'), totalScreenHours.toLocaleString(), window.JournalI18n.render('journal.window.stats.totals.hours.unit')));
    statsGrid.appendChild(statCard(window.JournalI18n.render('journal.window.stats.totals.tokens'), fmtTokens(totalTokens), window.JournalI18n.render('journal.window.stats.totals.tokens.unit')));
    statsGrid.appendChild(statCard(window.JournalI18n.render('journal.window.stats.totals.disk'), fmtBytes(totals.day_bytes || 0), window.JournalI18n.render('journal.window.stats.totals.disk.unit')));

    // Render progress cards
    const progressSection = document.getElementById('progressSection');
    progressSection.innerHTML = ''; // Clear existing content
    progressSection.appendChild(
      progressCard(window.JournalI18n.render('journal.window.stats.progress.audio'), totals.transcript_sessions || 0, totals.pending_segments || 0)
    );

    // Combined audio + screen chart data
    const recent = days.slice(-30);
    const hoursData = recent.map(day => {
      const dayData = stats.days[day];
      const audioHours = (dayData.transcript_duration || 0) / 3600;
      const screenHours = (dayData.percept_duration || 0) / 3600;
      return {
        day: day.slice(4, 6) + '/' + day.slice(6, 8),
        audio: audioHours,
        screen: screenHours,
        bytes: dayData.day_bytes || 0
      };
    });

    // Render stacked hours chart
    buildStackedHoursChart(document.getElementById('audioChart'), hoursData);

    // Render heatmap
    if (stats.heatmap) {
      buildHeatmap(document.getElementById('heatmap'), stats.heatmap);
    }

    // Render Facets stacked bar chart. Reserved runtime placeholders fold
    // into "unassigned" instead of charting as a facet (G2-30); each real
    // facet's legend/tooltip label comes from the owner's own facet titles,
    // not the storage slug (G2-35).
    const facetCounts = foldReservedFacetKeys(stats.facets.counts_by_day || {});
    const facetMeta = {
      emptyIcon: window.ConveyIcons.svg('tag'),
      emptyText: window.JournalI18n.render('journal.window.stats.facets.empty'),
      ariaLabel: window.JournalI18n.render('journal.window.stats.facets.accessible')
    };
    Object.entries(facetTitles).forEach(([slug, title]) => {
      facetMeta[slug] = {title};
    });
    facetMeta[UNASSIGNED_FACET_KEY] = {title: window.JournalI18n.render('journal.window.stats.facets.unassigned')};
    buildStackedCategoryChart(
      document.getElementById('facetsChart'),
      facetCounts,
      facetMeta
    );

    // Render Activities stacked bar chart. Activity ids are storage slugs
    // (e.g. 'ai_conversation'); the facets chart above already gets a
    // {title} per slug from the server, so give this chart the same shape
    // of meta rather than falling back to the raw key (G2-42). There's no
    // server-supplied title list for activities, so the fallback title is
    // the slug with underscores turned into spaces — enough to turn
    // 'ai_conversation' into 'AI conversation' without inventing a name.
    const activityCounts = stats.talents.counts_by_day || {};
    const activityMeta = {
      emptyIcon: window.ConveyIcons.svg('zap'),
      emptyText: window.JournalI18n.render('journal.window.stats.activities.empty'),
      ariaLabel: window.JournalI18n.render('journal.window.stats.activities.accessible')
    };
    Object.values(activityCounts).forEach(dayCounts => {
      Object.keys(dayCounts || {}).forEach(activityId => {
        if (!activityMeta[activityId]) {
          activityMeta[activityId] = {title: activityTitle(activityId)};
        }
      });
    });
    buildStackedCategoryChart(
      document.getElementById('activitiesChart'),
      activityCounts,
      activityMeta
    );

    // Render repairs if needed. pending_segments is dropped from this list
    // (G2-36): the audio-processing bar above already carries that same
    // all-time figure ("N pending"), so it isn't repeated as its own tile
    // here under a second name.
    const repairs = ['segments_pending_think'];
    const hasRepairs = repairs.some(key => (totals[key] || 0) > 0);
    const repairSection = document.getElementById('repairSection');
    repairSection.innerHTML = '';

    if (hasRepairs) {
      const alert = el('div', {className: 'chart-section alert-repair'}, [
        el('h2', {}, [window.JournalI18n.render('journal.window.stats.repair.heading')]),
        // This tile counts every processing step (audio, screen, and
        // everything else waiting on the thinking pass), not just the
        // audio backlog named in the verdict above — the two are different
        // jobs, not two answers to the same question (G2-41).
        el('p', {className: 'repair-section-note'}, [
          window.JournalI18n.render('journal.window.stats.repair.detail')
        ]),
        el('div', {className: 'stats-grid', id: 'repairGrid'})
      ]);

      const repairGrid = alert.querySelector('#repairGrid');
      const repairLabels = {
        segments_pending_think: window.JournalI18n.render('journal.window.stats.repair.thinking')
      };
      // Neutral (matches the rest of the page) once the 30-day verdict is
      // caught up; --danger only while it isn't (X-11).
      const backlogCounted = backlogCounts(stats);
      const caughtUp = backlogCounted.pending === 0 && backlogCounted.stuck === 0
        && !(stats.backlog && stats.backlog.degraded === true);
      const repairColor = caughtUp ? null : 'var(--danger)';

      repairs.forEach(key => {
        const repairCount = totals[key] || 0;
        if (repairCount > 0) {
          // These are all-time totals, distinct from the 30-day backlog
          // verdict above them (G2-09).
          repairGrid.appendChild(
            statCard(repairLabels[key], repairCount.toLocaleString(), window.JournalI18n.render('journal.window.stats.visible.text.since.your.journal.began'), repairColor)
          );
        }
      });

      repairSection.appendChild(alert);
    }
  }

  function readFailure(retryAgain) {
    document.getElementById('loading').style.display = 'none';
    const notice = document.getElementById('notice');
    const previous = document.getElementById('stats-read-error');
    if (previous) previous.remove();
    const retry = el('button', {type: 'button', className: 'stats-refresh', 'data-stats-action': 'try-again'}, [window.JournalI18n.render('journal.window.stats.try.again')]);
    // Busy while its read is out, never disabled: focus stays on it. A
    // press while it is out shares that read.
    retry.addEventListener('click', () => retryAgain(retry));
    notice.appendChild(el('div', {id: 'stats-read-error', className: 'alert alert-error', role: 'alert'}, [
      window.JournalI18n.render('journal.window.stats.read.error'), retry
    ]));
    return retry;
  }

  // What an answer draws, without what moves with the clock alone: the
  // file's time, when the stats were made (the freshness line says that), and
  // the freshness word the page never shows. Today's date is in it, so the
  // relative day labels still change at midnight.
  function drawnKey(data) {
    const copy = JSON.parse(JSON.stringify(data || {}));
    delete copy.file_mtime;
    if (copy.stats && typeof copy.stats === 'object') {
      delete copy.stats.generated_at;
      if (copy.stats.journal_status && typeof copy.stats.journal_status === 'object') {
        delete copy.stats.journal_status.freshness;
      }
    }
    const today = window.JournalClock?.today?.() || '';
    return today + '\u0000' + JSON.stringify(copy);
  }

  // A control the owner may be on when an answer redraws, named by what it
  // is, so the same control can take focus back once it is drawn again.
  function controlKey(node, root) {
    if (!node || !root.contains(node)) return null;
    const backlog = node.closest('[data-backlog-flavor]');
    if (backlog) return `[data-backlog-day="${backlog.dataset.backlogDay}"][data-backlog-flavor="${backlog.dataset.backlogFlavor}"]`;
    const rollupBar = node.closest('[data-tokens-day]');
    if (rollupBar) return `[data-tokens-day="${rollupBar.dataset.tokensDay}"]`;
    const chart = node.closest('.chart');
    if (chart && chart.id) {
      if (node.matches('select')) return `#${chart.id} select`;
      const bar = node.closest('.bar');
      if (bar) return {chart: chart.id, bar: Array.from(bar.parentElement.children).indexOf(bar)};
    }
    const cell = node.closest('.heatmap-cell');
    if (cell) return {heatmap: Array.from(root.querySelectorAll('.heatmap-cell')).indexOf(cell)};
    const summary = node.closest('summary');
    if (summary) {
      const row = summary.closest('[data-backlog-row]');
      if (row) return `[data-backlog-row="${row.dataset.backlogRow}"] summary`;
      if (summary.parentElement?.classList?.contains('backlog-list')) return '.backlog-list > summary';
    }
    const link = node.closest('a[href]');
    if (link) return `a[href="${link.getAttribute('href')}"]`;
    return null;
  }

  function findControl(key, root) {
    if (!key) return null;
    if (typeof key === 'string') return root.querySelector(key);
    if (key.chart) return root.querySelector(`#${key.chart} .bar-chart`)?.children?.[key.bar] || null;
    if (key.heatmap !== undefined) return root.querySelectorAll('.heatmap-cell')[key.heatmap] || null;
    return null;
  }

  function mountStats(ctx) {
    const root = document.querySelector('.dashboard[data-stats-url]');
    if (!root) throw new Error('stats markup is missing');
    const C = backlogCopy();
    const freshnessEl = root.querySelector('#statsFreshness');
    const freshnessText = document.createElement('span');
    freshnessText.className = 'stats-freshness-text';
    // Drawn into the freshness line by the first answer that has one, as the
    // page always has: not while it loads or when the read fails first.
    const refreshButton = el('button', {type: 'button', className: 'stats-refresh', 'data-stats-action': 'refresh'}, [window.JournalI18n.render('journal.window.stats.refresh')]);
    freshnessEl.replaceChildren(freshnessText);

    let reported = false;
    let drawn = null;
    let loading = null;
    // Reprocess writes by day: 'busy' while out, 'queued' once taken; and
    // the line that says what happened. A redraw draws them again.
    const writes = new Map();

    // The store ends the card's reads with the view.
    const card = window.StatsTokenCard
      ? window.StatsTokenCard.mount(ctx, root)
      : { rollup() {} };

    function ready(state) {
      if (reported) return;
      reported = true;
      ctx.ready(state);
    }

    const hooks = {
      freshness(generatedAt) {
        const text = generatedAt
          ? window.JournalI18n.render('journal.window.stats.freshness', [['age', String(relativeTime(Date.now() - new Date(generatedAt).getTime()))]])
          : '';
        if (freshnessText.textContent !== text) freshnessText.textContent = text;
        if (!refreshButton.isConnected) freshnessEl.append(refreshButton);
      },
      rollup(byDay) { card.rollup(byDay); }
    };

    function applyWrite(day) {
      const entry = writes.get(day);
      root.querySelectorAll('[data-backlog-flavor]').forEach(button => {
        if (button.dataset.backlogDay !== day) return;
        if (entry?.state === 'busy') {
          window.SolFocus.setBusy(button, true);
        } else if (entry?.state === 'queued') {
          // Taken: the day can't be asked for twice. It isn't busy any more.
          button.setAttribute('aria-disabled', 'true');
          button.removeAttribute('aria-busy');
        } else {
          window.SolFocus.setBusy(button, false);
        }
      });
      root.querySelectorAll('[data-backlog-status]').forEach(status => {
        if (status.dataset.backlogStatus !== day) return;
        const text = entry?.text || '';
        if (status.textContent !== text) status.textContent = text;
      });
    }

    async function reprocess(button) {
      const day = button.dataset.backlogDay;
      const flavor = button.dataset.backlogFlavor;
      if (window.SolFocus.isBusy(button)) return;
      if (flavor === 'from-scratch' && !(await window.SolQuestion.ask({ message: C.CONFIRM_REDO_SCRATCH, action: C.ACTION_REDO_SCRATCH, signal: ctx.signal, opener: () => Array.from(root.querySelectorAll('[data-backlog-flavor]')).find(el => el.dataset.backlogDay === day && el.dataset.backlogFlavor === flavor) }))) return;
      if (ctx.disposed) return;
      writes.set(day, {state: 'busy', text: ''});
      applyWrite(day);
      ctx.read('/app/health/api/reprocess', {
        method: 'POST',
        headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({day, flavor})
      })
        .then(result => {
          if (ctx.disposed) return;
          if (result && (result.status === 'already_complete' || result.status === 'current_degraded' || result.status === 'held_by_backoff')) {
            writes.set(day, {state: null, text: result.message || ''});
          } else {
            writes.set(day, {state: 'queued', text: C.QUEUED_FEEDBACK});
          }
          applyWrite(day);
        })
        .catch(err => {
          if (ctx.disposed) return;
          if (window.logError) {
            window.logError(err, {context: 'stats: reprocess failed'});
          }
          writes.set(day, {state: null, text: err && err.serverMessage ? err.serverMessage : window.JournalI18n.render('journal.window.stats.try.again')});
          applyWrite(day);
        });
    }

    // Draws an answer. One that draws what is already drawn leaves the page
    // alone but for the freshness line. When it does draw, the control the
    // owner was on takes focus back, else "refresh".
    function draw(data) {
      const key = drawnKey(data);
      if (key === drawn) {
        hooks.freshness(data.stats?.generated_at);
        return;
      }
      const active = document.activeElement;
      const held = controlKey(active, root);
      const wasInside = root.contains(active);
      // A disclosure the owner opened stays open through the redraw.
      const open = Array.from(root.querySelectorAll('.backlog-list[open], [data-backlog-row] details[open]'))
        .map(node => node.classList.contains('backlog-list') ? '.backlog-list' : `[data-backlog-row="${node.closest('[data-backlog-row]').dataset.backlogRow}"] details`);
      render(data, hooks);
      drawn = key;
      open.forEach(selector => {
        const node = root.querySelector(selector);
        if (node) node.open = true;
      });
      writes.forEach((_, day) => applyWrite(day));
      if (wasInside) {
        window.SolFocus.refocus(document, true, [findControl(held, root), refreshButton]);
      }
    }

    function load(pressed) {
      if (loading) return loading;
      if (pressed) window.SolFocus.setBusy(pressed, true);
      const done = () => {
        loading = null;
        if (pressed) window.SolFocus.setBusy(pressed, false);
      };
      loading = ctx.fetch(STATS_URL, {credentials: 'same-origin'})
        .then(response => {
          if (!response.ok) {
            if (response.status === 401 || response.redirected) {
              // Access changed while viewing the dashboard; reload the page.
              window.location.reload();
              return;
            }
            throw new Error('failed to load data');
          }
          return response.json();
        })
        .then(data => {
          // The store never answers a view that has gone, so this draws only
          // into the view on the page.
          done();
          if (!data) return;
          const error = document.getElementById('stats-read-error');
          const fromError = error && error.contains(document.activeElement);
          if (error) error.remove();
          draw(data);
          if (fromError) {
            // The error is gone; "refresh" took its place, else the heading.
            window.SolFocus.refocus(document, true, [refreshButton, root.querySelector('h1')]);
          }
          ready('content');
        })
        .catch(() => {
          done();
          const fromError = (document.getElementById('stats-read-error') || {contains: () => false}).contains(document.activeElement);
          const retry = readFailure(again => load(again));
          if (fromError) window.SolFocus.refocus(document, true, [retry]);
          ready('error');
        });
      return loading;
    }

    ctx.on(root, 'click', event => {
      const target = event.target;
      if (!target || typeof target.closest !== 'function') return;
      const backlog = target.closest('[data-backlog-flavor]');
      if (backlog && root.contains(backlog)) {
        reprocess(backlog);
        return;
      }
      const refresh = target.closest('[data-stats-action="refresh"]');
      if (refresh && root.contains(refresh)) {
        // A press while its read is out shares that read (load).
        load(refresh);
      }
    });

    load(null);
  }

  window.SolViews.register('stats', { mount: mountStats });
})();
