Open these HTML files directly in a browser; each assertion reports pass/fail inline. `support.html` is the exception: with the convey server running, open `/static/tests/support.html` so its server-mounted script can load.

- `api.html`
- `activities-participation-render.html`
- `diagnostic-console.html`
- `date-nav.html`: date labels, log1p heat, year totals, unit pluralization, and dayless fallback
- `date_format.html`: relative labels, last-week weekday collapse, and year suffixes
- `day-grid.html`: year blocks, legend, keyboard, peek, anchor, remount, and range selection
- `drawer.html`
- `gate-drawer.html`: reason lines, metric rows, missing metrics, action HTML, and open-state preservation
- `mount-workspace.html`
- `markdown-resource-policy.html`: remote markdown and HTML resource URLs are removed, no form control or submission target survives, and ordinary text, task-list state and local images survive. The form and task-list cases also run in `make ci-full` as `tests/markdown_form_policy_dom.js` under plain Node
- `quiet-notifs-disclosure.html`: manual, not CI-gated
- `relative-time.html`
- `status-pane-label.html`
- `sunarc.html`: the sun arc in a real document (mount, timers, listeners, the ground it paints). Its pure day/night math also runs without a browser in `make ci-full`, as `tests/sunarc_frame.js` under plain Node
- `support.html`: exact local report card, editable recent lines, and fragment-only handoff
- `surface-state.html`
- `ws-listen.html`
- `register-task.html`
