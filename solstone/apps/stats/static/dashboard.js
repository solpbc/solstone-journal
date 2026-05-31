// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Dashboard module for client-side rendering
const Dashboard = (function() {
  'use strict';

  const EXPECTED_SCHEMA_VERSION = 6;
  const DISPLAY_LABELS = { transcript: 'audio', percept: 'screen' };

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

  // Format byte counts with GB/MB/KB suffixes
  function fmtBytes(num) {
    const value = Number(num);
    if (value >= 1e12) return (value / 1e12).toFixed(1) + ' TB';
    if (value >= 1e9) return (value / 1e9).toFixed(1) + ' GB';
    if (value >= 1e6) return (value / 1e6).toFixed(1) + ' MB';
    if (value >= 1e3) return (value / 1e3).toFixed(1) + ' KB';
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
      el('div', {className: 'progress-bar'}, [
        el('div', {
          className: 'progress-fill',
          style: {width: `${pct}%`}
        }, [`${pct}%`])
      ]),
      el('div', {className: 'progress-stats'}, [
        el('span', {}, [`${processed} processed`]),
        el('span', {}, [`${repairable} pending`])
      ])
    ]);
  }

  // Build stacked token chart
  function buildTokenChart(container, tokenData, model) {
    container.innerHTML = ''; // Clear existing content
    
    if (!tokenData || !model) {
      container.appendChild(
        el('div', {className: 'empty-chart'}, [
          el('div', {style: 'font-size: 2em;'}, ['📊']),
          el('div', {style: 'font-weight: 600; font-size: 1.1em;'}, ['Select a model']),
          el('div', {style: 'color: #999;'}, ['Choose a model above to view token usage'])
        ])
      );
      return;
    }

    // Get last 30 days of data
    const days = Object.keys(tokenData).sort().slice(-30);
    
    if (!days.length) {
      container.appendChild(
        el('div', {className: 'empty-chart'}, [
          el('div', {style: 'font-size: 2em;'}, ['📊']),
          el('div', {style: 'font-weight: 600; font-size: 1.1em;'}, ['No token data']),
          el('div', {style: 'color: #999;'}, ['no token usage for this model'])
        ])
      );
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
      container.appendChild(
        el('div', {className: 'empty-chart'}, [
          el('div', {style: 'font-size: 2em;'}, ['📊']),
          el('div', {style: 'font-weight: 600; font-size: 1.1em;'}, ['No recent token usage']),
          el('div', {style: 'color: #999;'}, ['No tokens used for this model in the last 30 days'])
        ])
      );
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
        el('div', {className: 'legend-color', style: {background: '#2171b5'}, 'aria-hidden': 'true'}),
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
      container.appendChild(
        el('div', {className: 'empty-chart'}, [
          el('div', {style: 'font-size: 2em;'}, ['🎙️']),
          el('div', {style: 'font-weight: 600; font-size: 1.1em;'}, ['no observations yet']),
          el('div', {style: 'color: #999;'}, ['no audio or screen observations yet'])
        ])
      );
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
        el('div', {className: 'legend-color', style: {background: '#2171b5'}, 'aria-hidden': 'true'}),
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
          style: {background: `rgba(102,126,234,${intensity})`},
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

  // Generate consistent colors for categories
  function getCategoryColor(index, total) {
    // Use a palette of distinct colors
    const palette = [
      '#0072B2', '#E69F00', '#009E73', '#CC79A7', '#56B4E9',
      '#D55E00', '#F0E442', '#000000', '#332288', '#88CCEE',
      '#44AA99', '#117733', '#999933', '#882255', '#661100'
    ];
    return palette[index % palette.length];
  }

  // Build stacked category chart (for Activities or Facets)
  function buildStackedCategoryChart(container, countsByDay, meta = {}) {
    container.innerHTML = '';

    if (!countsByDay || Object.keys(countsByDay).length === 0) {
      container.appendChild(
        el('div', {className: 'empty-chart'}, [
          el('div', {style: 'font-size: 2em;'}, [meta.emptyIcon || '📊']),
          el('div', {style: 'font-weight: 600; font-size: 1.1em;'}, ['No data available']),
          el('div', {style: 'color: #999;'}, [meta.emptyText || 'No data available'])
        ])
      );
      return;
    }

    // Get last 30 days sorted
    const days = Object.keys(countsByDay).sort().slice(-30);
    if (!days.length) {
      container.appendChild(
        el('div', {className: 'empty-chart'}, [
          el('div', {style: 'font-size: 2em;'}, [meta.emptyIcon || '📊']),
          el('div', {style: 'font-weight: 600; font-size: 1.1em;'}, ['No data available']),
          el('div', {style: 'color: #999;'}, [meta.emptyText || 'No data available'])
        ])
      );
      return;
    }

    // Collect all unique categories across all days
    const allCategories = new Set();
    days.forEach(day => {
      Object.keys(countsByDay[day] || {}).forEach(cat => allCategories.add(cat));
    });
    const categories = Array.from(allCategories).sort();

    if (!categories.length) {
      container.appendChild(
        el('div', {className: 'empty-chart'}, [
          el('div', {style: 'font-size: 2em;'}, [meta.emptyIcon || '📊']),
          el('div', {style: 'font-weight: 600; font-size: 1.1em;'}, ['No data available']),
          el('div', {style: 'color: #999;'}, [meta.emptyText || 'No data available'])
        ])
      );
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
      container.appendChild(
        el('div', {className: 'empty-chart'}, [
          el('div', {style: 'font-size: 2em;'}, [meta.emptyIcon || '📊']),
          el('div', {style: 'font-weight: 600; font-size: 1.1em;'}, ['No data available']),
          el('div', {style: 'color: #999;'}, [meta.emptyText || 'No data available'])
        ])
      );
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

    const stuckArm = fmt(s === 1 ? C.VERDICT_STUCK_ONLY_SINGULAR : C.VERDICT_STUCK_ONLY_PLURAL, {stuck: s}).replace(/\.$/, '');
    const pendingTail = fmt((C.VERDICT_BOTH_PLURAL || '').split(' — ')[1], {pending: p});
    return stuckArm + ' — ' + pendingTail;
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

  function reasonCopy(day, C) {
    if (day.reason === 'corrupt_raw') return C.REASON_CORRUPT_RAW;
    return C.REASON_FAILING_STEP;
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
        if (result && result.status === 'already_complete') {
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
      ? day.why.map(unit => whyLabel(unit.why, C)).filter(Boolean)
      : [];
    const children = [
      el('a', {className: 'backlog-row-link', href: dayHref(day)}, [
        el('div', {className: 'backlog-row-main'}, mainChildren),
        depth > 0 ? el('span', {className: 'backlog-depth'}, [String(depth)]) : null
      ])
    ];

    if (!options.expanded && whyLabels.length > 0) {
      children.push(
        el('details', {}, [
          el('summary', {}, [copy]),
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

    const heading = fmt(C.CATCHING_UP_AGGREGATE, {pending: counts.pending});
    const children = [
      el('h2', {}, [heading])
    ];
    children.push(el('p', {className: 'backlog-routine-note'}, [C.CATCHING_UP_TAIL]));
    children.push(
      el('div', {className: 'backlog-rows'}, days.map(day => backlogRow(day, C, bl)))
    );
    return el('section', {className: 'backlog-list'}, children);
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

    // Handle API error
    if (data.error) {
      document.getElementById('notice').appendChild(
        el('div', {className: 'alert alert-error'}, [
          'Couldn\'t load stats data — the stats file may be corrupt or unreadable. ',
          'Try regenerating with think-journal-stats.'
        ])
      );
      return;
    }

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
        ? 'Stats generated ' + relativeTime(Date.now() - new Date(stats.generated_at).getTime()) + ' ago'
        : '';
      const refreshLink = el('a', {
        className: 'stats-refresh',
        href: '#'
      }, ['refresh']);
      refreshLink.addEventListener('click', function(e) {
        e.preventDefault();
        const statsUrl = document.querySelector('.dashboard').dataset.statsUrl;
        if (statsUrl) Dashboard.load(statsUrl);
      });
      freshnessEl.appendChild(refreshLink);
    }
    
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
    statsGrid.appendChild(statCard('total days', totalDays, 'days'));
    statsGrid.appendChild(statCard('audio hours', totalAudioHours, 'hours'));
    statsGrid.appendChild(statCard('screen hours', totalScreenHours, 'hours'));
    statsGrid.appendChild(statCard('total tokens', fmtTokens(totalTokens), 'tokens'));
    statsGrid.appendChild(statCard('disk usage', fmtBytes(totals.day_bytes || 0), 'journal days'));
    
    // Render progress cards
    const progressSection = document.getElementById('progressSection');
    progressSection.innerHTML = ''; // Clear existing content
    progressSection.appendChild(
      progressCard('audio processing', totals.transcript_sessions || 0, totals.pending_segments || 0)
    );
    
    // Token usage setup
    const tokenUsage = stats.tokens.by_day || {};
    const models = Object.keys(tokenTotals).sort();
    
    // Populate model selector
    const modelSelector = document.getElementById('modelSelector');
    if (models.length > 0) {
      modelSelector.innerHTML = '';
      
      // Add "total" option first
      const totalOption = el('option', {value: 'total'}, ['total']);
      modelSelector.appendChild(totalOption);
      
      // Add individual models
      models.forEach(model => {
        const option = el('option', {value: model}, [model]);
        modelSelector.appendChild(option);
      });
      
      // Set total as default
      modelSelector.value = 'total';
      
      // Initial render
      buildTokenChart(document.getElementById('tokenChart'), tokenUsage, 'total');
      
      // Handle model selection changes
      modelSelector.addEventListener('change', function() {
        buildTokenChart(document.getElementById('tokenChart'), tokenUsage, this.value);
      });
    } else {
      // No token data available
      buildTokenChart(document.getElementById('tokenChart'), null, null);
    }
    
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
        emptyIcon: '🏷️',
        emptyText: 'no facet data yet',
        ariaLabel: 'facets bar chart showing facet distribution over the last 30 days'
      }
    );

    // Render Activities stacked bar chart
    buildStackedCategoryChart(
      document.getElementById('activitiesChart'),
      stats.talents.counts_by_day || {},
      {
        emptyIcon: '⚡',
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
        segments_pending_think: 'segments awaiting thinking'
      };

      repairs.forEach(key => {
        const count = totals[key] || 0;
        if (count > 0) {
          repairGrid.appendChild(
            statCard(repairLabels[key], count, '', '#dc2626')
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
        credentials: 'same-origin'  // Include cookies for authentication
      })
        .then(response => {
          if (!response.ok) {
            if (response.status === 401 || response.redirected) {
              // Redirected to login, reload the page
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
              'failed to load dashboard data: ' + error.message
            ])
          );
        });
    }
  };
})();

// Export for use in templates
window.Dashboard = Dashboard;
