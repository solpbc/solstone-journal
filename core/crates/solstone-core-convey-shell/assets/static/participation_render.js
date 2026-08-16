// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Renders participation JSON with these classes:
// ad-participation, ad-participation-section, ad-participation-section-header,
// ad-participation-cards, ad-participation-card,
// ad-participation-card--muted, ad-participation-name,
// ad-participation-less-certain, ad-participation-provenance,
// ad-participation-context, ad-participation-empty,
// ad-participation-unavailable.
(function () {
  const copy = window.solActivitiesCopy;
  const SOURCE_PHRASES = {
    voice: copy.provVoice,
    speaker_label: copy.provSpeakerLabel,
    transcript: copy.provTranscript,
    screen: copy.provScreen,
    other: copy.provOther
  };

  function escapeHtml(value) {
    return String(value).replace(/[&<>"']/g, (char) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;"
    })[char]);
  }

  function unavailableHtml() {
    return `<div class="ad-participation-unavailable">${escapeHtml(copy.unavailable)}</div>`;
  }

  function emptyHtml() {
    return `<div class="ad-participation-empty">${escapeHtml(copy.empty)}</div>`;
  }

  function isPlainObject(value) {
    return typeof value === "object" && value !== null && !Array.isArray(value);
  }

  function normalizeEntry(entry) {
    if (!isPlainObject(entry)) return null;
    if (typeof entry.name !== "string") return null;
    if (entry.role !== "attendee" && entry.role !== "mentioned") return null;
    if (typeof entry.source !== "string") return null;
    if (!Object.prototype.hasOwnProperty.call(entry, "confidence")) return null;
    if (typeof entry.context !== "string") return null;

    const confidence = entry.confidence;
    const confident = Number.isFinite(confidence) && confidence >= 0 && confidence <= 1 && confidence >= 0.5;
    const muted = !confident;

    return {
      name: entry.name,
      role: entry.role,
      source: entry.source,
      context: entry.context,
      muted
    };
  }

  function renderCard(entry) {
    const phrase = SOURCE_PHRASES[entry.source] ?? SOURCE_PHRASES.other;
    const mutedClass = entry.muted ? " ad-participation-card--muted" : "";
    const lessCertain = entry.muted
      ? `<span class="ad-participation-less-certain">${escapeHtml(copy.lessCertain)}</span>`
      : "";
    const context = entry.context.trim() !== ""
      ? `<div class="ad-participation-context">${escapeHtml(entry.context)}</div>`
      : "";

    return `<article class="ad-participation-card${mutedClass}">`
      + `<div><span class="ad-participation-name">${escapeHtml(entry.name)}</span>${lessCertain}</div>`
      + `<div class="ad-participation-provenance">${escapeHtml(phrase)}</div>`
      + context
      + `</article>`;
  }

  function renderSection(title, entries) {
    if (!entries.length) return "";
    const cards = entries.map(renderCard).join("");
    return `<section class="ad-participation-section">`
      + `<h3 class="ad-participation-section-header">${escapeHtml(title)}</h3>`
      + `<div class="ad-participation-cards">${cards}</div>`
      + `</section>`;
  }

  function render(rawContent) {
    let parsed;
    try {
      parsed = JSON.parse(rawContent);
    } catch (error) {
      return unavailableHtml();
    }

    if (!isPlainObject(parsed) || !Array.isArray(parsed.participation)) {
      return unavailableHtml();
    }

    if (parsed.participation.length === 0) {
      return emptyHtml();
    }

    const attendees = [];
    const mentioned = [];
    parsed.participation.forEach((rawEntry) => {
      const entry = normalizeEntry(rawEntry);
      if (!entry) return;
      if (entry.role === "attendee") {
        attendees.push(entry);
      } else {
        mentioned.push(entry);
      }
    });

    if (!attendees.length && !mentioned.length) {
      return emptyHtml();
    }

    return `<div class="ad-participation">`
      + renderSection(copy.sectionAttendees, attendees)
      + renderSection(copy.sectionMentioned, mentioned)
      + `</div>`;
  }

  window.solActivitiesParticipation = { render };
})();
