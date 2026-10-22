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
  '  window.toggleBriefingCard = toggleBriefingCard;\n',
  `  window.__connections = { renderConnectionsHtml };
  window.toggleBriefingCard = toggleBriefingCard;\n`,
);

const location = { href: '' };
const document = {
  readyState: 'loading',
  addEventListener() {},
  querySelector() { return null; },
};
const window = {
  location,
  document,
  addEventListener() {},
  JournalFormat: {
    day: function (day) { return String(day || ''); },
  },
};
window.window = window;
document.defaultView = window;

vm.runInNewContext(source, { window, document, console }, { filename: 'home.js' });
const { renderConnectionsHtml } = window.__connections;
assert(renderConnectionsHtml, 'renderConnectionsHtml exported');

function getShelf(html, name) {
  const marker = `data-shelf="${name}"`;
  const start = html.indexOf(marker);
  if (start === -1) return null;
  const rest = html.slice(start);
  const nextShelf = rest.indexOf('data-shelf=', marker.length);
  const nextDetails = rest.indexOf('<details');
  let end = rest.length;
  if (nextShelf !== -1 && nextShelf < end) end = nextShelf;
  if (nextDetails !== -1 && nextDetails < end) end = nextDetails;
  return rest.slice(0, end);
}

const ada = { entity_id: 'ada', name: 'Ada Journal', evidence_class: 'semantic', count: 2, last_seen: '20260501', kinds: [{ kind: 'works-with' }] };
const bea = { entity_id: 'bea', name: 'Bea Events', evidence_class: 'attendance', count: 3, last_seen: '20260502', kinds: [{ kind: 'attended-with' }] };
const cal = { entity_id: 'cal', name: 'Cal Calendar', evidence_class: 'scheduled', count: 4, last_seen: '20260503', kinds: [{ kind: 'scheduled-with' }] };
const dee = { entity_id: 'dee', name: 'Dee Mention', evidence_class: 'semantic', count: 1, last_seen: '20260504', kinds: [{ kind: 'mentioned' }] };

// Case 1: All four neighbours, total: 4
{
  const pulse = {
    connections: {
      state: 'ok',
      total: 4,
      neighbors: [ada, bea, cal, dee],
      kind_words: {},
      attendance_kinds: ['attended-with', 'co-present', 'scheduled-with'],
    },
  };
  const html = renderConnectionsHtml(pulse);
  const journalIdx = html.indexOf('data-shelf="journal"');
  const attendanceIdx = html.indexOf('data-shelf="attendance"');
  const calendarIdx = html.indexOf('data-shelf="calendar"');
  assert(journalIdx !== -1 && attendanceIdx !== -1 && calendarIdx !== -1, 'all three shelves present');
  assert(journalIdx < attendanceIdx && attendanceIdx < calendarIdx, 'shelf order journal -> attendance -> calendar');

  const journalShelf = getShelf(html, 'journal');
  const attendanceShelf = getShelf(html, 'attendance');
  const calendarShelf = getShelf(html, 'calendar');

  assert(!journalShelf.includes('Cal Calendar'), 'journal shelf does not contain Cal Calendar');
  assert(calendarShelf.includes('Cal Calendar'), 'calendar shelf contains Cal Calendar');
  assert(!calendarShelf.includes('pulse-connections-cluster-count'), 'calendar shelf has no cluster count');
  assert(attendanceShelf.includes('pulse-connections-cluster-count'), 'attendance shelf has cluster count');
  assert(journalShelf.includes('pulse-connections-shelf-label'), 'journal shelf has label element');
  assert(!html.includes('pulse-connections-more'), 'more link absent when total == rendered');
}

// Case 2: Same four neighbours, total: 5
{
  const pulse = {
    connections: {
      state: 'ok',
      total: 5,
      neighbors: [ada, bea, cal, dee],
      kind_words: {},
      attendance_kinds: ['attended-with', 'co-present', 'scheduled-with'],
    },
  };
  const html = renderConnectionsHtml(pulse);
  assert(html.includes('pulse-connections-more'), 'more link present when total > rendered');
}

// Case 3: Relationship only (Ada Journal, total: 1)
{
  const pulse = {
    connections: {
      state: 'ok',
      total: 1,
      neighbors: [ada],
      kind_words: {},
      attendance_kinds: ['attended-with', 'co-present', 'scheduled-with'],
    },
  };
  const html = renderConnectionsHtml(pulse);
  assert(html.includes('data-shelf="journal"'), 'journal shelf present');
  assert(!html.includes('pulse-connections-shelf-label'), 'shelf label absent on relationship-only card');
  assert(!html.includes('data-shelf="calendar"'), 'calendar shelf absent');
  assert(!html.includes('data-shelf="attendance"'), 'attendance shelf absent');
}

// Case 4: Relationship plus mention, no attendance, no scheduled (total: 2)
{
  const pulse = {
    connections: {
      state: 'ok',
      total: 2,
      neighbors: [ada, dee],
      kind_words: {},
      attendance_kinds: ['attended-with', 'co-present', 'scheduled-with'],
    },
  };
  const html = renderConnectionsHtml(pulse);
  assert(html.includes('data-shelf="journal"'), 'journal shelf present');
  assert(!html.includes('pulse-connections-shelf-label'), 'shelf label absent with relationship + mention only');
  assert(html.includes('<details'), 'details present for mention');
}

// Case 5: Scheduled only (Cal Calendar, total: 1)
{
  const pulse = {
    connections: {
      state: 'ok',
      total: 1,
      neighbors: [cal],
      kind_words: {},
      attendance_kinds: ['attended-with', 'co-present', 'scheduled-with'],
    },
  };
  const html = renderConnectionsHtml(pulse);
  assert(html.includes('data-shelf="calendar"'), 'calendar shelf present');
  assert(!html.includes('pulse-connections-note'), 'empty note absent on scheduled-only card');
  assert(!html.includes('data-shelf="journal"'), 'journal shelf absent');
  assert(!html.includes('data-shelf="attendance"'), 'attendance shelf absent');
}

// Case 6: Nine scheduled neighbours named Cal1 through Cal9, in that order, total: 9
{
  const calNeighbors = [];
  for (let i = 1; i <= 9; i++) {
    calNeighbors.push({
      entity_id: 'cal' + i,
      name: 'Cal' + i,
      evidence_class: 'scheduled',
      count: 1,
      last_seen: '20260503',
      kinds: [{ kind: 'scheduled-with' }],
    });
  }
  const pulse = {
    connections: {
      state: 'ok',
      total: 9,
      neighbors: calNeighbors,
      kind_words: {},
      attendance_kinds: ['attended-with', 'co-present', 'scheduled-with'],
    },
  };
  const html = renderConnectionsHtml(pulse);
  const calendarShelf = getShelf(html, 'calendar');
  assert(calendarShelf, 'calendar shelf present for 9 scheduled neighbors');
  assert(calendarShelf.includes('Cal1'), 'calendar shelf contains Cal1');
  assert(calendarShelf.includes('Cal8'), 'calendar shelf contains Cal8');
  assert(!calendarShelf.includes('Cal9'), 'calendar shelf does not contain Cal9 (capped at 8)');
}
