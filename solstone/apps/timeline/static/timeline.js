// Timeline data comes from the real API; empty and failure states render visibly.
let months = [];
let realHourPlan = {};
let realDayPlan = {};
const segmentAvail = {};        // "monthIdx:day:hour" → buckets (12 entries)
const dayCache = new Map();      // "YYYYMMDD" → /app/timeline/api/day response
const segCache = new Map();      // "<day>/<stream>/<seg>" → /app/timeline/api/segment response
const monthCache = {};
const timelineMeta = { generatedAt: null, model: null, dataThrough: null };

const ACCENT_ROTATION = ["blue", "teal", "amber", "coral"];
const MONTH_FULL_NAMES = ["January","February","March","April","May","June","July","August","September","October","November","December"];

function isoDay(monthIndex, day) {
  const m = months[monthIndex];
  if (!m || !m.ym) return null;
  return m.ym + String(day).padStart(2, "0");
}
function isoToMonthIdx(yyyymm) {
  return months.findIndex((m) => m.ym === yyyymm);
}
function dayFromOrigin(origin) {
  const day8 = (origin || "").slice(0, 8);
  return /^\d{8}$/.test(day8) ? parseInt(day8.slice(6, 8), 10) : null;
}
function minuteFromOrigin(origin) {
  const seg = (origin || "").split("/").pop() || "";
  if (seg.length < 6 || !/^\d{6}/.test(seg)) return null;
  return parseInt(seg.slice(2, 4), 10);
}
function originParts(origin) {
  if (!origin || typeof origin !== "string") return null;
  if (origin.length < 8) return null;
  const day = origin.slice(0, 8);
  if (!/^\d{8}$/.test(day)) return null;
  const last = origin.split("/").pop();
  if (!last || !/^\d{6}/.test(last)) return null;
  return { day, hh: last.slice(0, 2), mm: last.slice(2, 4) };
}

function segmentCountFromHoursAvail(hoursAvail) {
  return Object.values(hoursAvail || {}).reduce((total, hour) => {
    return total + (hour.buckets || []).reduce((sum, bucket) => sum + (bucket.segment_count || 0), 0);
  }, 0);
}

function formatDateLabel(month, day) {
  return `${month.name} ${day}${month.year ? `, ${month.year}` : ""}`;
}

// Compose a wall-clock label from a "seconds-from-segment-start" offset
// anchored to a per-call meta {startSec}. Used by the river view.
function segmentTimeLabel(meta, secondsFromStart) {
  const total = meta.startSec + Math.floor(secondsFromStart);
  const hh = Math.floor(total / 3600) % 24;
  const mm = Math.floor((total % 3600) / 60);
  const ss = total % 60;
  return String(hh).padStart(2, "0") + ":" +
         String(mm).padStart(2, "0") + ":" +
         String(ss).padStart(2, "0");
}

// ── Data loaders (lazy, cached) ──────────────────────────────────────

async function loadIndex() {
  try {
    const res = await fetch("/app/timeline/api/index", { cache: "no-store" });
    if (!res.ok) {
      console.info(`/app/timeline/api/index failed (${res.status}); showing timeline error`);
      return { state: "error" };
    }
    let idx;
    try {
      idx = await res.json();
    } catch (e) {
      console.warn("/app/timeline/api/index returned unreadable JSON; showing timeline error", e);
      return { state: "error" };
    }
    timelineMeta.generatedAt = idx.generated_at ?? null;
    timelineMeta.model = idx.model ?? null;
    timelineMeta.dataThrough = idx.data_through ?? null;
    rebuildMonthsFromIndex(idx);
    const state = months.every((m) => !m.yearEvent && !(m.day_count > 0)) ? "empty" : "data";
    console.info(`loaded /app/timeline/api/index (${idx.months.length} months, year_top=${idx.year_top.length})`);
    return { state };
  } catch (e) {
    console.warn("/app/timeline/api/index fetch failed; showing timeline error", e);
    return { state: "error" };
  }
}

function rebuildMonthsFromIndex(idx) {
  const newMonths = idx.months.map((m, i) => {
    const fullName = MONTH_FULL_NAMES[m.month_num - 1];
    const head = (m.month_top || [])[0] || null;
    const yearEvent = head
      ? { title: head.title, text: head.description, origin: head.origin || "" }
      : null;
    return {
      name: fullName,
      short: fullName.slice(0, 3).toUpperCase(),
      year: m.year,
      month_num: m.month_num,
      ym: m.ym,
      accent: ACCENT_ROTATION[i % 4],
      days: m.days_in_month,
      first_weekday: m.first_weekday,
      side: i % 2 === 0 ? "top" : "bottom",
      yearEvent,
      dayEvents: {},
      day_count: m.day_count,
      days_with_data: new Set(m.days_with_data || []),
      daysWithData: new Set(m.days_with_data || []),
    };
  });
  months = newMonths;
}

async function loadMonth(ym) {
  if (Object.prototype.hasOwnProperty.call(monthCache, ym)) return monthCache[ym];
  try {
    const res = await fetch(`/app/timeline/api/month/${ym}`, { cache: "no-store" });
    if (!res.ok) {
      console.info(`/app/timeline/api/month/${ym} failed (${res.status}); using empty month`);
      monthCache[ym] = null;
      return null;
    }
    const payload = await res.json();
    monthCache[ym] = payload;
    const monthIndex = months.findIndex((m) => m.ym === ym);
    if (monthIndex >= 0) {
      const month = months[monthIndex];
      month.dayEvents = {};
      let toggle = true;
      for (const [day, info] of Object.entries(payload.days || {}).sort()) {
        const pickArr = info.day_top || [];
        const pick = pickArr[0] || null;
        if (!pick) continue;
        const dayNum = parseInt(day.slice(6, 8), 10);
        month.dayEvents[day] = {
          day: dayNum,
          side: toggle ? "top" : "bottom",
          title: pick.title,
          text: pick.description,
          origin: pick.origin || "",
        };
        toggle = !toggle;
      }
      const daysWithData = new Set(payload.days_with_data || []);
      month.days_with_data = daysWithData;
      month.daysWithData = daysWithData;
    }
    return payload;
  } catch (e) {
    console.warn(`/app/timeline/api/month/${ym} fetch failed`, e);
    monthCache[ym] = null;
    return null;
  }
}

async function loadDay(yyyymmdd) {
  if (dayCache.has(yyyymmdd)) return dayCache.get(yyyymmdd);
  let data = null;
  try {
    const res = await fetch(`/app/timeline/api/day/${yyyymmdd}`, { cache: "no-store" });
    if (res.ok) data = await res.json();
  } catch (e) { console.warn("loadDay failed", yyyymmdd, e); }
  if (!data) data = { day: yyyymmdd, day_top: [], hours: {}, hours_avail: {} };
  dayCache.set(yyyymmdd, data);
  // Populate the prototype's per-render lookups.
  const monthIdx = isoToMonthIdx(yyyymmdd.slice(0, 6));
  if (monthIdx >= 0) populateDayLookups(monthIdx, yyyymmdd, data);
  return data;
}

function clearMonthCache() {
  for (const key of Object.keys(monthCache)) delete monthCache[key];
}

function clearDayLookups(yyyymmdd) {
  const monthIdx = isoToMonthIdx(yyyymmdd.slice(0, 6));
  if (monthIdx < 0) return;
  const dayInt = parseInt(yyyymmdd.slice(6, 8), 10);
  const dayPrefix = `${monthIdx}:${dayInt}`;
  delete realDayPlan[dayPrefix];
  for (const key of Object.keys(realHourPlan)) {
    if (key.startsWith(`${dayPrefix}:`)) delete realHourPlan[key];
  }
  for (const key of Object.keys(segmentAvail)) {
    if (key.startsWith(`${dayPrefix}:`)) delete segmentAvail[key];
  }
}

