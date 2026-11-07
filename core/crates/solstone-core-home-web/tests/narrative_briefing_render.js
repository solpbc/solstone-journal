// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const manifestDir = process.argv[2];
if (!manifestDir) throw new Error('manifest directory required');

let source = fs.readFileSync(path.join(manifestDir, 'assets/home.js'), 'utf8');
source = source.replace(
  '  window.selectAccountSide = selectAccountSide;\n',
  `  window.__home = { renderNarrativeHtml, briefingPlaceholderHtml, renderYesterdaySideHtml, renderWeeklyReflectionHtml, writtenAtTime };
  window.selectAccountSide = selectAccountSide;\n`,
);

const location = { href: '' };
const document = {
  readyState: 'loading',
  addEventListener() {},
  querySelector() { return null; },
  getElementById() { return null; },
};
const window = { createViewReady: () => () => {},
  location,
  document,
  addEventListener() {},
};
window.window = window;
document.defaultView = window;

vm.runInNewContext(source, { window, document, console }, { filename: 'home.js' });
const { renderNarrativeHtml, briefingPlaceholderHtml, renderYesterdaySideHtml, renderWeeklyReflectionHtml, writtenAtTime } = window.__home;
assert(renderNarrativeHtml, 'renderNarrativeHtml exported');
assert(briefingPlaceholderHtml, 'briefingPlaceholderHtml exported');
assert(renderWeeklyReflectionHtml, 'renderWeeklyReflectionHtml exported');

// Weekly reflection card assertions with AppServices unset
const weekWithMemory = renderWeeklyReflectionHtml({
  latest_weekly_reflection: {
    state: 'week',
    title: 'week of august 10',
    url: '/app/home/week/20260810',
    memory: 'a kept line',
    empty: null
  }
});
assert(weekWithMemory.includes('your week'), 'contains your week');
assert(weekWithMemory.includes('week of august 10'), 'contains title');
assert(weekWithMemory.includes('a kept line'), 'contains memory text');
assert(weekWithMemory.includes('open your week →'), 'contains open link');

const weekEmpty = renderWeeklyReflectionHtml({
  latest_weekly_reflection: {
    state: 'week',
    title: 'week of august 10',
    url: '/app/home/week/20260810',
    memory: null,
    empty: 'nothing from this week is showing.'
  }
});
assert(weekEmpty.includes('nothing from this week is showing.'), 'contains empty sentence');
assert(weekEmpty.includes('open your week →'), 'contains open link');
assert.strictEqual(weekEmpty.includes('a kept line'), false, 'does not contain memory');

const weekBothNull = renderWeeklyReflectionHtml({
  latest_weekly_reflection: {
    state: 'week',
    title: 'week of august 10',
    url: '/app/home/week/20260810',
    memory: null,
    empty: null
  }
});
assert(weekBothNull.includes('your week'), 'contains your week');
assert(weekBothNull.includes('open your week →'), 'contains open link');
assert.strictEqual(weekBothNull.includes('nothing from this week is showing.'), false, 'does not contain empty sentence');

const weekMemoryMarkdown = renderWeeklyReflectionHtml({
  latest_weekly_reflection: {
    state: 'week',
    title: 'week of august 10',
    url: '/app/home/week/20260810',
    memory: null,
    empty: null,
    memory_markdown: 'should not appear'
  }
});
assert.strictEqual(weekMemoryMarkdown.includes('should not appear'), false, 'does not contain memory_markdown');

const weekUnreadable = renderWeeklyReflectionHtml({
  latest_weekly_reflection: {
    state: 'unreadable',
    line: "the week of september 20 couldn't be read."
  }
});
assert(weekUnreadable.includes("the week of september 20 couldn&#39;t be read.") || weekUnreadable.includes("the week of september 20 couldn't be read."), 'contains unreadable line');
assert.strictEqual(weekUnreadable.includes('open your week'), false, 'does not contain open your week');

