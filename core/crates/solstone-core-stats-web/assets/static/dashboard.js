// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Dashboard module for client-side rendering
const Dashboard = (function() {
  'use strict';

  const EXPECTED_SCHEMA_VERSION = 8;
  const DISPLAY_LABELS = { transcript: 'audio', percept: 'screen' };

  // Warm replacement for the prior cool blue (#2171b5) used by the input and
  // audio series — gold, from --gold in tokens.css (X-12).
  const WARM_INPUT_COLOR = '#FFCF33';

  // Warm categorical base (gold -> orange -> coral/danger -> warm plum ->
  // olive/success -> ink-soft), derived from the brand tokens, replacing the
  // prior unbranded rainbow palette for facets/activities legends (X-12).
  // Hex values mirror --gold, --orange, --danger, --success and --ink-soft in
  // tokens.css; "warm plum" has no existing token, so it is a hand-picked
  // warm-hued fill between the coral and olive anchors.
  const WARM_CATEGORICAL_BASE = ['#FFCF33', '#E8913A', '#9F2D2D', '#6B3A46', '#3F9D6A', '#5B5246'];

  // Orange sequential ramp for the heatmap, replacing the prior cool
  // rgba(102,126,234,…) indigo (X-12). RGB of --orange (#E8913A).
  const WARM_HEATMAP_RGB = '232,145,58';

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

  function fmtDay(raw) {
    const months = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
    if (raw.length === 8) {
      // YYYYMMDD
      return months[parseInt(raw.slice(4, 6), 10) - 1] + ' ' + parseInt(raw.slice(6, 8), 10);
    }
    // MM/DD
    return months[parseInt(raw.slice(0, 2), 10) - 1] + ' ' + parseInt(raw.slice(3, 5), 10);
  }

  function shouldLabel(i, len) {
    return i === 0 || i === len - 1 || (i % 7 === 0);
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
      el('p', {className: 'stat-subtitle'}, ['since your journal began']),
      el('div', {className: 'progress-bar'}, [
        el('div', {
          className: 'progress-fill',
          style: {width: `${pct}%`}
        }, [`${pct}%`])
      ]),
      el('div', {className: 'progress-stats'}, [
        el('span', {}, [`${processed.toLocaleString()} processed`]),
        el('span', {}, [`${repairable.toLocaleString()} pending`])
      ])
    ]);
  }

  // Build stacked token chart
  function buildTokenChart(container, tokenData, model) {
    container.innerHTML = ''; // Clear existing content

    if (!tokenData || !model) {
      renderEmptyChart(container, {
        icon: window.ConveyIcons.svg('chart-column'),
        heading: 'select a model…',
        desc: 'Choose a model above to view token usage'
      });
      return;
    }

    // Get last 30 days of data
    const days = Object.keys(tokenData).sort().slice(-30);

    if (!days.length) {
      renderEmptyChart(container, {
        icon: window.ConveyIcons.svg('chart-column'),
        heading: 'No token data',
        desc: 'no token usage for this model'
      });
      return;
    }

    // Calculate max total for scaling
    let maxTotal = 0;
    const chartData = days.map(day => {
      let input = 0, reasoning = 0, output = 0;

      if (model === 'total') {
        // Sum all models for this day
        const dayModels = tokenData[day] || {};
        Object.values(dayModels).forEach(modelData => {
          input += modelData.input_tokens || 0;
          reasoning += modelData.reasoning_tokens || 0;
          output += modelData.output_tokens || 0;
        });
      } else {
        // Single model data
        const dayData = tokenData[day][model] || {};
        input = dayData.input_tokens || 0;
        reasoning = dayData.reasoning_tokens || 0;
        output = dayData.output_tokens || 0;
      }

      const total = input + reasoning + output;
      maxTotal = Math.max(maxTotal, total);
      return { day, input, reasoning, output, total };
    });

    if (maxTotal === 0) {
      renderEmptyChart(container, {
        icon: window.ConveyIcons.svg('chart-column'),
        heading: 'No recent token usage',
        desc: 'No tokens used for this model in the last 30 days'
      });
      return;
    }

    const chart = el('div', {className: 'bar-chart', role: 'img', 'aria-label': 'Token activity bar chart showing usage over the last 30 days'});

    chartData.forEach((d, i) => {
      const height = (d.total / maxTotal) * 100;
      const bar = el('div', {
        className: 'bar',
        style: {height: `${height}%`, background: 'transparent', overflow: 'visible'}
      });

      // Create stacked segments
      const stackLabel = `${fmtDay(d.day)}: ${fmtTokens(d.total)} tokens (input: ${d.input}, reasoning: ${d.reasoning}, output: ${d.output})`;
      const stack = el('div', {className: 'bar-stack', style: {height: '100%'}, 'aria-label': stackLabel});

      // Calculate segment heights as percentages of the bar
      if (d.total > 0) {
        const inputPct = (d.input / d.total) * 100;
        const reasoningPct = (d.reasoning / d.total) * 100;
        const outputPct = (d.output / d.total) * 100;

        if (d.output > 0) {
          stack.appendChild(el('div', {
            className: 'stack-segment output',
            style: {height: `${outputPct}%`}
          }));
        }
        if (d.reasoning > 0) {
          stack.appendChild(el('div', {
            className: 'stack-segment reasoning',
            style: {height: `${reasoningPct}%`}
          }));
        }
        if (d.input > 0) {
          stack.appendChild(el('div', {
            className: 'stack-segment input',
            style: {height: `${inputPct}%`}
          }));
        }
      }

      bar.appendChild(stack);

      if (d.total > 0) {
        const formatted = fmtTokens(d.total);
        bar.appendChild(el('div', {className: 'bar-value'}, [formatted]));
        bar.dataset.tip = `${d.day.slice(4, 6)}/${d.day.slice(6, 8)} - Input: ${d.input}, Reasoning: ${d.reasoning}, Output: ${d.output}`;
      }

      if (shouldLabel(i, chartData.length)) {
        bar.appendChild(el('div', {className: 'bar-label'}, [fmtDay(d.day)]));
      }

      chart.appendChild(bar);
    });

    container.appendChild(chart);

    // Add legend
    const legend = el('div', {className: 'token-legend'}, [
      el('div', {className: 'legend-item'}, [
        el('div', {className: 'legend-color', style: {background: WARM_INPUT_COLOR}, 'aria-hidden': 'true'}),
        'input'
      ]),
      el('div', {className: 'legend-item'}, [
        el('div', {
          className: 'legend-color',
          style: {
            background: '#e6550d',
            backgroundImage: 'repeating-linear-gradient(45deg, transparent, transparent 3px, rgba(255,255,255,0.3) 3px, rgba(255,255,255,0.3) 6px)'
          },
          'aria-hidden': 'true'
        }),
        'reasoning'
      ]),
      el('div', {className: 'legend-item'}, [
        el('div', {
          className: 'legend-color',
          style: {
            background: '#31a354',
            backgroundImage: 'radial-gradient(circle, rgba(255,255,255,0.3) 1px, transparent 1px)',
            backgroundSize: '6px 6px'
          },
          'aria-hidden': 'true'
        }),
        'output'
      ])
    ]);
    container.appendChild(legend);
  }

  // Build stacked hours chart (audio + screen)
  function buildStackedHoursChart(container, data) {
    container.innerHTML = ''; // Clear existing content

    if (!data || !data.length) {
      renderEmptyChart(container, {
        icon: window.ConveyIcons.svg('mic-vocal'),
        heading: 'no observations yet',
        desc: 'no audio or screen observations yet'
      });
      return;
    }

    // Calculate max total for scaling
    const maxTotal = Math.max(...data.map(d => d.audio + d.screen)) || 1;

    const chart = el('div', {className: 'bar-chart', role: 'img', 'aria-label': 'observation hours bar chart showing audio and screen time per day'});

    data.forEach((d, i) => {
      const total = d.audio + d.screen;
      const height = (total / maxTotal) * 100;
      const bar = el('div', {
        className: 'bar',
        style: {height: `${height}%`, background: 'transparent', overflow: 'visible'}
      });

      // Create stacked segments
      const stackLabel = `${fmtDay(d.day)}: ${total.toFixed(1)}h (audio: ${d.audio.toFixed(1)}h, screen: ${d.screen.toFixed(1)}h)`;
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
        const titleParts = [`${d.day} - Audio: ${d.audio.toFixed(1)}h, Screen: ${d.screen.toFixed(1)}h`];
        if (d.bytes) titleParts.push(`Disk: ${fmtBytes(d.bytes)}`);
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
        'audio'
      ]),
      el('div', {className: 'legend-item'}, [
        el('div', {
          className: 'legend-color',
          style: {
            background: '#e6550d',
            backgroundImage: 'repeating-linear-gradient(45deg, transparent, transparent 3px, rgba(255,255,255,0.3) 3px, rgba(255,255,255,0.3) 6px)'
          },
          'aria-hidden': 'true'
        }),
        'screen'
      ])
    ]);
    container.appendChild(legend);
  }

  // Build heatmap
  function buildHeatmap(container, data) {
    const days = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
    const maxVal = Math.max(...data.flat()) || 1;

    const heatmap = el('div', {className: 'heatmap', role: 'grid', 'aria-label': 'activity heatmap showing observations by day of week and hour'});

    // Empty top-left corner
    heatmap.appendChild(el('div'));

    // Hour headers
    const header = el('div', {className: 'heatmap-header'});
    for (let h = 0; h < 24; h++) {
      header.appendChild(el('div', {className: 'heatmap-hour'}, [String(h)]));
    }
    heatmap.appendChild(header);

    // Days with cells
    for (let d = 0; d < 7; d++) {
      heatmap.appendChild(el('div', {className: 'heatmap-label'}, [days[d]]));

      for (let h = 0; h < 24; h++) {
        const intensity = data[d][h] / maxVal;
        const cellTitle = `${days[d]} ${h}:00 - ${Math.round(data[d][h])} min`;
        const cell = el('div', {
          className: 'heatmap-cell',
          style: {background: `rgba(${WARM_HEATMAP_RGB},${intensity})`},
          'data-tip': cellTitle,
          'aria-label': cellTitle,
          role: 'gridcell',
          tabindex: '-1'
        });
        heatmap.appendChild(cell);
      }
    }

    container.appendChild(heatmap);
  }

  // Lighten (positive percent) or darken (negative percent) a hex color,
  // used to extend the warm categorical base past its 6 anchor hues.
  function shadeHex(hex, percent) {
    const num = parseInt(hex.slice(1), 16);
    const r = (num >> 16) & 0xff;
    const g = (num >> 8) & 0xff;
    const b = num & 0xff;
    const target = percent < 0 ? 0 : 255;
    const amount = Math.min(1, Math.abs(percent) / 100);
    const mix = (channel) => Math.round((target - channel) * amount + channel);
    return '#' + [mix(r), mix(g), mix(b)].map(v => v.toString(16).padStart(2, '0')).join('');
  }

  // Generate consistent colors for categories from the warm categorical base
  // (X-12). Cycles of six reuse the same six warm hues at alternating
  // lighter/darker shades so a repeated hue at the same palette position
  // still reads apart from its first pass, instead of falling back to an
  // unbranded (and previously cool-hued) default set.
  function getCategoryColor(index, total) {
    const cycle = Math.floor(index / WARM_CATEGORICAL_BASE.length);
    const base = WARM_CATEGORICAL_BASE[index % WARM_CATEGORICAL_BASE.length];
    if (cycle === 0) return base;
    const step = Math.ceil(cycle / 2) * 22;
    return shadeHex(base, cycle % 2 === 1 ? step : -step);
  }

  // Build stacked category chart (for Activities or Facets)
  function buildStackedCategoryChart(container, countsByDay, meta = {}) {
    container.innerHTML = '';

    if (!countsByDay || Object.keys(countsByDay).length === 0) {
      renderEmptyChart(container, {
        icon: meta.emptyIcon,
        heading: 'No data available',
        desc: meta.emptyText || 'No data available'
      });
      return;
    }

    // Get last 30 days sorted
    const days = Object.keys(countsByDay).sort().slice(-30);
    if (!days.length) {
      renderEmptyChart(container, {
        icon: meta.emptyIcon,
        heading: 'No data available',
        desc: meta.emptyText || 'No data available'
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
        heading: 'No data available',
        desc: meta.emptyText || 'No data available'
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
        heading: 'No data available',
        desc: meta.emptyText || 'No data available'
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
  }

  function backlogCopy() {
    return window.BACKLOG_COPY || {};
  }

  function fmt(template, vals = {}) {
    return String(template || '')
      .replace(/\{stuck_n\}/g, String(vals.stuck ?? ''))
      .replace(/\{pending_n\}/g, String(vals.pending ?? ''));
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

  function backlogVerdict(stats) {
    const C = backlogCopy();
    const bl = stats.backlog;
    const counts = backlogCounts(stats);
    const p = counts.pending;
    const s = counts.stuck;

    if (!bl || bl.degraded === true) return C.VERDICT_CANT_TELL;
    if (p === 0 && s === 0) return C.VERDICT_CAUGHT_UP;
    if (s > 0 && p === 0) {
      return fmt(s === 1 ? C.VERDICT_STUCK_ONLY_SINGULAR : C.VERDICT_STUCK_ONLY_PLURAL, {stuck: s});
    }
    if (s === 0 && p > 0) {
      return fmt(p === 1 ? C.VERDICT_PENDING_ONLY_SINGULAR : C.VERDICT_PENDING_ONLY_PLURAL, {pending: p});
    }

    const stuckArm = fmt(s === 1 ? C.VERDICT_MIXED_STUCK_SINGULAR : C.VERDICT_MIXED_STUCK_PLURAL, {stuck: s});
    const pendingArm = fmt(p === 1 ? C.VERDICT_MIXED_PENDING_SINGULAR : C.VERDICT_MIXED_PENDING_PLURAL, {pending: p});
    return stuckArm + '. ' + pendingArm + '.';
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
    context_window_exceeded: "a summary step hit the model's context limit",
  };

  function unitReasonLabel(unit, C) {
    if (unit && unit.reason_code && UNIT_REASON_COPY[unit.reason_code]) {
      return UNIT_REASON_COPY[unit.reason_code];
    }
    return whyLabel(unit && unit.why, C);
  }

  // The single most useful reason to surface directly on a collapsed backlog
  // row, without expanding "work remaining" (G2-14).
  function topReasonLabel(day, C) {
    if (!Array.isArray(day.why) || day.why.length === 0) return null;
    return unitReasonLabel(day.why[0], C);
  }

  function reasonCopy(day, C) {
    const REASON_COPY_KEYS = {
      corrupt_raw: 'REASON_CORRUPT_RAW',
      catchup_backoff: 'REASON_CATCHUP_BACKOFF',
      segment_repair_progressing: 'REASON_SEGMENT_REPAIR_PROGRESSING',
      segment_repair_degraded: 'REASON_SEGMENT_REPAIR_DEGRADED',
      segment_repair_stuck: 'REASON_SEGMENT_REPAIR_STUCK',
      segment_repair_unknown: 'REASON_SEGMENT_REPAIR_UNKNOWN',
    };
    const key = REASON_COPY_KEYS[day.reason];
    // "failing_step" and any other/unrecognized reason keep the generic
    // failing-step copy -- it's the one reason where "keeps failing, try
    // again" is actually accurate.
    return (key && C[key]) || C.REASON_FAILING_STEP;
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

  function requestBacklogReprocess(day, flavor, buttons, statusEl, queuedFeedback) {
    buttons.forEach(button => { button.disabled = true; });
    statusEl.textContent = '';
    return window.apiJson('/app/health/api/reprocess', {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({day, flavor})
    })
      .then(result => {
        if (result && (result.status === 'already_complete' || result.status === 'held_by_backoff')) {
          statusEl.textContent = result.message || '';
          buttons.forEach(button => { button.disabled = false; });
          return;
        }
        statusEl.textContent = queuedFeedback;
      })
      .catch(err => {
        buttons.forEach(button => { button.disabled = false; });
        if (window.logError) {
          window.logError(err, {context: 'stats: reprocess failed'});
        }
        statusEl.textContent = err && err.serverMessage ? err.serverMessage : 'try again';
      });
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
        depth > 0 ? el('span', {className: 'backlog-depth'}, [`${depth.toLocaleString()} steps left`]) : null
      ])
    ];

    if (!options.expanded && whyLabels.length > 0) {
      children.push(
        el('details', {}, [
          el('summary', {}, ['work remaining']),
          el('ul', {className: 'backlog-why-list'}, whyLabels.map(label => el('li', {}, [label])))
        ])
      );
    }

    const processButton = el('button', {type: 'button', className: 'backlog-action'}, [C.ACTION_PROCESS_NOW]);
    const redoButton = el('button', {type: 'button', className: 'backlog-action'}, [C.ACTION_REDO_SCRATCH]);
    const statusEl = el('span', {className: 'backlog-action-status'});
    const buttons = [processButton, redoButton];
    processButton.addEventListener('click', () => {
      requestBacklogReprocess(day.day, 'process-now', buttons, statusEl, C.QUEUED_FEEDBACK);
    });
    redoButton.addEventListener('click', () => {
      if (!window.confirm(C.CONFIRM_REDO_SCRATCH)) return;
      requestBacklogReprocess(day.day, 'from-scratch', buttons, statusEl, C.QUEUED_FEEDBACK);
    });
    children.push(
      el('div', {className: 'backlog-row-actions'}, [
        processButton,
        redoButton,
        statusEl
      ])
    );

    return el('div', {className: 'backlog-row'}, children);
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

    const children = [el('summary', {}, ['processing details'])];
    children.push(el('p', {className: 'backlog-routine-note'}, [C.CATCHING_UP_TAIL]));
    children.push(
      el('div', {className: 'backlog-rows'}, days.map(day => backlogRow(day, C, bl)))
    );
    return el('details', {className: 'backlog-list'}, children);
  }

  function renderBacklog(stats) {
    const main = document.getElementById('mainContent');
    const statsGrid = document.getElementById('statsGrid');
    if (!main || !statsGrid) return;

    const existing = document.getElementById('backlogSection');
    if (existing) existing.remove();

    const C = backlogCopy();
    const bl = stats.backlog;
    const counts = backlogCounts(stats);
    const section = el('section', {className: 'backlog-section', id: 'backlogSection'}, [
      el('div', {className: 'backlog-hero'}, [
        // The verdict below is scoped to the 30-day backlog window; label it
        // so it doesn't read as contradicting the all-time totals in the
        // tiles further down the page (G2-09).
        el('p', {className: 'backlog-hero-scope'}, ['last 30 days']),
        el('p', {className: 'backlog-hero-line'}, [backlogVerdict(stats)])
      ])
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
      'facetsChart',
      'activitiesChart'
    ].forEach(id => {
      const node = document.getElementById(id);
      if (node) node.innerHTML = '';
    });
  }

  // Main render function
  function render(data) {
    if (!data) return;

    const stats = data.stats || {};

    // Clear loading state and notices
    document.getElementById('loading').style.display = 'none';
    document.getElementById('notice').innerHTML = '';

    // Schema version check (non-blocking warning)
    if (stats.schema_version && stats.schema_version !== EXPECTED_SCHEMA_VERSION) {
      document.getElementById('notice').appendChild(
        el('div', {className: 'alert alert-warning'}, [
          'These stats were generated with an older format. Run ',
          el('code', {}, ['journal journal-stats']),
          ' to regenerate.'
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
      document.getElementById('notice').appendChild(
        el('div', {className: 'alert alert-warning'}, [
          'your stats aren\'t ready yet. check back in a moment.'
        ])
      );
      return;
    }

    // Freshness indicator
    const freshnessEl = document.getElementById('statsFreshness');
    if (freshnessEl) {
      freshnessEl.textContent = stats.generated_at
        ? 'updated ' + relativeTime(Date.now() - new Date(stats.generated_at).getTime()) + ' ago'
        : '';
      const refreshButton = el('button', {
        type: 'button',
        className: 'stats-refresh'
      }, ['refresh']);
      refreshButton.addEventListener('click', function() {
        const statsUrl = document.querySelector('.dashboard').dataset.statsUrl;
        if (statsUrl) Dashboard.load(statsUrl);
      });
      freshnessEl.appendChild(refreshButton);
    }

    document.dispatchEvent(new CustomEvent('stats:token-rollup', {
      detail: stats.tokens.by_day || {}
    }));

    // Handle empty data
    if (!stats.days || Object.keys(stats.days).length === 0) {
      clearDashboardSections();
      document.getElementById('notice').appendChild(
        el('div', {className: 'alert alert-warning'}, [
          el('strong', {}, ['No data available. ']),
          'Run think-journal-stats to generate statistics.'
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
    statsGrid.appendChild(statCard('total days', totalDays.toLocaleString(), 'days'));
    statsGrid.appendChild(statCard('audio hours', totalAudioHours.toLocaleString(), 'hours'));
    statsGrid.appendChild(statCard('screen hours', totalScreenHours.toLocaleString(), 'hours'));
    statsGrid.appendChild(statCard('total tokens', fmtTokens(totalTokens), 'tokens'));
    statsGrid.appendChild(statCard('disk usage', fmtBytes(totals.day_bytes || 0), 'on disk'));

    // Render progress cards
    const progressSection = document.getElementById('progressSection');
    progressSection.innerHTML = ''; // Clear existing content
    progressSection.appendChild(
      progressCard('audio processing', totals.transcript_sessions || 0, totals.pending_segments || 0)
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

    // Render Facets stacked bar chart
    buildStackedCategoryChart(
      document.getElementById('facetsChart'),
      stats.facets.counts_by_day || {},
      {
        emptyIcon: window.ConveyIcons.svg('tag'),
        emptyText: 'no facet data yet',
        ariaLabel: 'facets bar chart showing facet distribution over the last 30 days'
      }
    );

    // Render Activities stacked bar chart
    buildStackedCategoryChart(
      document.getElementById('activitiesChart'),
      stats.talents.counts_by_day || {},
      {
        emptyIcon: window.ConveyIcons.svg('zap'),
        emptyText: 'no activity data yet',
        ariaLabel: 'activities bar chart showing activity counts over the last 30 days'
      }
    );

    // Render repairs if needed
    const repairs = ['pending_segments', 'segments_pending_think'];
    const hasRepairs = repairs.some(key => (totals[key] || 0) > 0);
    const repairSection = document.getElementById('repairSection');
    repairSection.innerHTML = '';

    if (hasRepairs) {
      const alert = el('div', {className: 'chart-section alert-repair'}, [
        el('h2', {}, ['items needing processing']),
        el('div', {className: 'stats-grid', id: 'repairGrid'})
      ]);

      const repairGrid = alert.querySelector('#repairGrid');
      const repairLabels = {
        pending_segments: 'pending segments',
        segments_pending_think: 'segments waiting for processing'
      };

      repairs.forEach(key => {
        const count = totals[key] || 0;
        if (count > 0) {
          // These are all-time totals, distinct from the 30-day backlog
          // verdict above them (G2-09).
          repairGrid.appendChild(
            statCard(repairLabels[key], count.toLocaleString(), 'since your journal began', '#dc2626')
          );
        }
      });

      repairSection.appendChild(alert);
    }
  }

  // Public API
  return {
    load: function(url) {
      fetch(url, {
        credentials: 'same-origin'
      })
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
          if (data) render(data);
        })
        .catch(error => {
          document.getElementById('loading').style.display = 'none';
          document.getElementById('notice').appendChild(
            el('div', {className: 'alert alert-error'}, [
              'Couldn\'t load dashboard data — the stats file may be corrupt or unreadable. ',
              'Try regenerating with think-journal-stats.'
            ])
          );
        });
    }
  };
})();

// Export for use in templates
window.Dashboard = Dashboard;