function clearRollupCaches() {
  clearMonthCache();
  dayCache.clear();
  realHourPlan = {};
  realDayPlan = {};
  for (const key of Object.keys(segmentAvail)) delete segmentAvail[key];
}

function populateDayLookups(monthIdx, yyyymmdd, data) {
  const dayInt = parseInt(yyyymmdd.slice(6, 8), 10);
  // Day-view hour events: first pick of each hour with picks, alternating sides.
  const dayPlan = [];
  const seenOrigins = new Set();
  const eventByHour = new Map();
  let toggle = true;
  for (const hh of Object.keys(data.hours || {}).sort()) {
    const picks = data.hours[hh].picks || [];
    if (!picks.length) continue;
    const p = picks[0];
    const hour = parseInt(hh, 10);
    const event = {
      hour,
      side: toggle ? "top" : "bottom",
      kind: "work",
      title: p.title, text: p.description, origin: p.origin || "",
    };
    dayPlan.push(event);
    if (event.origin) seenOrigins.add(event.origin);
    eventByHour.set(hour, event);
    toggle = !toggle;
  }

  for (const pick of data.day_top || []) {
    const origin = pick && pick.origin;
    if (!origin) {
      console.warn("timeline: day_top pick missing origin", pick);
      continue;
    }
    const parts = origin.split("/");
    const segName = parts[parts.length - 1] || "";
    const match = /^(\d{2})/.exec(segName);
    if (!match) {
      console.warn("timeline: day_top pick has malformed origin", origin);
      continue;
    }
    const hour = parseInt(match[1], 10);
    if (!(hour >= 0 && hour <= 23)) {
      console.warn("timeline: day_top pick has out-of-range hour", origin);
      continue;
    }
    if (seenOrigins.has(origin)) continue;

    let side;
    const existing = eventByHour.get(hour);
    if (existing) {
      side = existing.side;
    } else {
      side = toggle ? "top" : "bottom";
      toggle = !toggle;
    }

    const event = {
      hour,
      side,
      kind: "work",
      title: pick.title,
      text: pick.description,
      origin,
    };
    dayPlan.push(event);
    seenOrigins.add(origin);
    if (!eventByHour.has(hour)) eventByHour.set(hour, event);
  }

  dayPlan.sort((a, b) => a.hour - b.hour);
  realDayPlan[`${monthIdx}:${dayInt}`] = dayPlan;
  // Hour view minute events.
  for (const [hh, hd] of Object.entries(data.hours || {})) {
    const picks = hd.picks || [];
    if (!picks.length) continue;
    realHourPlan[`${monthIdx}:${dayInt}:${parseInt(hh, 10)}`] = pickListToMinutePlan(picks);
  }
  // Per-cell availability: drives hour-view tinting + click gating.
  for (const [hh, ha] of Object.entries(data.hours_avail || {})) {
    segmentAvail[`${monthIdx}:${dayInt}:${parseInt(hh, 10)}`] = ha.buckets;
  }
}

function pickListToMinutePlan(picks) {
  const used = new Set();
  const fallbackSlots = [5, 20, 35, 50];
  const out = [];
  picks.slice(0, 4).forEach((p, i) => {
    let slot;
    const m = minuteFromOrigin(p.origin);
    if (m == null) slot = fallbackSlots[i];
    else slot = Math.max(0, Math.min(55, Math.floor(m / 5) * 5));
    const orig = slot;
    while (used.has(slot) && slot < 55) slot += 5;
    if (used.has(slot)) {
      slot = orig;
      while (used.has(slot) && slot > 0) slot -= 5;
    }
    used.add(slot);
    out.push({
      minute: slot,
      side: i % 2 === 0 ? "top" : "bottom",
      title: p.title, text: p.description, origin: p.origin || "",
    });
  });
  out.sort((a, b) => a.minute - b.minute);
  return out;
}

async function loadSegment(origin) {
  if (segCache.has(origin)) return segCache.get(origin);
  try {
    const res = await fetch(`/app/timeline/api/segment/${origin}`, { cache: "no-store" });
    if (!res.ok) return null;
    const data = await res.json();
    segCache.set(origin, data);
    return data;
  } catch (e) { console.warn("loadSegment failed", origin, e); return null; }
}

// Frame category → CSS color variable (shared with the prototype palette).
const SCREEN_CATEGORY_COLOR = {
  terminal:    "var(--ink)",
  code:        "var(--ink)",
  coding:      "var(--ink)",
  browsing:    "var(--teal)",
  productivity:"var(--amber)",
  reading:     "var(--muted)",
  messaging:   "var(--coral)",
  meeting:     "var(--teal)",
  media:       "var(--coral)",
  other:       "var(--muted)",
};
function categoryColor(primary) {
  return SCREEN_CATEGORY_COLOR[(primary || "").toLowerCase()] || "var(--muted)";
}

// Featured = frames with extracted text content (the meaningful ones to
// surface as visible serif marginalia). Non-featured render as ticks only.
function isFeatured(frame) {
  return !!(frame.content && Object.keys(frame.content).length);
}

// Excerpt the most important content from a frame for the inline detail
// panel — visual_description first, then any text content.
function frameDetailText(frame) {
  const a = frame.analysis || {};
  const c = frame.content || {};
  const parts = [];
  if (a.visual_description) parts.push(a.visual_description);
  for (const [k, v] of Object.entries(c)) {
    if (typeof v === "string") parts.push(`[${k}]\n${v}`);
  }
  return parts.join("\n\n");
}

function clearActiveMarks() {
  for (const el of document.querySelectorAll(".river-tick.is-active, .river-audio-dot.is-active")) {
    el.classList.remove("is-active");
  }
}

// The river renderer stashes the rendered segment's data here so the
// click-driven detail handlers can find frames + transcript lines
// without re-fetching.
let _activeSegment = null;
let _activeMeta = null;

function showSegmentDetail(frameId) {
  const detail = document.getElementById("segment-detail");
  if (!detail || !_activeSegment || !_activeSegment.screen) return;
  const frame = _activeSegment.screen.frames.find((f) => f.frame_id === frameId);
  if (!frame) return;
  const a = frame.analysis || {};
  const tLabel = segmentTimeLabel(_activeMeta, frame.timestamp || 0);
  const featured = isFeatured(frame);
  detail.innerHTML = `
    <div class="seg-detail-meta">
      <span class="seg-detail-time">${tLabel}</span>
      <span class="seg-detail-cat" style="--cat:${categoryColor(a.primary)}">${escapeHtml(a.primary || "?")}</span>
      <span class="seg-detail-frame">frame #${frame.frame_id}</span>
    </div>
    <div class="seg-detail-desc">${escapeHtml(a.visual_description || "")}</div>
    ${featured ? Object.entries(frame.content).map(([k, v]) =>
      `<div class="seg-detail-content">
         <div class="seg-detail-content-tag">${escapeHtml(k)}</div>
         <pre class="seg-detail-content-body">${escapeHtml(typeof v === "string" ? v : JSON.stringify(v, null, 2))}</pre>
       </div>`).join("") : ""}
  `;
  clearActiveMarks();
  const active = document.querySelector(`.river-tick[data-frame-id="${frameId}"]`);
  if (active) active.classList.add("is-active");
}