const weekUnchecked = renderWeeklyReflectionHtml({
  latest_weekly_reflection: {
    state: 'unchecked',
    line: "your week couldn't be checked."
  }
});
assert(weekUnchecked.includes("your week couldn&#39;t be checked.") || weekUnchecked.includes("your week couldn't be checked."), 'contains unchecked line');

const weekFirst = renderWeeklyReflectionHtml({
  latest_weekly_reflection: {
    state: 'first',
    line: 'your first week comes on monday.'
  }
});
assert(weekFirst.includes('your first week comes on monday.'), 'contains first week line');

// A day with no audio in it yet is not a fault, so it offers no fault recovery:
// the health page is green and sending the owner there teaches them to ignore
// recovery links.
const emptyDay = renderNarrativeHtml({ narrative_content: null, segment_count: 0 });
assert(emptyDay.includes('no narrative yet'), emptyDay);
assert.strictEqual(emptyDay.includes('/app/health'), false, emptyDay);
assert.strictEqual(emptyDay.includes('check system health'), false, emptyDay);

// A day that has audio but no analysis yet is a third state again, and still
// offers no recovery.
const awaitingAnalysis = renderNarrativeHtml({ narrative_content: null, segment_count: 4 });
assert(awaitingAnalysis.includes('analysis will be available after the next processing cycle.'), awaitingAnalysis);
assert.strictEqual(awaitingAnalysis.includes('/app/health'), false, awaitingAnalysis);

// The yesterday side never renders empty: any hour with no account and no
// not-yet state says it wasn't written up, with the run. X-04.
for (const phase of ['eod', 'active', 'missing']) {
  const html = briefingPlaceholderHtml(
    { phase, exists: false },
    { briefing_lateness: { late: true, late_hours: 1 }, briefing_analysis_day: '20260906' },
  );
  assert(html.includes('/app/thinking/#runs/20260906/morning_briefing'), `${phase}: ${html}`);
  assert(html.includes('pulse-briefing-status-link'), `${phase}: ${html}`);
}
assert.strictEqual(briefingPlaceholderHtml({ phase: 'pending', exists: true }, {}), '');

