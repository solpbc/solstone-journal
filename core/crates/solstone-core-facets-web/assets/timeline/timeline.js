// Timeline data comes from the real API; empty and failure states render visibly.
let months = [];
let realHourPlan = {};
let realDayPlan = {};
const segmentAvail = {};        // "monthIdx:day:hour" → buckets (12 entries)
const dayCache = new Map();      // "YYYYMMDD" → /app/timeline/api/day response
const segCache = new Map();      // "<day>/<stream>/<seg>" → /app/timeline/api/segment response
const monthCache = {};
let gridCache = null;
let gridInflight = null;
let gridGeneration = 0;
let timelineUnit = null;
let overviewArtifact = null;

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

function segmentEpochBaseMs(meta) {
  return new Date(`${meta.day}T00:00:00`).getTime() + meta.startSec * 1000;
}

function browserTimeLabel(epochMs) {
  const dt = new Date(Number(epochMs) || 0);
  return String(dt.getHours()).padStart(2, "0") + ":" +
         String(dt.getMinutes()).padStart(2, "0") + ":" +
         String(dt.getSeconds()).padStart(2, "0");
}

function browserOffsetSeconds(meta, epochMs) {
  return (Number(epochMs) - segmentEpochBaseMs(meta)) / 1000;
}

// ── Data loaders (lazy, cached) ──────────────────────────────────────

async function loadIndex() {
  try {
    const res = await fetch("/app/timeline/api/overview", { cache: "no-store" });
    if (!res.ok) {
      console.info(`/app/timeline/api/overview failed (${res.status}); showing timeline error`);
      return { state: "error" };
    }
    let idx;
    try {
      idx = await res.json();
    } catch (e) {
      console.warn("/app/timeline/api/overview returned unreadable JSON; showing timeline error", e);
      return { state: "error" };
    }
    overviewArtifact = idx;
    rebuildMonthsFromIndex(idx);
    const state = months.every((m) => !(m.day_count > 0)) ? "empty" : "data";
    console.info(`loaded /app/timeline/api/overview (${idx.months.length} months)`);
    return { state };
  } catch (e) {
    console.warn("/app/timeline/api/overview fetch failed; showing timeline error", e);
    return { state: "error" };
  }
}

async function loadGrid(force = false) {
  if (!force && gridCache) return gridCache;
  if (!force && gridInflight) return gridInflight;

  const generation = gridGeneration;
  gridInflight = (async () => {
    try {
      const res = await fetch("/app/timeline/api/grid", { cache: "no-store" });
      if (!res.ok) {
        console.info(`/app/timeline/api/grid failed (${res.status}); showing timeline error`);
        return null;
      }
      const payload = await res.json();
      if (generation === gridGeneration) {
        gridCache = payload && typeof payload === "object" ? payload : null;
      }
      return gridCache;
    } catch (e) {
      console.warn("/app/timeline/api/grid fetch failed; showing timeline error", e);
      return null;
    } finally {
      gridInflight = null;
    }
  })();
  return gridInflight;
}

async function loadTimelineUnit() {
  if (timelineUnit) return timelineUnit;
  if (!window.whenShellReady) {
    throw new Error("timeline date_nav unit requires shell readiness");
  }
  return window.whenShellReady((shell) => {
    const app = (shell?.apps || []).find((item) => item.name === "timeline");
    if (!app?.date_nav?.unit) {
      throw new Error("timeline date_nav unit missing");
    }
    timelineUnit = app.date_nav.unit;
    return timelineUnit;
  });
}