function showSegmentAudioDetail(audioIndex) {
  const detail = document.getElementById("segment-detail");
  if (!detail || !_activeSegment || !_activeSegment.audio) return;
  const lines = _activeSegment.audio.lines;
  const line = lines[audioIndex];
  if (!line) return;
  const sp = line.speaker || 1;
  const speakerColor = ["var(--blue)","var(--teal)","var(--coral)","var(--amber)"][sp - 1] || "var(--muted)";
  // Stitch together a small context window: 1 line before + this + 1 after
  const before = lines[audioIndex - 1];
  const after = lines[audioIndex + 1];
  const renderLine = (l, isFocus) =>
    l ? `<div class="seg-detail-line ${isFocus ? "is-focus" : ""}">
           <span class="seg-detail-line-time">${escapeHtml(l.start || "")}</span>
           <span class="seg-detail-line-sp">s${l.speaker || "?"}</span>
           <span class="seg-detail-line-text">${escapeHtml(l.corrected || l.text || "")}</span>
           ${l.emotion ? `<span class="seg-detail-line-emotion">${escapeHtml(l.emotion)}</span>` : ""}
         </div>` : "";
  detail.innerHTML = `
    <div class="seg-detail-meta">
      <span class="seg-detail-time">${escapeHtml(line.start || "")}</span>
      <span class="seg-detail-cat" style="--cat:${speakerColor}">speaker ${sp}</span>
      <span class="seg-detail-frame">audio #${audioIndex + 1} of ${lines.length}</span>
    </div>
    <div class="seg-detail-lines">
      ${renderLine(before, false)}
      ${renderLine(line, true)}
      ${renderLine(after, false)}
    </div>
  `;
  clearActiveMarks();
  const active = document.querySelector(`.river-audio-dot[data-audio-index="${audioIndex}"]`);
  if (active) active.classList.add("is-active");
}

function clearSegmentDetail() {
  const detail = document.getElementById("segment-detail");
  if (detail) detail.innerHTML = `<div class="seg-detail-empty">click a tick or audio dot on the river to see what sol observed at that moment</div>`;
  clearActiveMarks();
}

const timeline = document.querySelector("#timeline-root");
const timelineInitial = window.timelineInitial || { view: "year", day: null, month: null };
const { view: initialView, day: initialDay, month: initialMonth } = timelineInitial;
let currentView = initialView;
let selectedMonth = null;
let selectedDay = null;
let selectedHour = null;
let selectedMinute = null;

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function renderOriginChip(origin) {
  const parts = originParts(origin);
  if (!parts) return "";
  return `<a class="timeline-origin-chip" href="/app/activities/${parts.day}">→ ${parts.hh}:${parts.mm}</a>`;
}

function renderYearFooter(dataThrough) {
  if (!dataThrough || !/^\d{8}$/.test(dataThrough)) return "";
  const y = dataThrough.slice(0, 4);
  const m = parseInt(dataThrough.slice(4, 6), 10) - 1;
  const d = parseInt(dataThrough.slice(6, 8), 10);
  const date = new Date(Date.UTC(parseInt(y, 10), m, d));
  const monthName = date.toLocaleString("en-US", { month: "long", timeZone: "UTC" });
  return `<footer class="timeline-data-through">data through ${monthName} ${d}, ${y}</footer>`;
}

function renderDayProvenance(generatedAt, model) {
  if (!generatedAt || !model) return "";
  const date = new Date(generatedAt * 1000);
  const hh = String(date.getHours()).padStart(2, "0");
  const mm = String(date.getMinutes()).padStart(2, "0");
  const y = date.getFullYear();
  const mo = String(date.getMonth() + 1).padStart(2, "0");
  const da = String(date.getDate()).padStart(2, "0");
  return `<p class="timeline-day-provenance">rolled up at ${hh}:${mm} on ${y}-${mo}-${da} · ${escapeHtml(model)}</p>`;
}

function renderEmptyState(headline, body, opts = {}) {
  const classes = ["timeline-empty-state", opts.modifierClass || ""].filter(Boolean).join(" ");
  const link = opts.href && opts.linkText
    ? `<a href="${escapeHtml(opts.href)}">${escapeHtml(opts.linkText)}</a>`
    : "";
  return `
    <div class="${classes}" data-timeline-state="empty">
      <h2>${escapeHtml(headline)}</h2>
      <p>${escapeHtml(body)}</p>
      ${link}
    </div>
  `;
}

function renderErrorState() {
  return `
    <div class="timeline-empty-state" data-timeline-state="error" role="alert">
      <h2>couldn't reach the timeline service</h2>
      <p>reload to try again, or check whether sol is running</p>
      <a href="/app/health">system health →</a>
    </div>
  `;
}

function eventColumn(day, span, days) {
  const start = Math.max(1, Math.min(day - 2, days - span + 1));
  return `${start} / span ${span}`;
}

function hourColumn(hour, span = 4) {
  const start = Math.max(1, Math.min(hour + 1, 25 - span));
  return `${start} / span ${span}`;
}

function segmentColumn(minute, span = 3) {
  const index = Math.floor(minute / 5) + 1;
  const start = Math.max(1, Math.min(index, 13 - span));
  return `${start} / span ${span}`;
}

function getDayMeta(monthIndex, day) {
  // Use the real year from the dynamically-built months[] entry so
  // weekend computation matches the actual calendar (e.g., Jun 14
  // 2025 is a Saturday, but Jun 14 2026 is a Sunday).
  const m = months[monthIndex] || {};
  const year = m.year || 2025;
  const monthNum = (m.month_num != null ? m.month_num - 1 : monthIndex);
  const date = new Date(year, monthNum, day);
  const weekday = date.getDay();
  return {
    dayType: weekday === 0 || weekday === 6 ? "weekend" : "weekday",
  };
}

function formatHour(hour) {
  if (hour === 0) return "12a";
  if (hour < 12) return `${hour}a`;
  if (hour === 12) return "12p";
  return `${hour - 12}p`;
}

function formatTime(hour, minute = 0) {
  const suffix = hour < 12 ? "a" : "p";
  const normalizedHour = hour % 12 === 0 ? 12 : hour % 12;
  return `${normalizedHour}:${String(minute).padStart(2, "0")}${suffix}`;
}

function syncPathStateFromLocation() {
  const match = /^\/app\/timeline\/?([^/]*)$/.exec(window.location.pathname);
  let view = initialView;
  let day = initialDay;
  let month = initialMonth;

  if (match) {
    const value = match[1];
    if (value === "year") {
      view = "year";
      day = null;
      month = null;
    } else if (/^\d{8}$/.test(value)) {
      view = "day";
      day = value;
      month = null;
    } else if (/^\d{6}$/.test(value)) {
      view = "month";
      day = null;
      month = value;
    }
  }

  currentView = view;
  if (view === "day" && day) {
    selectedMonth = isoToMonthIdx(day.slice(0, 6));
    selectedDay = parseInt(day.slice(6, 8), 10);
  } else if (view === "month" && month) {
    selectedMonth = isoToMonthIdx(month);
    selectedDay = null;
  } else {
    selectedMonth = null;
    selectedDay = null;
  }
}

async function dispatchBootView() {
  if (currentView === "year") {
    renderYear();
    return;
  }

  if (currentView === "month") {
    const month = months[selectedMonth];
    if (!month) {
      timeline.innerHTML = renderEmptyState(
        "month not in timeline",
        "this month is outside the current timeline window.",
      );
      return;
    }
    await loadMonth(month.ym);
    await renderMonth(selectedMonth);
    return;
  }

  if (currentView === "day") {
    const day = selectedMonth !== null && selectedDay !== null
      ? isoDay(selectedMonth, selectedDay)
      : null;
    const month = months[selectedMonth];
    if (!day || !month) {
      timeline.innerHTML = renderEmptyState(
        "day not in timeline",
        "this day is outside the current timeline window.",
      );
      return;
    }
    await loadMonth(day.slice(0, 6));
    await loadDay(day);
    await renderDay(selectedMonth, selectedDay);
    return;
  }

  renderYear();
}