// Being written: one line, no link. Late: the run linked.
const pending = briefingPlaceholderHtml({ phase: 'pending', exists: false }, { briefing_lateness: { late: false } });
assert(pending.includes('pulse-briefing-placeholder'), pending);
assert.strictEqual(pending.includes('<a'), false, pending);
const late = briefingPlaceholderHtml(
  { phase: 'pending', exists: false },
  { briefing_lateness: { late: true, late_hours: 3 }, today: '20260906', briefing_analysis_day: '20260905' },
);
assert(late.includes('3h'), late);
assert(late.includes('/app/thinking/#runs/20260905/morning_briefing'), late);
assert.strictEqual(/\bI\b|I'm|\bmy\b/.test(late), false, late);

// Before the nightly run's first chance, the side says when, never "late" or
// "wasn't written" (the day-0 rule), and a saved lane says which.
for (const phase of ['awaiting_engine', 'first_night', 'first_night_empty']) {
  const html = briefingPlaceholderHtml({ phase, exists: false }, { briefing_lateness: { late: true, late_hours: 3 } });
  assert(html.includes('pulse-briefing-placeholder'), `${phase}: ${html}`);
  assert.strictEqual(html.includes('late'), false, html);
  assert.strictEqual(html.includes('<a'), false, html);
}
assert.notStrictEqual(
  briefingPlaceholderHtml({ phase: 'awaiting_engine', exists: false, saved_lane: 'local' }, {}),
  briefingPlaceholderHtml({ phase: 'awaiting_engine', exists: false }, {}),
);

// An account shows its yesterday section, when it was written and how. It
// never shows a forecast or the dropped needs-attention section.
const ready = renderYesterdaySideHtml(
  {
    phase: 'eod', exists: true, analysis_day: '20261023',
    meta: { generated: '2026-10-24T07:12:00' },
    sections: { yesterday: '- a steady morning.', your_day: 'a planned call.', needs_attention: '- a pipeline gap' },
  },
  { briefing_analysis_day: '20261023' },
);
assert(ready.includes('a steady morning.'), ready);
assert(ready.includes('pulse-briefing-body'), ready);
assert(ready.includes('/app/thinking/#runs/20261023/morning_briefing'), ready);
assert(ready.includes('7:12'), ready);
assert.strictEqual(ready.includes('a planned call.'), false, ready);
assert.strictEqual(ready.includes('a pipeline gap'), false, ready);
assert.strictEqual(ready.includes('pulse-briefing-status-link'), false, ready);

// An account whose yesterday section is empty is still an account: it says
// so plainly, with its run, and never "wasn't written up".
const quiet = renderYesterdaySideHtml(
  { phase: 'active', exists: true, analysis_day: '20261023', meta: null, sections: {} },
  { briefing_analysis_day: '20261023' },
);
assert(quiet.includes('pulse-briefing-placeholder'), quiet);
assert(quiet.includes('pulse-tell-more'), quiet);
assert.strictEqual(quiet.includes('pulse-briefing-status-link'), false, quiet);

// The day's body lines sit after the briefing and its stamp, under their own
// label, and never inside what the briefing wrote. A day with no briefing
// keeps its placeholder and still shows them; a day with none shows nothing.
const lines = [
  { name: 'running', start: { time: '07:00', date: null }, end: { time: '07:40', date: null },
    distance: '5.0 km', from: ['Synthetic Watch', '<b>Ring</b>'], logged: false, part_removed: false },
  { name: 'sleep', start: { time: '22:10', date: null }, end: { time: '06:20', date: '20261024' },
    distance: null, from: ['Synthetic Watch'], logged: false, part_removed: true },
  { name: 'mindfulness', start: { time: '07:00', date: null }, end: { time: '07:10', date: null },
    distance: null, from: [], logged: true, part_removed: false },
];
const withBody = renderYesterdaySideHtml(
  {
    phase: 'eod', exists: true, analysis_day: '20261023',
    meta: { generated: '2026-10-24T07:12:00' },
    sections: { yesterday: '- a steady morning.' },
    body: lines,
  },
  { briefing_analysis_day: '20261023' },
);
// Every time on the panel is said one way: the briefing's stamp and the lines.
const clock = (h, m) => new Date(2000, 0, 1, h, m).toLocaleString(undefined, { hour: 'numeric', minute: '2-digit' }).toLowerCase();
assert(withBody.includes('from your devices and apps'), withBody);
assert(withBody.includes('running, ' + clock(7, 0) + ' to ' + clock(7, 40) + ', 5.0 km, from Synthetic Watch and &lt;b&gt;Ring&lt;/b&gt;.'), withBody);
assert(withBody.includes('sleep, ' + clock(22, 10) + ' to ' + clock(6, 20) + ' on oct 24, from Synthetic Watch, part of it removed.'), withBody);
assert(withBody.includes('mindfulness, ' + clock(7, 0) + ' to ' + clock(7, 10) + ', which you logged.'), withBody);
assert(withBody.indexOf(clock(7, 12)) < withBody.indexOf('from your devices and apps'), withBody);
assert.strictEqual(withBody.includes('<b>Ring</b>'), false, withBody);
const notYet = renderYesterdaySideHtml(
  { phase: 'eod', exists: false, analysis_day: '20261023', meta: null, sections: {}, body: lines },
  { briefing_analysis_day: '20261023' },
);
assert(notYet.includes('running, '), notYet);
assert.strictEqual(notYet.includes('written at'), false, notYet);
assert.strictEqual(ready.includes('from your devices and apps'), false, ready);

// A yesterday that owes no briefing: nothing on the side says one is late,
// pending, due or missing, and the lines stand alone. With none, it says only
// that there was nothing to write up.
const bodyOnly = renderYesterdaySideHtml(
  { phase: 'none_owed', exists: false, analysis_day: '20261023', meta: null, sections: {}, body: lines },
  { briefing_lateness: { late: true, late_hours: 3 }, briefing_analysis_day: '20261023' },
);
assert(bodyOnly.includes('running, '), bodyOnly);
assert.strictEqual(bodyOnly.includes('pulse-briefing-placeholder'), false, bodyOnly);
assert.strictEqual(/written|late|being|shows here|<a/.test(bodyOnly), false, bodyOnly);
const nothingOwed = renderYesterdaySideHtml(
  { phase: 'none_owed', exists: false, analysis_day: '20261023', meta: null, sections: {}, body: [] },
  { briefing_lateness: { late: true, late_hours: 3 }, briefing_analysis_day: '20261023' },
);
assert(nothingOwed.includes('nothing from yesterday to write up.'), nothingOwed);
assert.strictEqual(/late|being|shows here|<a/.test(nothingOwed), false, nothingOwed);
// A capture day whose briefing is late keeps saying so, lines or not.
const lateWithLines = renderYesterdaySideHtml(
  { phase: 'pending', exists: false, analysis_day: '20261023', meta: null, sections: {}, body: lines },
  { briefing_lateness: { late: true, late_hours: 3 }, briefing_analysis_day: '20261023' },
);
assert(lateWithLines.includes('3h late'), lateWithLines);
assert(lateWithLines.indexOf('3h late') < lateWithLines.indexOf('running, '), lateWithLines);

// The stamp is the journal's wall time: its hour and minute as written,
// whatever zone the browser is in.
assert(writtenAtTime('2026-10-24T07:12:00').includes('7:12'));
assert(writtenAtTime('2026-10-24T19:05:00').includes('7:05'));
assert.strictEqual(writtenAtTime(null), '');
assert.strictEqual(writtenAtTime('not a time'), '');

// The today side's account is named by its tab, not by the run that wrote
// it. X-03.
const live = renderNarrativeHtml({
  narrative_content: 'the morning remains steady.',
  narrative_header: 'pulse',
  segment_count: 4,
  today: '20260906',
});
assert.strictEqual(live.includes('pulse-section-header'), false, live);
assert.strictEqual(live.toLowerCase().includes('>pulse<'), false, live);
assert(live.includes('pulse-tell-more'), live);


const scopeBase = { narrative_content: 'current evidence', narrative_source: 'pulse', today: '20260907' };
const scoped = renderNarrativeHtml({...scopeBase, narrative_window: {segments: 2, activities: 1, input_segments: 10, input_activities: 4, since_ms: null, gaps: ['<missing>']}});
assert(scoped.includes('2 of 10') && scoped.includes('1 of 4'));
assert(scoped.includes('&lt;missing&gt;') && !scoped.includes('<missing>'));
assert(renderNarrativeHtml(scopeBase).includes('wasn\'t saved'));
assert(renderNarrativeHtml({...scopeBase, narrative_window: {segments: -1}}).includes('wasn\'t saved'));
assert(!renderNarrativeHtml({...scopeBase, narrative_source: 'flow'}).includes('wasn\'t saved'));

const firstBatch = renderNarrativeHtml({...scopeBase, narrative_window: {segments: 1, activities: 0, input_segments: 1, input_activities: 0, since_ms: 0, gaps: []}});
assert(!firstBatch.includes('processing window starts'));

const markdownCalls = [];
window.AppServices = {
  renderMarkdown(text) {
    markdownCalls.push(text);
    return `<span class="marker">${text}</span>`;
  },
};
const itemText = '- Review item with sol://20260901/100000_300 reference';
const rendered = renderYesterdaySideHtml(
  { phase: 'morning', exists: true, analysis_day: '20260905', meta: null, sections: { yesterday: itemText } },
  { briefing_analysis_day: '20260905' },
);
assert(markdownCalls.includes(itemText), 'renderMarkdown called with the yesterday section');
assert(rendered.includes(`<span class="marker">${itemText}</span>`), rendered);

console.log('narrative and briefing render contract passed');
