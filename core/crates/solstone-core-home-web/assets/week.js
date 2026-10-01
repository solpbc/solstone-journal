// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  function esc(s) {
    return String(s ?? '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function markdown(text) {
    if (window.AppServices && typeof window.AppServices.renderMarkdown === 'function') {
      return window.AppServices.renderMarkdown(text);
    }
    return esc(text);
  }

  let activeMenu = null;

  function closeOpenMenu() {
    if (activeMenu) {
      const { btn, dropdown } = activeMenu;
      btn.setAttribute('aria-expanded', 'false');
      dropdown.hidden = true;
      dropdown.style.display = 'none';
      activeMenu = null;
    }
  }

  document.addEventListener('click', function (e) {
    if (activeMenu && !activeMenu.wrapper.contains(e.target)) {
      closeOpenMenu();
    }
  });

  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && activeMenu) {
      e.preventDefault();
      const btn = activeMenu.btn;
      closeOpenMenu();
      if (btn) btn.focus();
    }
  });

  function renderWeekHtml(model) {
    let html = '<div class="week-dashboard" data-week-day="' + esc(model.day) + '">';

    // Top Nav (above title)
    if (model.prev_href || model.next_href) {
      html += '<div class="week-nav">';
      if (model.prev_href) {
        html += '<a class="week-nav-prev" href="' + esc(model.prev_href) + '">' + esc(model.prev_label) + '</a>';
      }
      if (model.next_href) {
        html += '<a class="week-nav-next" href="' + esc(model.next_href) + '">' + esc(model.next_label) + '</a>';
      }
      html += '</div>';
    }

    // Header
    html += '<div class="week-header">';
    html += '<h1 class="week-title">' + esc(model.title) + '</h1>';
    html += '<p class="week-intro">' + esc(model.intro) + '</p>';
    html += '</div>';

    // Grid
    html += '<div class="week-grid" role="group" aria-label="days of the week">';
    (model.cells || []).forEach(function (c) {
      html += '<div class="week-cell ' + esc(c.state) + '" role="img" aria-label="' + esc(c.accessible_name) + '">';
      html += '<span class="week-cell-weekday">' + esc(c.weekday) + '</span>';
      html += '<span class="week-cell-day">' + esc(c.day_number) + '</span>';
      html += '<span class="week-cell-mark ' + esc(c.state) + '" aria-hidden="true"></span>';
      html += '</div>';
    });
    html += '</div>';

    // Legend
    if (model.legend && model.legend.length > 0) {
      html += '<div class="week-legend">';
      model.legend.forEach(function (l) {
        html += '<div class="week-legend-item">';
        html += '<span class="week-cell-mark ' + esc(l.state) + '" aria-hidden="true"></span>';
        html += '<span>' + esc(l.label) + '</span>';
        html += '</div>';
      });
      html += '</div>';
    }

    // From-line
    if (model.from_line) {
      html += '<p class="week-from-line">' + esc(model.from_line) + '</p>';
    }

    // Notice
    if (model.left_out_notice) {
      html += '<p class="week-left-out-notice">' + esc(model.left_out_notice) + '</p>';
    }

    // Rows
    html += '<div class="week-rows">';
    (model.rows || []).forEach(function (r) {
      if (r.left_out) {
        html += '<div class="week-row week-row--left-out" data-memory-key="' + esc(r.key) + '">';
        html += '<span class="week-left-out-text">left out of this week · </span>';
        html += '<button type="button" class="week-undo-btn" data-action="undo" data-key="' + esc(r.key) + '">undo</button>';
        html += '</div>';
      } else {
        html += '<div class="week-row" data-memory-key="' + esc(r.key) + '">';
        html += '<div class="week-row-header">';
        html += '<span class="week-row-day">' + esc(r.day_label) + '</span>';
        html += '<div class="week-row-actions">';
        html += '<div class="week-menu-wrapper">';
        html += '<button type="button" class="week-menu-btn" aria-expanded="false" aria-label="more for ' + esc(r.day_label) + '">···</button>';
        html += '<div class="week-menu-dropdown" hidden style="display:none;">';
        html += '<button type="button" class="week-menu-item week-leave-out-btn" data-action="leave-out" data-key="' + esc(r.key) + '"><span class="week-menu-item-label">leave out of this week</span><span class="week-menu-hint">only this week changes. the memory stays in your journal.</span></button>';
        html += '<a class="week-menu-item" href="' + esc(r.peek_href) + '"><span class="week-menu-item-label">this isn\'t right</span><span class="week-menu-hint">opens where it came from, so you can check it.</span></a>';
        html += '</div>';
        html += '</div>'; // week-menu-wrapper
        html += '</div>'; // week-row-actions
        html += '</div>'; // week-row-header
        html += '<div class="week-row-text">' + markdown(r.text) + '</div>';
        if (r.peek_caption) {
          html += '<div class="week-row-peek"><a href="' + esc(r.peek_href) + '">' + esc(r.peek_caption) + ' · ' + esc(r.peek_label) + '</a></div>';
        }
        html += '</div>'; // week-row
      }
    });
    html += '</div>'; // week-rows

    html += '<p class="week-end-line">' + esc(model.end_line || "that's the week.") + '</p>';

    if (model.prev_href) {
      html += '<p class="week-prev-repeat"><a href="' + esc(model.prev_href) + '">' + esc(model.prev_label) + '</a></p>';
    }

    html += '</div>';
    return html;
  }

  function wireWeekEvents(container, model) {
    const day = model.day;

    // Menu toggle
    container.querySelectorAll('.week-menu-wrapper').forEach(function (wrapper) {
      const btn = wrapper.querySelector('.week-menu-btn');
      const dropdown = wrapper.querySelector('.week-menu-dropdown');
      if (!btn || !dropdown) return;

      btn.addEventListener('click', function (e) {
        e.stopPropagation();
        const isOpen = btn.getAttribute('aria-expanded') === 'true';
        closeOpenMenu();
        if (!isOpen) {
          btn.setAttribute('aria-expanded', 'true');
          dropdown.hidden = false;
          dropdown.style.display = 'block';
          activeMenu = { btn, dropdown, wrapper };
        }
      });
    });

    // Leave-out and undo buttons
    function sendLeaveOut(key, undo, triggerBtn) {
      closeOpenMenu();
      const row = container.querySelector('[data-memory-key="' + key + '"]');
      if (row) {
        const existingErr = row.querySelector('.week-row-error');
        if (existingErr) existingErr.remove();
      }

      fetch('/app/home/api/week/' + encodeURIComponent(day) + '/leave-out', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({ key: key, undo: undo })
      })
        .then(function (res) {
          if (!res.ok) {
            return res.json().then(function (data) {
              throw new Error(data.error || (undo ? "couldn't bring this back. nothing changed." : "couldn't leave this out. nothing changed."));
            }).catch(function (err) {
              throw new Error(err.message || (undo ? "couldn't bring this back. nothing changed." : "couldn't leave this out. nothing changed."));
            });
          }
          return res.json();
        })
        .then(function (newModel) {
          renderWeekPage(container, newModel);
          if (undo) {
            const restoredRow = container.querySelector('[data-memory-key="' + key + '"]');
            const menuBtn = restoredRow ? restoredRow.querySelector('.week-menu-btn') : null;
            if (menuBtn) menuBtn.focus();
          } else {
            const undoBtn = container.querySelector('[data-action="undo"][data-key="' + key + '"]');
            if (undoBtn) undoBtn.focus();
          }
        })
        .catch(function (_postErr) {
          // POST failed: re-fetch to see if disk state changed
          fetch('/app/home/api/week/' + encodeURIComponent(day))
            .then(function (res) {
              if (!res.ok) {
                throw new Error("couldn't load this week. reload to try again.");
              }
              return res.json();
            })
            .then(function (freshModel) {
              const matchingRow = (freshModel.rows || []).find(function (r) { return r.key === key; });
              const expectedLeftOut = !undo;
              if (matchingRow && matchingRow.left_out === expectedLeftOut) {
                // State changed on disk despite error response
                renderWeekPage(container, freshModel);
                if (undo) {
                  const restoredRow = container.querySelector('[data-memory-key="' + key + '"]');
                  const menuBtn = restoredRow ? restoredRow.querySelector('.week-menu-btn') : null;
                  if (menuBtn) menuBtn.focus();
                } else {
                  const undoBtn = container.querySelector('[data-action="undo"][data-key="' + key + '"]');
                  if (undoBtn) undoBtn.focus();
                }
              } else {
                // State did not change: show inline row error
                const targetRow = container.querySelector('[data-memory-key="' + key + '"]');
                if (targetRow) {
                  const errP = document.createElement('p');
                  errP.className = 'week-row-error';
                  errP.setAttribute('role', 'alert');
                  errP.textContent = undo ? "couldn't bring this back. nothing changed." : "couldn't leave this out. nothing changed.";
                  targetRow.appendChild(errP);
                }
                if (triggerBtn) triggerBtn.focus();
              }
            })
            .catch(function () {
              container.innerHTML = '<div class="surface-state surface-state--error" role="alert">'
                + '<h2 class="surface-state-heading">couldn\'t load this week. reload to try again.</h2>'
                + '</div>';
            });
        });
    }

    container.querySelectorAll('[data-action="leave-out"]').forEach(function (btn) {
      btn.addEventListener('click', function (e) {
        e.preventDefault();
        const key = btn.getAttribute('data-key');
        if (key) sendLeaveOut(key, false, btn);
      });
    });

    container.querySelectorAll('[data-action="undo"]').forEach(function (btn) {
      btn.addEventListener('click', function (e) {
        e.preventDefault();
        const key = btn.getAttribute('data-key');
        if (key) sendLeaveOut(key, true, btn);
      });
    });
  }

  function renderWeekPage(container, model) {
    container.innerHTML = renderWeekHtml(model);
    wireWeekEvents(container, model);
  }

  function initWeek(stem) {
    const surface = document.querySelector('[data-pulse-surface]') || document.querySelector('[data-home-root]');
    if (!surface) return;
    surface.innerHTML = '<div class="surface-state surface-state--loading" role="status" aria-busy="true">'
      + '<div class="surface-state-spinner" aria-hidden="true"></div>'
      + '<span class="surface-state-text">loading your week…</span>'
      + '</div>';
    fetch('/app/home/api/week/' + encodeURIComponent(stem))
      .then(function (res) {
        if (!res.ok) {
          throw new Error("couldn't load this week. reload to try again.");
        }
        return res.json();
      })
      .then(function (model) {
        renderWeekPage(surface, model);
      })
      .catch(function () {
        surface.innerHTML = '<div class="surface-state surface-state--error" role="alert">'
          + '<h2 class="surface-state-heading">couldn\'t load this week. reload to try again.</h2>'
          + '</div>';
      });
  }

  window.initWeek = initWeek;
  window.renderWeekPage = renderWeekPage;
  window.renderWeekHtml = renderWeekHtml;
})();