async function prefetchSegmentForMinute(hour, minute) {
  const buckets = segmentAvail[`${selectedMonth}:${selectedDay}:${hour}`] || [];
  const bucket = buckets[Math.floor(minute / 5)] || null;
  if (bucket && bucket.best_origin) {
    await loadSegment(bucket.best_origin);
  }
}

async function applyHash(hash) {
  syncPathStateFromLocation();
  if (!hash || hash === "#") {
    selectedHour = null;
    selectedMinute = null;
    return dispatchBootView();
  }

  const hourMatch = hash.match(/^#h=(\d{1,2})$/);
  if (hourMatch) {
    const hour = parseInt(hourMatch[1], 10);
    if (
      hour >= 0 &&
      hour <= 23 &&
      currentView === "day" &&
      Number.isInteger(selectedMonth) &&
      selectedMonth >= 0 &&
      Number.isInteger(selectedDay)
    ) {
      selectedHour = hour;
      selectedMinute = null;
      return renderMinute(selectedMonth, selectedDay, hour);
    }
  }

  const minuteMatch = hash.match(/^#m=(\d{4})$/);
  if (minuteMatch) {
    const hour = parseInt(minuteMatch[1].slice(0, 2), 10);
    const minute = parseInt(minuteMatch[1].slice(2, 4), 10);
    if (
      hour >= 0 &&
      hour <= 23 &&
      minute >= 0 &&
      minute <= 59 &&
      minute % 5 === 0 &&
      currentView === "day" &&
      Number.isInteger(selectedMonth) &&
      selectedMonth >= 0 &&
      Number.isInteger(selectedDay)
    ) {
      selectedHour = hour;
      selectedMinute = minute;
      await prefetchSegmentForMinute(hour, minute);
      return renderFiveMinute(selectedMonth, selectedDay, hour, minute);
    }
  }

  selectedHour = null;
  selectedMinute = null;
  return dispatchBootView();
}

function renderYear() {
  if (months.every((m) => !m.yearEvent && !(m.day_count > 0))) {
    timeline.innerHTML = renderEmptyState(
      "no timeline data yet",
      "once observers experience a day alongside you and sol rolls it up, that day will show up here",
      { href: "/app/health", linkText: "system health →" },
    );
    return;
  }

  timeline.innerHTML = `
    <div class="year-view">
      ${months
        .map(
          (month, index) => `
            <article class="milestone timeline-${month.side} accent-${month.accent}" style="grid-column: ${index + 1}">
              ${month.yearEvent ? `
                <div class="timeline-card">
                  <div class="timeline-date">${month.name} ${month.year || ""}</div>
                  <h2>${escapeHtml(month.yearEvent.title)}</h2>
                  <p>${escapeHtml(month.yearEvent.text)}</p>
                  ${renderOriginChip(month.yearEvent.origin)}
                </div>
              ` : ""}
              <button class="timeline-node" type="button" data-month="${index}" aria-label="Open ${month.name} ${month.year || ""}">
                ${month.short}
              </button>
            </article>
          `,
        )
        .join("")}
      ${renderYearFooter(timelineMeta.dataThrough)}
    </div>
  `;
}

async function renderMonth(index) {
  const month = months[index];
  const previous = index > 0 ? months[index - 1] : null;
  const next = index < months.length - 1 ? months[index + 1] : null;
  const monthEvents = Object.values(month.dayEvents || {}).filter(Boolean);

  if (!monthEvents.length) {
    timeline.innerHTML = renderEmptyState(
      `nothing observed in ${month.name}`,
      "this month has no timeline rollups yet.",
    );
    return;
  }

  const topEvents = monthEvents.filter((event) => event.side === "top");
  const bottomEvents = monthEvents.filter((event) => event.side === "bottom");
  const eventDays = new Map(monthEvents.map((event) => [event.day, event.side]));

  timeline.innerHTML = `
    <div class="month-view accent-${month.accent}" style="--days: ${month.days}">
      ${previous ? renderEdgeMonth(previous, index - 1, "prev") : ""}
      ${next ? renderEdgeMonth(next, index + 1, "next") : ""}

      <section class="timeline-focus-panel" aria-label="${month.name} ${month.year || ""} daily timeline">
        <svg class="month-connectors" aria-hidden="true"></svg>

        <div class="timeline-focus-heading">
          <button class="timeline-focus-node" type="button" data-month="${index}" aria-label="Return to year view">
            ${month.short}
          </button>
        </div>

        <div class="events-lane timeline-top" aria-label="${month.name} highlighted events above the daily timeline">
          ${topEvents.map((event) => renderDayEvent(event, month.days, "top")).join("")}
        </div>

        <div class="day-grid" aria-label="${month.name} ${month.year || ""} days">
          ${Array.from({ length: month.days }, (_, dayIndex) => {
            const day = dayIndex + 1;
            const side = eventDays.get(day);
            const { dayType } = getDayMeta(index, day);
            const classes = ["day-cell", dayType, side ? `has-event timeline-${side}` : ""]
              .filter(Boolean)
              .join(" ");
            const label = `${month.name} ${day}, ${month.year || ""}`;
            return `
              <button class="${classes}" type="button" data-month="${index}" data-day="${day}" title="${escapeHtml(label)}" aria-label="Open ${escapeHtml(label)}">
                ${day}
              </button>
            `;
          }).join("")}
        </div>

        <div class="events-lane timeline-bottom" aria-label="${month.name} highlighted events below the daily timeline">
          ${bottomEvents.map((event) => renderDayEvent(event, month.days, "bottom")).join("")}
        </div>
      </section>
    </div>
  `;
  layoutMonth();
}

async function renderDay(monthIndex, day) {
  const month = months[monthIndex];
  const previous = day > 1 ? day - 1 : null;
  const next = day < month.days ? day + 1 : null;
  // Lazy-fetch the day's rollup so realDayPlan/realHourPlan/segmentAvail
  // are populated before the day-view renders.
  const yyyymmdd = isoDay(monthIndex, day);
  const data = yyyymmdd ? await loadDay(yyyymmdd) : { day_top: [], hours: {}, hours_avail: {} };
  const plan = realDayPlan[`${monthIndex}:${day}`] || [];
  const segmentCount = segmentCountFromHoursAvail(data.hours_avail);
  const dateLabel = formatDateLabel(month, day);
  if (!plan.length && segmentCount === 0) {
    timeline.innerHTML = renderEmptyState(
      `nothing observed on ${dateLabel}`,
      "the day looks empty here.",
      { href: "/app/health", linkText: "system health →" },
    );
    return;
  }
  if (!plan.length && segmentCount > 0 && !(data.day_top || []).length) {
    const noun = segmentCount === 1 ? "segment" : "segments";
    timeline.innerHTML = renderEmptyState(
      `rollup pending for ${dateLabel}`,
      `${segmentCount} ${noun} are ready for a timeline rollup.`,
    );
    return;
  }
  const topEvents = plan.filter((event) => event.side === "top");
  const bottomEvents = plan.filter((event) => event.side === "bottom");
  const eventHours = new Map(plan.map((event) => [event.hour, event]));
  const dayLabel = `${month.short} ${day}`;

  timeline.innerHTML = `
    <div class="day-view accent-${month.accent}">
      ${previous ? renderEdgeDay(monthIndex, previous, "prev") : ""}
      ${next ? renderEdgeDay(monthIndex, next, "next") : ""}

      <section class="hour-panel" aria-label="${month.name} ${day}, ${month.year || ""} hourly timeline">
        <svg class="day-connectors" aria-hidden="true"></svg>

        <div class="timeline-focus-heading">
          <button class="day-focus-node" type="button" data-month="${monthIndex}" data-return-month="true" aria-label="Return to ${month.name} ${month.year || ""}">
            ${dayLabel}
          </button>
          ${renderDayProvenance(data.generated_at, data.model)}
        </div>

        <div class="hour-lane timeline-top" aria-label="${month.name} ${day} highlighted events above the hourly timeline">
          ${topEvents.map(renderHourEvent).join("")}
        </div>

        <div class="hour-grid" aria-label="${month.name} ${day}, ${month.year || ""} hours">
          ${Array.from({ length: 24 }, (_, hour) => {
            const event = eventHours.get(hour);
            const hourKind = hour >= 8 && hour <= 17 ? "work" : "personal";
            const classes = ["hour-cell", hourKind, event ? `has-hour-event timeline-${event.side}` : ""]
              .filter(Boolean)
              .join(" ");
            const label = `${formatHour(hour)} on ${month.name} ${day}, ${month.year || ""}${event ? `, ${event.title}` : ""}`;
            return `
              <button class="${classes}" type="button" data-month="${monthIndex}" data-day="${day}" data-hour="${hour}" title="${escapeHtml(label)}" aria-label="Open ${escapeHtml(label)}">
                ${formatHour(hour)}
              </button>
            `;
          }).join("")}
        </div>

        <div class="hour-lane timeline-bottom" aria-label="${month.name} ${day} highlighted events below the hourly timeline">
          ${bottomEvents.map(renderHourEvent).join("")}
        </div>
      </section>
    </div>
  `;
  layoutDay();
}

// Generic layout primitive used by every "axis with events above and
// below" view (hour view, day view, eventually month + year). For each
// side: each card's ideal left = its anchor cell's center − cardWidth/2;
// sort by anchor key; forward-pass to push apart any overlap; then draw
// SVG dotted connectors from card edge to anchor cell edge so slants
// appear when cards had to slide off their cells.
//
// opts: {
//   viewSelector,    // e.g. ".minute-view"
//   panelSelector,   // e.g. ".minute-panel"   (layout origin for SVG)
//   gridSelector,    // e.g. ".minute-grid"
  //   laneSelectors,   // e.g. [".minute-lane.timeline-top", ".minute-lane.timeline-bottom"]
//   eventSelector,   // e.g. ".minute-event"
//   cellSelector,    // e.g. ".segment-cell[data-minute='${k}']"  template
//   anchorAttr,      // e.g. "data-anchor-minute"
//   svgSelector,     // e.g. ".minute-connectors"
//   cardWidth,       // e.g. 170
//   cardGap,         // e.g. 14
// }
function layoutScale(opts) {
  const view = document.querySelector(opts.viewSelector);
  if (!view) return;
  const panel = view.querySelector(opts.panelSelector);
  const grid = view.querySelector(opts.gridSelector);
  const svg = view.querySelector(opts.svgSelector);
  if (!panel || !grid || !svg) return;

  // Mobile responsive layouts use a stacked block flow; skip the
  // absolute-positioned overlay entirely so it doesn't fight CSS.
  const isMobile = window.matchMedia("(max-width: 768px)").matches;
  if (isMobile) {
    svg.innerHTML = "";
    for (const c of view.querySelectorAll(opts.eventSelector)) c.style.left = "";
    return;
  }

  const tabletQuery = window.matchMedia("(max-width: 1023px) and (min-width: 769px)");
  const useTablet = tabletQuery.matches && opts.tablet;
  const cardWidth = useTablet ? opts.tablet.cardWidth : opts.cardWidth;
  const cardGap = useTablet ? opts.tablet.cardGap : opts.cardGap;

  const panelRect = panel.getBoundingClientRect();
  svg.setAttribute("viewBox", `0 0 ${panelRect.width} ${panelRect.height}`);
  svg.style.width = panelRect.width + "px";
  svg.style.height = panelRect.height + "px";
  svg.innerHTML = "";

  const ns = "http://www.w3.org/2000/svg";
  const accent = getComputedStyle(view).getPropertyValue("--accent").trim() || "#0f4c81";

  for (const sideName of ["top", "bottom"]) {
    const lane = view.querySelector(opts.laneSelectorFor(sideName));
    if (!lane) continue;
    const cards = Array.from(lane.querySelectorAll(opts.eventSelector));
    if (!cards.length) continue;

    const laneRect = lane.getBoundingClientRect();
    const items = cards.map((card) => {
      const anchor = parseInt(card.getAttribute(opts.anchorAttr), 10);
      const cell = grid.querySelector(opts.cellSelectorFor(anchor));
      const cellRect = cell ? cell.getBoundingClientRect() : null;
      const cellCenterInLane = cellRect
        ? cellRect.left + cellRect.width / 2 - laneRect.left
        : 0;
      return {
        card,
        cell,
        anchor,
        idealLeft: cellCenterInLane - cardWidth / 2,
      };
    }).sort((a, b) => a.anchor - b.anchor);

    // Forward pass: never let a card overlap its left neighbor.
    let prevRight = -Infinity;
    for (const it of items) {
      it.left = Math.max(it.idealLeft, prevRight + cardGap);
      prevRight = it.left + cardWidth;
    }
    for (const it of items) {
      it.card.style.left = it.left + "px";
    }

    // Connectors — drawn in panel coords so the SVG layer can overlap
    // both lanes and the central grid.
    for (const it of items) {
      if (!it.cell) continue;
      const cardRect = it.card.getBoundingClientRect();
      const cellRect = it.cell.getBoundingClientRect();

      const cardEdgeY = sideName === "top" ? cardRect.bottom : cardRect.top;
      const cellEdgeY = sideName === "top" ? cellRect.top : cellRect.bottom;

      const x1 = cardRect.left + cardRect.width / 2 - panelRect.left;
      const y1 = cardEdgeY - panelRect.top;
      const x2 = cellRect.left + cellRect.width / 2 - panelRect.left;
      const y2 = cellEdgeY - panelRect.top;

      const line = document.createElementNS(ns, "line");
      line.setAttribute("x1", x1);
      line.setAttribute("y1", y1);
      line.setAttribute("x2", x2);
      line.setAttribute("y2", y2);
      line.setAttribute("stroke", accent);
      line.setAttribute("stroke-width", "1.5");
      line.setAttribute("stroke-dasharray", "2 4");
      line.setAttribute("stroke-linecap", "round");
      line.setAttribute("opacity", "0.55");
      svg.appendChild(line);

      const dot = document.createElementNS(ns, "circle");
      dot.setAttribute("cx", x2);
      dot.setAttribute("cy", y2);
      dot.setAttribute("r", "4");
      dot.setAttribute("fill", accent);
      svg.appendChild(dot);
    }
  }
}

// Per-scale wrappers — fixed selectors and card sizing.
const LAYOUT_MINUTE = {
  viewSelector: ".minute-view",
  panelSelector: ".minute-panel",
  gridSelector: ".minute-grid",
  svgSelector: ".minute-connectors",
  eventSelector: ".minute-event",
  anchorAttr: "data-anchor-minute",
  laneSelectorFor: (s) => `.minute-lane.timeline-${s}`,
  cellSelectorFor: (k) => `.segment-cell[data-minute="${k}"]`,
  cardWidth: 170,
  cardGap: 14,
  tablet: { cardWidth: 140, cardGap: 10 },
};
const LAYOUT_DAY = {
  viewSelector: ".day-view",
  panelSelector: ".hour-panel",
  gridSelector: ".hour-grid",
  svgSelector: ".day-connectors",
  eventSelector: ".hour-event",
  anchorAttr: "data-anchor-hour",
  laneSelectorFor: (s) => `.hour-lane.timeline-${s}`,
  cellSelectorFor: (k) => `.hour-cell[data-hour="${k}"]`,
  cardWidth: 170,
  cardGap: 12,
  tablet: { cardWidth: 140, cardGap: 10 },
};
const LAYOUT_MONTH = {
  viewSelector: ".month-view",
  panelSelector: ".timeline-focus-panel",
  gridSelector: ".day-grid",
  svgSelector: ".month-connectors",
  eventSelector: ".day-event",
  anchorAttr: "data-anchor-day",
  laneSelectorFor: (s) => `.events-lane.timeline-${s}`,
  cellSelectorFor: (k) => `.day-cell[data-day="${k}"]`,
  cardWidth: 170,
  cardGap: 12,
  tablet: { cardWidth: 140, cardGap: 10 },
};

function layoutMinute() { layoutScale(LAYOUT_MINUTE); }
function layoutDay()    { layoutScale(LAYOUT_DAY); }
function layoutMonth()  { layoutScale(LAYOUT_MONTH); }

// Re-layout the active scale on resize.
window.addEventListener("resize", () => {
  if (document.querySelector(".minute-view")) layoutMinute();
  if (document.querySelector(".day-view")) layoutDay();
  if (document.querySelector(".month-view")) layoutMonth();
});

async function renderMinute(monthIndex, day, hour) {
  const month = months[monthIndex];
  const previous = hour > 0 ? hour - 1 : null;
  const next = hour < 23 ? hour + 1 : null;
  // Make sure the day's data (rollup picks + per-cell availability) is
  // loaded before we compute the plan + grid.
  const yyyymmdd = isoDay(monthIndex, day);
  if (yyyymmdd) await loadDay(yyyymmdd);
  const buckets = segmentAvail[`${monthIndex}:${day}:${hour}`] || [];
  if (!buckets.some((bucket) => bucket && bucket.best_origin)) {
    timeline.innerHTML = renderEmptyState(
      "nothing observed in this hour",
      `there are no segment observations for ${formatTime(hour, 0)}.`,
    );
    return;
  }

  const plan = realHourPlan[`${monthIndex}:${day}:${hour}`] || [];
  const topEvents = plan.filter((event) => event.side === "top");
  const bottomEvents = plan.filter((event) => event.side === "bottom");
  const eventMinutes = new Map(plan.map((event) => [event.minute, event]));
  const focusLabel = `${month.short} ${day} ${formatHour(hour)}`;

  timeline.innerHTML = `
    <div class="minute-view accent-${month.accent}">
      ${previous !== null ? renderEdgeHour(monthIndex, day, previous, "prev") : ""}
      ${next !== null ? renderEdgeHour(monthIndex, day, next, "next") : ""}

      <section class="minute-panel" aria-label="${month.name} ${day}, ${month.year || ""} ${formatHour(hour)} five-minute timeline">
        <svg class="minute-connectors" aria-hidden="true"></svg>

        <div class="timeline-focus-heading">
          <button class="minute-focus-node" type="button" data-month="${monthIndex}" data-day="${day}" data-return-day="true" aria-label="Return to ${month.name} ${day}, ${month.year || ""}">
            ${focusLabel}
          </button>
        </div>

        <div class="minute-lane timeline-top" aria-label="${formatHour(hour)} segment events above the timeline">
          ${topEvents.map(renderMinuteEvent).join("")}
        </div>

        <div class="minute-grid" aria-label="${formatHour(hour)} five-minute segments">
          ${Array.from({ length: 12 }, (_, segmentIndex) => {
            const minute = segmentIndex * 5;
            const event = eventMinutes.get(minute);
            const bucket = buckets[segmentIndex] || null;
            const hasData = !!(bucket && bucket.best_origin);
            // Availability tint: both = accent, screen-only = teal,
            // audio-only = coral, none = grey/disabled.
            let availClass = "avail-none";
            if (hasData && bucket.has_audio && bucket.has_screen) availClass = "avail-both";
            else if (hasData && bucket.has_screen) availClass = "avail-screen";
            else if (hasData && bucket.has_audio) availClass = "avail-audio";
            const classes = ["segment-cell", event ? `timeline-focus timeline-${event.side}` : "", availClass].filter(Boolean).join(" ");
            const availLabel = hasData
              ? (bucket.has_audio && bucket.has_screen ? "audio + screen"
                 : bucket.has_screen ? "screen only"
                 : bucket.has_audio ? "audio only" : "metadata only")
              : "no observation";
            const label = `${formatTime(hour, minute)} · ${availLabel}${event ? `, ${event.title}` : ""}`;
            const disabled = hasData ? "" : "disabled aria-disabled=\"true\"";
            return `
              <button class="${classes}" type="button" ${disabled} data-month="${monthIndex}" data-day="${day}" data-hour="${hour}" data-minute="${minute}" title="${escapeHtml(label)}" aria-label="${escapeHtml(label)}">
                ${String(minute).padStart(2, "0")}
              </button>
            `;
          }).join("")}
        </div>

        <div class="minute-lane timeline-bottom" aria-label="${formatHour(hour)} segment events below the timeline">
          ${bottomEvents.map(renderMinuteEvent).join("")}
        </div>
      </section>
    </div>
  `;
  layoutMinute();
}

// Empty-state river when a 5-min cell has no underlying segment data.
// The hour view should disable empty cells, so this is a defensive render.
async function renderEmptySegment(monthIndex, day, hour, minute, focusLabel) {
  const month = months[monthIndex];
  const previous = minute > 0 ? minute - 5 : null;
  const next = minute < 55 ? minute + 5 : null;
  timeline.innerHTML = `
    <div class="segment-view accent-${month.accent}">
      ${previous !== null ? renderEdgeSegment(monthIndex, day, hour, previous, "prev") : ""}
      ${next !== null ? renderEdgeSegment(monthIndex, day, hour, next, "next") : ""}
      <section class="segment-panel">
        <div class="timeline-focus-heading">
          <button class="segment-focus-node" type="button"
                  data-month="${monthIndex}" data-day="${day}" data-hour="${hour}"
                  data-return-hour="true">${focusLabel}</button>
        </div>
        <div class="segment-empty">no observation in this slice</div>
      </section>
    </div>
  `;
}

async function renderFiveMinute(monthIndex, day, hour, minute) {
  // The 5-min view breaks from the event-cards-around-an-axis pattern
  // of higher levels. Here we visualize what sol *actually observed* in
  // that 5-minute window — screen frames as ticks above the time axis,
  // transcript lines as dots below. No cards, no slants. Data loads
  // dynamically per the cell's best_origin from the day endpoint.
  const month = months[monthIndex];
  const previous = minute > 0 ? minute - 5 : null;
  const next = minute < 55 ? minute + 5 : null;
  const focusLabel = `${formatTime(hour, minute)}`;

  // Look up which segment to load from the cached day data.
  const yyyymmdd = isoDay(monthIndex, day);
  if (yyyymmdd) await loadDay(yyyymmdd);
  const buckets = segmentAvail[`${monthIndex}:${day}:${hour}`] || [];
  const bucketIdx = Math.floor(minute / 5);
  const bucket = buckets[bucketIdx] || null;
  const origin = bucket && bucket.best_origin ? bucket.best_origin : null;

  // No data → render an empty-state river. (Cell shouldn't have been
  // clickable in the first place; this is a defensive fallback.)
  if (!origin) {
    return renderEmptySegment(monthIndex, day, hour, minute, focusLabel);
  }

  // Derive the segment's wall-clock start from its segment name's HHMMSS.
  // origin format: "YYYYMMDD/<stream>/<HHMMSS_LEN>" or "YYYYMMDD/<HHMMSS_LEN>"
  const parts = origin.split("/");
  const segName = parts[parts.length - 1];
  const segMatch = /^(\d{2})(\d{2})(\d{2})_(\d{1,6})$/.exec(segName);
  const startSec = segMatch ? parseInt(segMatch[1],10)*3600 + parseInt(segMatch[2],10)*60 + parseInt(segMatch[3],10) : (hour*3600 + minute*60);
  const dur = segMatch ? parseInt(segMatch[4], 10) : 300;
  const stream = parts.length === 3 ? parts[1] : "";
  const dayStr = `${parts[0].slice(0,4)}-${parts[0].slice(4,6)}-${parts[0].slice(6,8)}`;
  const meta = { day: dayStr, startSec, durationSec: dur, stream };

  const sample = await loadSegment(origin);
  if (!sample) {
    return renderEmptySegment(monthIndex, day, hour, minute, focusLabel);
  }

  // Stash for the click-driven detail handlers.
  _activeSegment = sample;
  _activeMeta = meta;

  const audioHeader = sample.audio?.header || {};
  const screenHeader = sample.screen?.header || {};
  const audioLines = sample.audio?.lines || [];
  const screenFrames = sample.screen?.frames || [];

  const setting = audioHeader.setting || screenHeader.setting || "—";
  const rawTopics = audioHeader.topics ?? "";
  const topics = Array.isArray(rawTopics)
    ? rawTopics.map((s) => String(s).trim()).filter(Boolean)
    : String(rawTopics).split(",").map((s) => s.trim()).filter(Boolean);
  const fmtPct = (sec) => `${(sec / dur * 100).toFixed(2)}%`;

  // Pre-render screen ticks (one per frame) and audio dots/lines.
  const featuredCount = screenFrames.filter(isFeatured).length;
  const screenMarks = screenFrames.map((f) => {
    const a = f.analysis || {};
    const featured = isFeatured(f);
    const left = fmtPct(f.timestamp || 0);
    const tipText = `${segmentTimeLabel(meta, f.timestamp || 0)} · ${a.primary || "?"}\n${(a.visual_description || "").slice(0, 200)}`;
    // No always-visible labels — too crowded with 19 featured frames.
    // Tick height = featured signal; full content surfaces via title
    // hover and the click-to-detail panel.
    return `<button class="river-tick screen ${featured ? "is-featured" : ""}"
      data-frame-id="${f.frame_id}"
      style="left:${left}; --cat:${categoryColor(a.primary)};"
      title="${escapeHtml(tipText)}"
      type="button">
      <span class="river-tick-bar"></span>
      ${featured ? `<span class="river-tick-pip"></span>` : ""}
    </button>`;
  }).join("");

  const audioMarks = audioLines.length
    ? audioLines.map((line, i) => {
        // Convert "HH:MM:SS" → seconds offset from segment start.
        const [hh, mm, ss] = (line.start || "00:00:00").split(":").map(Number);
        const lineSec = hh * 3600 + mm * 60 + ss;
        const offset = Math.max(0, Math.min(dur, lineSec - meta.startSec));
        const sp = line.speaker || 1;
        const speakerColor = ["var(--blue)","var(--teal)","var(--coral)","var(--amber)"][sp - 1] || "var(--muted)";
        const tipText = `${line.start} · speaker ${sp}\n${(line.text || "").slice(0, 200)}`;
        return `<button class="river-audio-dot"
          data-audio-index="${i}"
          style="left:${fmtPct(offset)}; --cat:${speakerColor};"
          title="${escapeHtml(tipText)}"
          aria-label="${escapeHtml(tipText)}"
          type="button"></button>`;
      }).join("")
    : `<div class="river-empty">no microphone input in this slice</div>`;

  // Minute markers along the axis: 0, 60, 120, 180, 240, (the right edge is the segment end)
  const axisMarks = [0, 60, 120, 180, 240].map((s) =>
    `<div class="axis-mark" style="left:${fmtPct(s)};"><span>${segmentTimeLabel(meta, s).slice(0, 5)}</span></div>`
  ).join("");
  const startHHMM = segmentTimeLabel(meta, 0).slice(0, 5);
  const endHHMM = segmentTimeLabel(meta, dur).slice(0, 5);
  const minutesStr = `${Math.floor(dur / 60)} min${dur % 60 ? ` ${dur % 60}s` : ""}`;

  timeline.innerHTML = `
    <div class="segment-view accent-${month.accent}">
      ${previous !== null ? renderEdgeSegment(monthIndex, day, hour, previous, "prev") : ""}
      ${next !== null ? renderEdgeSegment(monthIndex, day, hour, next, "next") : ""}

      <section class="segment-panel" aria-label="${month.name} ${day}, ${month.year || ""} ${focusLabel} segment observations">
        <div class="timeline-focus-heading">
          <button class="segment-focus-node" type="button"
                  data-month="${monthIndex}" data-day="${day}" data-hour="${hour}"
                  data-return-hour="true"
                  aria-label="Return to ${formatHour(hour)} on ${month.name} ${day}, ${month.year || ""}">
            ${focusLabel}
          </button>
        </div>

        <header class="segment-header">
          <div class="seg-header-row">
            <span class="seg-header-time">${meta.day} · ${startHHMM} → ${endHHMM} · ${minutesStr}</span>
            <span class="seg-header-mid">${escapeHtml(meta.stream || "—")} observer</span>
            <span class="seg-header-end">${escapeHtml(setting)} setting</span>
          </div>
          ${topics.length ? `<div class="seg-topics">${topics.map((t) => `<span class="topic-chip">${escapeHtml(t)}</span>`).join("")}</div>` : ""}
        </header>

        <div class="segment-river">
          <div class="river-screen" aria-label="screen frames sol observed">
            ${screenMarks}
          </div>
          <div class="river-axis">
            ${axisMarks}
          </div>
          <div class="river-audio" aria-label="microphone input">
            ${audioMarks}
          </div>
        </div>

        <div class="segment-detail" id="segment-detail">
          <div class="seg-detail-empty">click a tick or audio dot on the river to see what sol observed at that moment</div>
        </div>

        <footer class="segment-footer">
          ${screenFrames.length} frames analyzed
          · ${audioLines.length} transcript line${audioLines.length === 1 ? "" : "s"}
          · ${featuredCount} frames with extracted text
        </footer>
      </section>
    </div>
  `;

  // Wire click handlers for tick + audio-dot selection.
  for (const tick of document.querySelectorAll(".river-tick[data-frame-id]")) {
    tick.addEventListener("click", (e) => {
      e.stopPropagation();
      const fid = parseInt(tick.getAttribute("data-frame-id"), 10);
      if (tick.classList.contains("is-active")) clearSegmentDetail();
      else showSegmentDetail(fid);
    });
  }
  for (const dot of document.querySelectorAll(".river-audio-dot[data-audio-index]")) {
    dot.addEventListener("click", (e) => {
      e.stopPropagation();
      const idx = parseInt(dot.getAttribute("data-audio-index"), 10);
      if (dot.classList.contains("is-active")) clearSegmentDetail();
      else showSegmentAudioDetail(idx);
    });
  }
}

function renderEdgeDay(monthIndex, day, position) {
  const month = months[monthIndex];
  return `
    <button class="edge-day timeline-${position}" type="button" data-month="${monthIndex}" data-day="${day}" aria-label="Open ${month.name} ${day}, ${month.year || ""}">
      ${day}
    </button>
  `;
}

function renderEdgeHour(monthIndex, day, hour, position) {
  const month = months[monthIndex];
  return `
    <button class="edge-hour timeline-${position}" type="button" data-month="${monthIndex}" data-day="${day}" data-hour="${hour}" aria-label="Open ${formatHour(hour)} on ${month.name} ${day}, ${month.year || ""}">
      ${formatHour(hour)}
    </button>
  `;
}

function renderEdgeSegment(monthIndex, day, hour, minute, position) {
  return `
    <button class="edge-segment timeline-${position}" type="button" data-month="${monthIndex}" data-day="${day}" data-hour="${hour}" data-minute="${minute}" aria-label="Open ${formatTime(hour, minute)}">
      ${formatTime(hour, minute)}
    </button>
  `;
}

function renderEdgeMonth(month, index, position) {
  return `
    <button class="edge-node timeline-${position} accent-${month.accent}" type="button" data-month="${index}" aria-label="Open ${month.name} ${month.year || ""}">
      ${month.short}
    </button>
  `;
}

function renderDayEvent(event, days, side) {
  return `
    <article class="day-event timeline-${side}" data-anchor-day="${event.day}" data-side="${side}">
      <div class="day-date">Day ${event.day}</div>
      <h3>${escapeHtml(event.title)}</h3>
      <p>${escapeHtml(event.text)}</p>
      ${renderOriginChip(event.origin)}
    </article>
  `;
}

function renderHourEvent(event) {
  return `
    <article class="hour-event timeline-${event.side}" data-anchor-hour="${event.hour}" data-side="${event.side}">
      <div class="hour-time">${formatHour(event.hour)}</div>
      <h3>${escapeHtml(event.title)}</h3>
      <p>${escapeHtml(event.text)}</p>
      ${renderOriginChip(event.origin)}
    </article>
  `;
}

function renderMinuteEvent(event) {
  return `
    <article class="minute-event timeline-${event.side}" data-anchor-minute="${event.minute}" data-side="${event.side}">
      <div class="minute-time">${String(event.minute).padStart(2, "0")}</div>
      <h3>${escapeHtml(event.title)}</h3>
      <p>${escapeHtml(event.text)}</p>
      ${renderOriginChip(event.origin)}
    </article>
  `;
}

timeline.addEventListener("click", async (event) => {
  const returnHourButton = event.target.closest("[data-return-hour]");
  if (returnHourButton) {
    const monthIndex = Number(returnHourButton.dataset.month);
    const day = Number(returnHourButton.dataset.day);
    const hour = Number(returnHourButton.dataset.hour);
    if (Number.isInteger(monthIndex) && Number.isInteger(day) && Number.isInteger(hour)) {
      currentView = "day";
      selectedMonth = monthIndex;
      selectedDay = day;
      selectedHour = hour;
      selectedMinute = null;
      history.pushState({}, "", "#h=" + hour);
      await renderMinute(monthIndex, day, hour);
    }
    return;
  }

  const returnDayButton = event.target.closest("[data-return-day]");
  if (returnDayButton) {
    const monthIndex = Number(returnDayButton.dataset.month);
    const day = Number(returnDayButton.dataset.day);
    if (Number.isInteger(monthIndex) && Number.isInteger(day)) {
      currentView = "day";
      selectedMonth = monthIndex;
      selectedDay = day;
      selectedHour = null;
      selectedMinute = null;
      history.pushState({}, "", window.location.pathname);
      await renderDay(monthIndex, day);
    }
    return;
  }

  const returnMonthButton = event.target.closest("[data-return-month]");
  if (returnMonthButton) {
    const monthIndex = Number(returnMonthButton.dataset.month);
    const ym = months[monthIndex]?.ym;
    if (Number.isInteger(monthIndex) && ym) {
      currentView = "month";
      selectedMonth = monthIndex;
      selectedDay = null;
      selectedHour = null;
      selectedMinute = null;
      history.pushState({}, "", "/app/timeline/" + ym);
      await loadMonth(ym);
      await renderMonth(monthIndex);
    }
    return;
  }

  const minuteButton = event.target.closest("[data-minute]");
  if (minuteButton) {
    const monthIndex = Number(minuteButton.dataset.month);
    const day = Number(minuteButton.dataset.day);
    const hour = Number(minuteButton.dataset.hour);
    const targetMinute = Number(minuteButton.dataset.minute);
    if (
      Number.isInteger(monthIndex) &&
      Number.isInteger(day) &&
      Number.isInteger(hour) &&
      Number.isInteger(targetMinute)
    ) {
      currentView = "day";
      selectedMonth = monthIndex;
      selectedDay = day;
      selectedHour = hour;
      selectedMinute = targetMinute;
      const hh = String(selectedHour).padStart(2, "0");
      const mm = String(targetMinute).padStart(2, "0");
      history.pushState({}, "", "#m=" + hh + mm);
      await renderFiveMinute(monthIndex, day, hour, targetMinute);
    }
    return;
  }

  const hourButton = event.target.closest("[data-hour]");
  if (hourButton) {
    const monthIndex = Number(hourButton.dataset.month);
    const day = Number(hourButton.dataset.day);
    const hour = Number(hourButton.dataset.hour);
    if (Number.isInteger(monthIndex) && Number.isInteger(day) && Number.isInteger(hour)) {
      currentView = "day";
      selectedMonth = monthIndex;
      selectedDay = day;
      selectedHour = hour;
      selectedMinute = null;
      history.pushState({}, "", "#h=" + hour);
      await renderMinute(monthIndex, day, hour);
    }
    return;
  }

  const dayButton = event.target.closest("[data-day]");
  if (dayButton) {
    const monthIndex = Number(dayButton.dataset.month);
    const day = Number(dayButton.dataset.day);
    const dayString = isoDay(monthIndex, day);
    if (Number.isInteger(monthIndex) && Number.isInteger(day) && dayString) {
      currentView = "day";
      selectedMonth = monthIndex;
      selectedDay = day;
      selectedHour = null;
      selectedMinute = null;
      history.pushState({}, "", "/app/timeline/" + dayString);
      await renderDay(monthIndex, day);
    }
    return;
  }

  const button = event.target.closest("[data-month]");
  if (!button) return;

  const index = Number(button.dataset.month);
  if (!Number.isInteger(index)) return;

  if (selectedMonth === index && button.classList.contains("timeline-focus-node")) {
    currentView = "year";
    selectedMonth = null;
    selectedDay = null;
    selectedHour = null;
    selectedMinute = null;
    history.pushState({}, "", "/app/timeline/year");
    renderYear();
    return;
  }

  const ym = months[index]?.ym;
  if (!ym) return;
  currentView = "month";
  selectedMonth = index;
  selectedDay = null;
  selectedHour = null;
  selectedMinute = null;
  history.pushState({}, "", "/app/timeline/" + ym);
  await loadMonth(ym);
  await renderMonth(index);
});

window.addEventListener("popstate", (e) => {
  // Pathname is authoritative for view; hash is authoritative for sub-day depth.
  // For 3b, popstate only fires within the same document, so pathname is stable
  // across all events EXCEPT browser back/forward across the boot pathname.
  // Day URLs are the only pathname that hosts sub-day fragments, so we re-derive
  // hash state and re-render relative to the current day/month/year context.
  applyHash(window.location.hash);
});

async function bootTimeline() {
  if (window.AppServices?.badges?.app?.clear) {
    window.AppServices.badges.app.clear("timeline");
  }
  const result = await loadIndex();
  if (result.state === "error") {
    timeline.innerHTML = renderErrorState();
    return;
  }
  await applyHash(window.location.hash);
}

window.timelineRefresh = {
  day(yyyymmdd) {
    if (!yyyymmdd) return undefined;
    dayCache.delete(yyyymmdd);
    delete monthCache[yyyymmdd.slice(0, 6)];
    clearDayLookups(yyyymmdd);
    return loadDay(yyyymmdd).then(() => applyHash(window.location.hash));
  },
  index() {
    clearRollupCaches();
    return loadIndex().then((result) => {
      if (result.state === "error") {
        timeline.innerHTML = renderErrorState();
        return undefined;
      }
      return applyHash(window.location.hash);
    });
  },
  getCurrentDay() {
    if (selectedMonth === null || selectedDay === null) return null;
    return isoDay(selectedMonth, selectedDay);
  },
  getCurrentView() {
    if (selectedMinute !== null) return "five-minute";
    if (selectedHour !== null) return "hour";
    return currentView || "year";
  },
};

bootTimeline();