function rebuildMonthsFromIndex(idx) {
  const newMonths = idx.months.map((m, i) => {
    const fullName = MONTH_FULL_NAMES[m.month_num - 1];
    return {
      name: fullName,
      short: fullName.slice(0, 3).toUpperCase(),
      year: m.year,
      month_num: m.month_num,
      ym: m.ym,
      accent: ACCENT_ROTATION[i % 4],
      days: m.days_in_month,
      first_weekday: m.first_weekday,
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

function clearGridCache() {
  gridGeneration += 1;
  gridCache = null;
  gridInflight = null;
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
  browsing:    "var(--teal)",
  social:      "var(--blue)",
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
  for (const el of document.querySelectorAll(".river-tick.is-active, .river-audio-dot.is-active, .river-browser-mark.is-active")) {
    el.classList.remove("is-active");
  }
}

// The river renderer stashes the rendered segment's data here so the
// click-driven detail handlers can find frames + transcript lines
// without re-fetching.
let _activeSegment = null;
let _activeMeta = null;
let _activeBrowserFiles = [];

function renderTimelineMarkdown(value) {
  const markdown = String(value || "");
  if (window.AppServices?.renderMarkdown) {
    return window.AppServices.renderMarkdown(markdown);
  }
  return `<pre class="seg-browser-pre">${escapeHtml(markdown)}</pre>`;
}

function hasBrowserContent(browserFiles) {
  return (browserFiles || []).some((site) => site && (site.error || (site.entries || []).length));
}

function browserChangeCount(browserFiles) {
  return (browserFiles || []).reduce((total, site) => {
    return total + (site.entries || []).filter((entry) => entry.kind === "change").length;
  }, 0);
}

function renderBrowserSections(browserFiles, focusSiteIndex = null, focusEntryIndex = null) {
  const sections = (browserFiles || []).map((site, siteIndex) => {
    if (!site || (!site.error && !(site.entries || []).length)) return "";
    const siteName = site.site_name || site.site || site.file || "pages";
    const title = site.title ? `<div class="seg-browser-title">${escapeHtml(site.title)}</div>` : "";
    const error = site.error ? `<div class="seg-browser-error">${escapeHtml(site.error)}</div>` : "";
    const rows = (site.entries || []).map((entry, entryIndex) => {
      const isFocus = siteIndex === focusSiteIndex && entryIndex === focusEntryIndex;
      const kind = entry.kind === "snapshot" ? "snapshot" : "change";
      return `
        <article class="seg-browser-entry ${isFocus ? "is-focus" : ""}">
          <div class="seg-browser-entry-meta">
            <span class="seg-detail-time">${browserTimeLabel(entry.ts)}</span>
            <span class="seg-detail-cat" style="--cat:var(--teal)">${kind}</span>
          </div>
          <div class="seg-browser-markdown">${renderTimelineMarkdown(entry.markdown)}</div>
        </article>
      `;
    }).join("");
    return `
      <section class="seg-browser-section">
        <header class="seg-browser-header">
          <h3>${escapeHtml(siteName)}</h3>
          ${title}
        </header>
        ${error}
        ${rows}
      </section>
    `;
  }).join("");
  return sections || `<div class="seg-detail-empty">no page content in this slice</div>`;
}

function showBrowserDetail(siteIndex, entryIndex) {
  const detail = document.getElementById("segment-detail");
  if (!detail || !_activeBrowserFiles.length) return;
  detail.innerHTML = renderBrowserSections(_activeBrowserFiles, siteIndex, entryIndex);
  clearActiveMarks();
  const active = document.querySelector(`.river-browser-mark[data-browser-site="${siteIndex}"][data-browser-entry="${entryIndex}"]`);
  if (active) active.classList.add("is-active");
}

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
  if (detail) {
    detail.innerHTML = hasBrowserContent(_activeBrowserFiles)
      ? renderBrowserSections(_activeBrowserFiles)
      : `<div class="seg-detail-empty">click a tick, audio dot, or page mark to inspect that moment</div>`;
  }
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

function renderEmptyState(headline, body, opts = {}) {
  const classes = ["timeline-empty-state", opts.modifierClass || ""].filter(Boolean).join(" ");
  const icon = opts.icon
    ? `\n      <div class="timeline-empty-icon" aria-hidden="true">${opts.icon}</div>`
    : "";
  const link = opts.href && opts.linkText
    ? `<a href="${escapeHtml(opts.href)}">${escapeHtml(opts.linkText)}</a>`
    : "";
  const detail = opts.detailHtml || "";
  return `
    <div class="${classes}" data-timeline-state="empty">${icon}
      <h2>${escapeHtml(headline)}</h2>
      <p>${escapeHtml(body)}</p>
      ${detail}
      ${link}
    </div>
  `;
}

// Loading is its own state, never a blank panel: the day fetch can run for
// tens of seconds and silence reads as "this day has nothing".
function renderLoadingState(label) {
  return `
    <div class="timeline-loading-state" data-timeline-state="loading" role="status">
      <span class="timeline-loading-spinner" aria-hidden="true"></span>
      <p>${escapeHtml(label)}</p>
    </div>
  `;
}

function renderErrorState() {
  return `
    <div class="timeline-empty-state" data-timeline-state="error" role="alert">
      <h2>couldn't reach the timeline service</h2>
      <p>reload to try again, or check whether the solstone app is running</p>
      <a href="/app/health">system health →</a>
    </div>
  `;
}

function renderArtifactTruth(data) {
  return window.TimelineProvenance.renderArtifactTruth(
    data?.status,
    data?.generated_at_ms,
    data?.provenance,
    data?.artifact_outcome,
  );
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
    await renderAllHistory();
    return;
  }

  if (currentView === "month") {
    const month = months[selectedMonth];
    if (!month) {
      timeline.innerHTML = renderEmptyState(
        "month not in timeline",
        "this month has no timeline data.",
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
        "this day has no timeline data.",
      );
      return;
    }
    // Only on a cold day — a live refresh already has the day cached, and
    // flashing a skeleton over it would throw away the reader's place.
    if (!dayCache.has(day)) {
      timeline.innerHTML = renderLoadingState(`loading ${window.JournalFormat.day(day).replace(/^(Today|Yesterday|Tomorrow)$/, (word) => word.toLowerCase())} …`);
    }
    await loadMonth(day.slice(0, 6));
    await loadDay(day);
    await renderDay(selectedMonth, selectedDay);
    return;
  }

  await renderAllHistory();
}

async function prefetchSegmentForMinute(hour, minute) {
  const buckets = segmentAvail[`${selectedMonth}:${selectedDay}:${hour}`] || [];
  const bucket = buckets[Math.floor(minute / 5)] || null;
  const origins = [];
  if (bucket?.best_origin) origins.push(bucket.best_origin);
  if (bucket?.browser_origin && bucket.browser_origin !== bucket.best_origin) origins.push(bucket.browser_origin);
  await Promise.all(origins.map((origin) => loadSegment(origin)));
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

function formatHistoryStart(day) {
  if (!/^\d{8}$/.test(day || "")) return "";
  return `${day.slice(0, 4)}-${day.slice(4, 6)}-${day.slice(6, 8)}`;
}

function allHistorySummary(data, unit) {
  const nav = window.DateNav;
  const seenDays = new Set();
  let total = 0;
  for (const [day, value] of Object.entries(data.days || {})) {
    const count = nav.coerceCount(value);
    if (count > 0) {
      total += count;
      seenDays.add(day);
    }
  }
  for (const [day, value] of Object.entries(data.pending || {})) {
    const count = nav.coerceCount(value);
    if (count > 0) {
      total += count;
      seenDays.add(day);
    }
  }
  const activeDays = seenDays.size;
  return {
    total: nav.countLabel(total, unit),
    activeDays,
    activeDayLabel: activeDays === 1 ? "active day" : "active days",
    coverageStart: formatHistoryStart(data.coverage?.start || ""),
  };
}

async function renderAllHistory() {
  const [gridData, unit] = await Promise.all([loadGrid(), loadTimelineUnit()]);
  if (!gridData) {
    timeline.innerHTML = renderErrorState();
    return;
  }
  const summary = allHistorySummary(gridData, unit);
  if (summary.activeDays === 0) {
    timeline.innerHTML = renderEmptyState(
      "no timeline data yet",
      "once what you share from a day is in your journal, that day shows up here",
      {
        href: "/app/health",
        linkText: "system health →",
        detailHtml: renderArtifactTruth(overviewArtifact),
      },
    );
    return;
  }

  timeline.innerHTML = `
    <div class="timeline-history-view">
      <section class="timeline-history-lede" aria-label="timeline history summary">
        <h2>all history</h2>
        <p>${escapeHtml(summary.total)} across ${summary.activeDays} ${summary.activeDayLabel} since ${escapeHtml(summary.coverageStart)}</p>
        ${renderArtifactTruth(overviewArtifact)}
      </section>
      <div class="timeline-history-grid" data-timeline-daygrid></div>
      <div class="timeline-history-legend" data-timeline-daygrid-legend></div>
    </div>
  `;
  const host = timeline.querySelector("[data-timeline-daygrid]");
  const legendHost = timeline.querySelector("[data-timeline-daygrid-legend]");
  const mounted = window.DayGrid.mount(host, {
    data: gridData,
    unit,
    mode: "navigate",
    appPath: "/app/timeline",
    monthLinks: true,
  });
  if (!mounted) {
    timeline.innerHTML = renderErrorState();
    return;
  }
  window.DayGrid.legend(legendHost, { unit, data: gridData });
}

async function renderMonth(index) {
  const month = months[index];
  const previous = index > 0 ? months[index - 1] : null;
  const next = index < months.length - 1 ? months[index + 1] : null;
  const monthEvents = Object.values(month.dayEvents || {}).filter(Boolean);

  if (!monthEvents.length) {
    if (!monthCache[month.ym]) {
      timeline.innerHTML = renderErrorState();
      return;
    }
    timeline.innerHTML = renderEmptyState(
      `timeline summaries aren't available for ${month.name}`,
      "choose a day to browse your journal.",
      { detailHtml: renderArtifactTruth(monthCache[month.ym]) },
    ) + `<nav class="timeline-month-days" aria-label="${escapeHtml(month.name)} days">
      ${Array.from({ length: month.days }, (_, i) => {
        const day = i + 1;
        return `<a href="/app/timeline/${isoDay(index, day)}" aria-label="${escapeHtml(month.name)} ${day}">${day}</a>`;
      }).join("")}
    </nav>`;
    return;
  }

  const topEvents = monthEvents.filter((event) => event.side === "top");
  const bottomEvents = monthEvents.filter((event) => event.side === "bottom");
  const eventDays = new Map(monthEvents.map((event) => [event.day, event.side]));

  timeline.innerHTML = `
    <div class="timeline-rollup-status">
      ${renderArtifactTruth(monthCache[month.ym])}
    </div>
    <div class="month-view accent-${month.accent}" style="--days: ${month.days}">
      ${previous ? renderEdgeMonth(previous, index - 1, "prev") : ""}
      ${next ? renderEdgeMonth(next, index + 1, "next") : ""}

      <section class="timeline-focus-panel" aria-label="${month.name} ${month.year || ""} daily timeline">
        <svg class="month-connectors" aria-hidden="true"></svg>

        <div class="timeline-focus-heading">
          <button class="timeline-focus-node" type="button" data-month="${index}" aria-label="return to all history">
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
              <button class="${classes}" type="button" data-month="${index}" data-day="${day}" title="${escapeHtml(label)}" aria-label="open ${escapeHtml(label)}">
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
      `nothing in your journal for ${dateLabel}`,
      "the day looks empty here.",
      {
        icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><polyline points="12 6 12 12 16 14"></polyline></svg>',
        href: "/app/health",
        linkText: "system health →",
        detailHtml: renderArtifactTruth(data),
      },
    );
    return;
  }
  if (!plan.length && segmentCount > 0 && !(data.day_top || []).length) {
    const noun = segmentCount === 1 ? "segment" : "segments";
    const verb = segmentCount === 1 ? "is" : "are";
    timeline.innerHTML = renderEmptyState(
      `rollup pending for ${dateLabel}`,
      `${segmentCount} ${noun} ${verb} ready for a timeline rollup.`,
      { detailHtml: renderArtifactTruth(data) },
    );
    return;
  }
  const topEvents = plan.filter((event) => event.side === "top");
  const bottomEvents = plan.filter((event) => event.side === "bottom");
  const eventHours = new Map(plan.map((event) => [event.hour, event]));
  const dayLabel = `${month.short} ${day}`;

  timeline.innerHTML = `
    <div class="timeline-rollup-status">
      ${renderArtifactTruth(data)}
    </div>
    <div class="day-view accent-${month.accent}">
      ${previous ? renderEdgeDay(monthIndex, previous, "prev") : ""}
      ${next ? renderEdgeDay(monthIndex, next, "next") : ""}

      <section class="hour-panel" aria-label="${month.name} ${day}, ${month.year || ""} hourly timeline">
        <svg class="day-connectors" aria-hidden="true"></svg>

        <div class="timeline-focus-heading">
          <button class="day-focus-node" type="button" data-month="${monthIndex}" data-return-month="true" aria-label="return to ${month.name} ${month.year || ""}">
            ${dayLabel}
          </button>
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
              <button class="${classes}" type="button" data-month="${monthIndex}" data-day="${day}" data-hour="${hour}" title="${escapeHtml(label)}" aria-label="open ${escapeHtml(label)}">
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

// Generic layout primitive used by each axis view with events above and below
// (month, day, and hour). For each
// side: each card's ideal left = its anchor cell's center − cardWidth/2;
// sort by anchor key; place left-to-right, wrapping into another row once the
// lane is full so no card is ever pushed past the panel edge; then draw
// SVG dotted connectors from card edge to anchor cell edge so slants
// appear when cards had to slide off their cells.
//
// Every card stays inside its lane, the lanes grow to hold their rows, and the
// hour axis plus the panel grow with them. Nothing depends on the view's
// overflow to hide a card that did not fit.
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
//   minCardWidth,    // e.g. 132  (narrowest a card may shrink to before wrapping)
//   cardGap,         // e.g. 14
//   rowGap,          // e.g. 12   (vertical gap between wrapped rows)
// }

// Drop every inline value a previous pass wrote so each pass measures the
// stylesheet's own geometry rather than its own last answer.
function resetScaleLayout(view, opts) {
  view.style.removeProperty("--axis-top");
  view.style.removeProperty("min-height");
  const panel = view.querySelector(opts.panelSelector);
  if (panel) panel.style.removeProperty("min-height");
  for (const sideName of ["top", "bottom"]) {
    const lane = view.querySelector(opts.laneSelectorFor(sideName));
    if (!lane) continue;
    lane.style.removeProperty("top");
    lane.style.removeProperty("height");
  }
  for (const card of view.querySelectorAll(opts.eventSelector)) {
    card.style.removeProperty("left");
    card.style.removeProperty("width");
    card.style.removeProperty("top");
    card.style.removeProperty("bottom");
  }
}

// Widest card that still lets every card in this lane share one row, clamped
// to [minCardWidth, cardWidth]. Lanes that already fit keep the full width.
function laneCardWidth(count, laneWidth, cardWidth, minCardWidth, cardGap) {
  const capped = Math.min(cardWidth, laneWidth);
  if (count < 2) return capped;
  const fitsOneRow = (laneWidth - (count - 1) * cardGap) / count;
  return Math.max(Math.min(capped, fitsOneRow), Math.min(minCardWidth, laneWidth));
}

function layoutScale(opts) {
  const view = document.querySelector(opts.viewSelector);
  if (!view) return;
  const panel = view.querySelector(opts.panelSelector);
  const grid = view.querySelector(opts.gridSelector);
  const svg = view.querySelector(opts.svgSelector);
  if (!panel || !grid || !svg) return;

  resetScaleLayout(view, opts);

  // Mobile responsive layouts use a stacked block flow; skip the
  // absolute-positioned overlay entirely so it doesn't fight CSS.
  const isMobile = window.matchMedia("(max-width: 768px)").matches;
  if (isMobile) {
    svg.innerHTML = "";
    return;
  }

  const tabletQuery = window.matchMedia("(max-width: 1023px) and (min-width: 769px)");
  const useTablet = tabletQuery.matches && opts.tablet;
  const cardWidth = useTablet ? opts.tablet.cardWidth : opts.cardWidth;
  const cardGap = useTablet ? opts.tablet.cardGap : opts.cardGap;
  const minCardWidth = Math.min(
    useTablet ? opts.tablet.minCardWidth : opts.minCardWidth,
    cardWidth,
  );
  const rowGap = opts.rowGap;

  const baseAxisTop = parseFloat(getComputedStyle(view).getPropertyValue("--axis-top")) || 0;
  const basePanelMin = parseFloat(getComputedStyle(panel).minHeight) || 0;

  // Pass 1 — width, row assignment and row heights, per lane.
  const lanes = [];
  for (const sideName of ["top", "bottom"]) {
    const lane = view.querySelector(opts.laneSelectorFor(sideName));
    if (!lane) continue;
    const laneStyle = getComputedStyle(lane);
    const entry = {
      sideName,
      lane,
      items: [],
      baseTop: parseFloat(laneStyle.top) || 0,
      baseHeight: parseFloat(laneStyle.height) || 0,
    };
    entry.height = entry.baseHeight;
    lanes.push(entry);

    const cards = Array.from(lane.querySelectorAll(opts.eventSelector));
    if (!cards.length) continue;

    const laneRect = lane.getBoundingClientRect();
    const laneWidth = laneRect.width;
    const width = laneCardWidth(cards.length, laneWidth, cardWidth, minCardWidth, cardGap);
    for (const card of cards) card.style.width = width + "px";

    const insetProperty = sideName === "top" ? "bottom" : "top";
    const inset = parseFloat(getComputedStyle(cards[0])[insetProperty]) || 0;

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
        idealLeft: cellCenterInLane - width / 2,
      };
    }).sort((a, b) => a.anchor - b.anchor);

    // As few rows as the lane can hold, dealt round-robin so each row spans the
    // whole day rather than a contiguous block — neighbours in a row are then
    // far enough apart that every card can sit near its own cell. Dealing them
    // in order instead would leave each new row starting where the last one ran
    // out of room, which stair-steps a busy day off to one side.
    const perRow = Math.max(1, Math.floor((laneWidth + cardGap) / (width + cardGap)));
    const rowCount = Math.ceil(items.length / perRow);
    const rows = Array.from({ length: rowCount }, () => []);
    items.forEach((it, index) => {
      it.row = index % rowCount;
      rows[it.row].push(it);
    });

    const maxLeft = Math.max(0, laneWidth - width);
    for (const row of rows) {
      // Forward pass: never overlap the card to the left. Backward pass: pull
      // the tail back inside the lane. Together they keep every card in view.
      let prevRight = -Infinity;
      for (const it of row) {
        it.left = Math.max(it.idealLeft, prevRight + cardGap);
        prevRight = it.left + width;
      }
      let nextLeft = laneWidth + cardGap;
      for (let index = row.length - 1; index >= 0; index -= 1) {
        row[index].left = Math.min(row[index].left, nextLeft - cardGap - width);
        nextLeft = row[index].left;
      }
      for (const it of row) it.left = Math.min(Math.max(it.left, 0), maxLeft);
    }

    const rowHeights = [];
    for (const it of items) {
      rowHeights[it.row] = Math.max(rowHeights[it.row] || 0, it.card.offsetHeight);
    }
    const rowInsets = [];
    let stacked = inset;
    for (let index = 0; index < rowHeights.length; index += 1) {
      rowInsets.push(stacked);
      stacked += rowHeights[index] + rowGap;
    }
    for (const it of items) {
      it.card.style.left = it.left + "px";
      it.card.style[insetProperty] = rowInsets[it.row] + "px";
    }

    entry.items = items;
    entry.height = Math.max(entry.baseHeight, stacked - rowGap + inset);
  }

  // Pass 2 — grow the lanes, and push the axis and the panel down with them so
  // the extra rows are laid out in real space instead of over the hour strip.
  const topLane = lanes.find((entry) => entry.sideName === "top");
  const axisShift = topLane ? Math.max(0, topLane.height - topLane.baseHeight) : 0;
  const axisTop = baseAxisTop + axisShift;
  if (axisShift > 0) view.style.setProperty("--axis-top", axisTop + "px");
  let panelExtent = axisTop;
  for (const entry of lanes) {
    const laneTop = entry.sideName === "top" ? entry.baseTop : entry.baseTop + axisShift;
    if (laneTop !== entry.baseTop) entry.lane.style.top = laneTop + "px";
    if (entry.height !== entry.baseHeight) entry.lane.style.height = entry.height + "px";
    panelExtent = Math.max(panelExtent, laneTop + entry.height + 24);
  }
  if (panelExtent > basePanelMin) {
    panel.style.minHeight = panelExtent + "px";
    view.style.minHeight = panelExtent + "px";
  }

  const panelRect = panel.getBoundingClientRect();
  svg.setAttribute("viewBox", `0 0 ${panelRect.width} ${panelRect.height}`);
  svg.style.width = panelRect.width + "px";
  svg.style.height = panelRect.height + "px";
  svg.innerHTML = "";

  const ns = "http://www.w3.org/2000/svg";
  const accent = getComputedStyle(view).getPropertyValue("--accent").trim() || "#0f4c81";

  // Connectors — drawn in panel coords so the SVG layer can overlap
  // both lanes and the central grid.
  for (const entry of lanes) {
    for (const it of entry.items) {
      if (!it.cell) continue;
      const cardRect = it.card.getBoundingClientRect();
      const cellRect = it.cell.getBoundingClientRect();

      const cardEdgeY = entry.sideName === "top" ? cardRect.bottom : cardRect.top;
      const cellEdgeY = entry.sideName === "top" ? cellRect.top : cellRect.bottom;

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
  minCardWidth: 132,
  cardGap: 14,
  rowGap: 12,
  tablet: { cardWidth: 140, minCardWidth: 120, cardGap: 10 },
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
  minCardWidth: 132,
  cardGap: 12,
  rowGap: 12,
  tablet: { cardWidth: 140, minCardWidth: 120, cardGap: 10 },
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
  minCardWidth: 132,
  cardGap: 12,
  rowGap: 12,
  tablet: { cardWidth: 140, minCardWidth: 120, cardGap: 10 },
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
  if (!buckets.some((bucket) => bucket && (bucket.best_origin || bucket.browser_origin))) {
    timeline.innerHTML = renderEmptyState(
      "nothing in this hour",
      `nothing in your journal for ${formatTime(hour, 0)}.`,
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
          <button class="minute-focus-node" type="button" data-month="${monthIndex}" data-day="${day}" data-return-day="true" aria-label="return to ${month.name} ${day}, ${month.year || ""}">
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
            const hasData = !!(bucket && (bucket.best_origin || bucket.browser_origin));
            // Availability tint: both = accent, screen-only = teal,
            // audio-only = coral, pages-only = blue, none = grey/disabled.
            let availClass = "avail-none";
            if (hasData && bucket.has_audio && bucket.has_screen) availClass = "avail-both";
            else if (hasData && bucket.has_screen) availClass = "avail-screen";
            else if (hasData && bucket.has_audio) availClass = "avail-audio";
            else if (hasData && bucket.has_browser) availClass = "avail-browser";
            const classes = ["segment-cell", event ? `timeline-focus timeline-${event.side}` : "", availClass].filter(Boolean).join(" ");
            const availLabel = hasData
              ? (bucket.has_audio && bucket.has_screen ? "audio + screen"
                 : bucket.has_screen ? "screen only"
                 : bucket.has_audio ? "audio only"
                 : bucket.has_browser ? "pages" : "metadata only")
              : "nothing kept";
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
        <div class="segment-empty">nothing kept in this slice</div>
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
  const browserOrigin = bucket && bucket.browser_origin ? bucket.browser_origin : null;
  const metaOrigin = origin || browserOrigin;

  // No data → render an empty-state river. (Cell shouldn't have been
  // clickable in the first place; this is a defensive fallback.)
  if (!metaOrigin) {
    return renderEmptySegment(monthIndex, day, hour, minute, focusLabel);
  }

  // Derive the segment's wall-clock start from its segment name's HHMMSS.
  // origin format: "YYYYMMDD/<stream>/<HHMMSS_LEN>" or "YYYYMMDD/<HHMMSS_LEN>"
  const parts = metaOrigin.split("/");
  const segName = parts[parts.length - 1];
  const segMatch = /^(\d{2})(\d{2})(\d{2})_(\d{1,6})$/.exec(segName);
  const startSec = segMatch ? parseInt(segMatch[1],10)*3600 + parseInt(segMatch[2],10)*60 + parseInt(segMatch[3],10) : (hour*3600 + minute*60);
  const dur = segMatch ? parseInt(segMatch[4], 10) : 300;
  const stream = parts.length === 3 ? parts[1] : "";
  const dayStr = `${parts[0].slice(0,4)}-${parts[0].slice(4,6)}-${parts[0].slice(6,8)}`;
  const meta = { day: dayStr, startSec, durationSec: dur, stream };

  const sample = origin ? await loadSegment(origin) : null;
  const browserSample = browserOrigin && browserOrigin !== origin
    ? await loadSegment(browserOrigin)
    : sample;
  const primarySample = sample || browserSample;
  if (!primarySample) {
    return renderEmptySegment(monthIndex, day, hour, minute, focusLabel);
  }

  // Stash for the click-driven detail handlers.
  _activeSegment = primarySample;
  _activeMeta = meta;
  _activeBrowserFiles = browserSample?.browser || [];

  const audioHeader = primarySample.audio?.header || {};
  const screenHeader = primarySample.screen?.header || {};
  const audioLines = primarySample.audio?.lines || [];
  const screenFrames = primarySample.screen?.frames || [];
  const browserFiles = _activeBrowserFiles;
  const browserHasContent = hasBrowserContent(browserFiles);
  const pageUpdateCount = browserChangeCount(browserFiles);

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
    : (browserHasContent ? "" : `<div class="river-empty">no microphone input in this slice</div>`);

  const browserMarks = browserFiles.map((site, siteIndex) => {
    return (site.entries || []).map((entry, entryIndex) => {
      if (entry.kind !== "change") return "";
      const offset = Math.max(0, Math.min(dur, browserOffsetSeconds(meta, entry.ts)));
      const tipText = `${browserTimeLabel(entry.ts)} · ${site.site_name || site.site || "pages"}\n${(entry.markdown || "").slice(0, 200)}`;
      return `<button class="river-browser-mark"
        data-browser-site="${siteIndex}"
        data-browser-entry="${entryIndex}"
        style="left:${fmtPct(offset)};"
        title="${escapeHtml(tipText)}"
        aria-label="${escapeHtml(tipText)}"
        type="button"></button>`;
    }).join("");
  }).join("");

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

      <section class="segment-panel" aria-label="${month.name} ${day}, ${month.year || ""} ${focusLabel} — what your journal kept">
        <div class="timeline-focus-heading">
          <button class="segment-focus-node" type="button"
                  data-month="${monthIndex}" data-day="${day}" data-hour="${hour}"
                  data-return-hour="true"
                  aria-label="return to ${formatHour(hour)} on ${month.name} ${day}, ${month.year || ""}">
            ${focusLabel}
          </button>
        </div>

        <header class="segment-header">
          <div class="seg-header-row">
            <span class="seg-header-time">${meta.day} · ${startHHMM} → ${endHHMM} · ${minutesStr}</span>
            <span class="seg-header-mid">${escapeHtml(meta.stream || "—")}</span>
            <span class="seg-header-end">${escapeHtml(setting)} setting</span>
          </div>
          ${renderArtifactTruth({
            ...primarySample,
            generated_at_ms: primarySample.timeline?.generated_at_ms,
            provenance: primarySample.timeline?.provenance,
          })}
          ${topics.length ? `<div class="seg-topics">${topics.map((t) => `<span class="topic-chip">${escapeHtml(t)}</span>`).join("")}</div>` : ""}
        </header>

        <div class="segment-river">
          <div class="river-screen" aria-label="screen frames your journal kept">
            ${screenMarks}
          </div>
          <div class="river-axis">
            ${axisMarks}
          </div>
          <div class="river-browser" aria-label="page updates">
            ${browserMarks}
          </div>
          <div class="river-audio" aria-label="microphone input">
            ${audioMarks}
          </div>
        </div>

        <div class="segment-detail" id="segment-detail">
          ${browserHasContent ? renderBrowserSections(browserFiles) : `<div class="seg-detail-empty">click a tick, audio dot, or page mark to inspect that moment</div>`}
        </div>

        <footer class="segment-footer">
          ${screenFrames.length} frames analyzed
          · ${audioLines.length} transcript line${audioLines.length === 1 ? "" : "s"}
          · ${featuredCount} frames with extracted text
          · ${pageUpdateCount} page update${pageUpdateCount === 1 ? "" : "s"}
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
  for (const mark of document.querySelectorAll(".river-browser-mark[data-browser-site]")) {
    mark.addEventListener("click", (e) => {
      e.stopPropagation();
      const siteIndex = parseInt(mark.getAttribute("data-browser-site"), 10);
      const entryIndex = parseInt(mark.getAttribute("data-browser-entry"), 10);
      if (mark.classList.contains("is-active")) clearSegmentDetail();
      else showBrowserDetail(siteIndex, entryIndex);
    });
  }
}

function renderEdgeDay(monthIndex, day, position) {
  const month = months[monthIndex];
  // A bare "4" reads as nothing; name the day and point the chevron the way
  // it moves. The accessible name stays the full date either way.
  const label = position === "prev"
    ? `‹ ${month.name.slice(0, 3)} ${day}`
    : `${month.name.slice(0, 3)} ${day} ›`;
  return `
    <button class="edge-day timeline-${position}" type="button" data-month="${monthIndex}" data-day="${day}" aria-label="open ${month.name} ${day}, ${month.year || ""}">
      ${escapeHtml(label)}
    </button>
  `;
}

function renderEdgeHour(monthIndex, day, hour, position) {
  const month = months[monthIndex];
  return `
    <button class="edge-hour timeline-${position}" type="button" data-month="${monthIndex}" data-day="${day}" data-hour="${hour}" aria-label="open ${formatHour(hour)} on ${month.name} ${day}, ${month.year || ""}">
      ${formatHour(hour)}
    </button>
  `;
}

function renderEdgeSegment(monthIndex, day, hour, minute, position) {
  return `
    <button class="edge-segment timeline-${position}" type="button" data-month="${monthIndex}" data-day="${day}" data-hour="${hour}" data-minute="${minute}" aria-label="open ${formatTime(hour, minute)}">
      ${formatTime(hour, minute)}
    </button>
  `;
}

function renderEdgeMonth(month, index, position) {
  return `
    <button class="edge-node timeline-${position} accent-${month.accent}" type="button" data-month="${index}" aria-label="open ${month.name} ${month.year || ""}">
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
    await renderAllHistory();
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
    clearGridCache();
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
