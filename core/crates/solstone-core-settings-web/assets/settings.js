// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  // --- owner-facing strings ---
  const REDACTION_RULE_SINGULAR = 'rule';
  const REDACTION_RULE_PLURAL = 'rules';
  const VISION_CATEGORY_SINGULAR = 'category';
  const VISION_CATEGORY_PLURAL = 'categories';
  const STREAM_OVERRIDE_SINGULAR = 'override';
  const STREAM_OVERRIDE_PLURAL = 'overrides';
  // --- end owner-facing strings ---

  function countLine(count, singular, plural) {
    const number = Math.max(0, Math.trunc(Number(count) || 0));
    return `${number} ${number === 1 ? singular : plural}`;
  }

  function redactionRulesLine(count) {
    return countLine(count, REDACTION_RULE_SINGULAR, REDACTION_RULE_PLURAL);
  }

  function visionCategoriesLine(count) {
    return countLine(count, VISION_CATEGORY_SINGULAR, VISION_CATEGORY_PLURAL);
  }

  function streamOverridesLine(count) {
    return countLine(count, STREAM_OVERRIDE_SINGULAR, STREAM_OVERRIDE_PLURAL);
  }

  function buildStreamOverridesDrawerProps(streams, perStream) {
    const streamList = Array.isArray(streams) ? streams : [];
    const streamCount = streamList.length;
    if (streamCount === 0) return null;
    const overrides = perStream && typeof perStream === 'object' ? perStream : {};
    const configuredCount = streamList.filter((stream) => overrides[stream?.name]?.raw_media).length;
    const bodyHtml = '<p style="color: #666; font-size: 0.85em; margin: 0 0 1em 0;">override the global retention mode for individual streams.</p><div id="streamOverridesList"></div>';
    return {
      id: 'stream-overrides',
      open: false,
      label: 'per-stream overrides',
      line: streamOverridesLine(configuredCount),
      bodyHtml,
    };
  }

  function resolve(copy, key) {
    if (!copy || typeof key !== 'string' || !key) return '';
    const value = key.split('.').reduce((current, part) => {
      if (current === undefined || current === null) return undefined;
      return current[part];
    }, copy);
    return value === undefined || value === null ? '' : String(value);
  }

  function applyCopyAttr(el, assignment, copy) {
    const separator = assignment.indexOf(':');
    if (separator <= 0) return;
    const attr = assignment.slice(0, separator).trim();
    const key = assignment.slice(separator + 1).trim();
    if (!attr || !key || typeof el.setAttribute !== 'function') return;
    el.setAttribute(attr, resolve(copy, key));
  }

  function applyCopy(root, copy) {
    if (!root || typeof root.querySelectorAll !== 'function') return;
    root.querySelectorAll('[data-copy]').forEach((el) => {
      el.textContent = resolve(copy, el.dataset.copy);
    });
    root.querySelectorAll('[data-copy-attr]').forEach((el) => {
      String(el.dataset.copyAttr || '')
        .split(';')
        .map((part) => part.trim())
        .filter(Boolean)
        .forEach((assignment) => applyCopyAttr(el, assignment, copy));
    });
  }

  function applyThinkingSurfaces(root, value) {
    if (!root || typeof root.querySelectorAll !== 'function') return;
    root.querySelectorAll('input[name="thinking_surfaces"]').forEach((input) => {
      input.checked = input.value === value;
    });
  }

  function formatTitle(template, title) {
    return String(template || '').split('{title}').join(String(title || ''));
  }

  function findById(root, id) {
    if (!root) return null;
    if (typeof root.getElementById === 'function') return root.getElementById(id);
    if (typeof root.querySelector === 'function') return root.querySelector(`#${id}`);
    return null;
  }

  function documentFor(root) {
    return root?.ownerDocument || global.document || null;
  }

  function createEl(doc, tag, className = '') {
    const el = doc.createElement(tag);
    if (className) el.className = className;
    return el;
  }

  function clear(el) {
    if (!el) return;
    if (typeof el.replaceChildren === 'function') {
      el.replaceChildren();
      return;
    }
    while (el.firstChild && typeof el.removeChild === 'function') {
      el.removeChild(el.firstChild);
    }
    el.textContent = '';
  }

  function setFacetColor(container, color) {
    if (!container || !color) return;
    if (container.style && typeof container.style.setProperty === 'function') {
      container.style.setProperty('--facet-color', color);
    } else if (container.style) {
      container.style['--facet-color'] = color;
    }
  }

  function renderFacetDetail(root, facet, copy) {
    const doc = documentFor(root);
    const container = findById(root, 'settings-facet-detail-view') || root;
    if (!doc || !container) return null;

    const config = facet?.config && typeof facet.config === 'object' ? facet.config : {};
    const slug = String(facet?.facet || facet?.name || '');
    const title = String(config.title || slug);
    const color = String(config.color || '');
    const emoji = String(config.emoji || '');
    const muted = Boolean(config.muted);

    clear(container);
    container.className = 'facet-detail-page';
    container.setAttribute('aria-labelledby', 'facetDetailHeading');
    container.hidden = false;
    setFacetColor(container, color);

    const hiddenHeading = createEl(doc, 'h1', 'visually-hidden');
    hiddenHeading.textContent = 'settings';
    container.appendChild(hiddenHeading);

    const back = createEl(doc, 'a', 'facet-detail-back');
    back.href = '/app/settings#facets';
    back.textContent = copy.FACET_DETAIL_TERTIARY_ESCAPE || '';
    container.appendChild(back);

    const heading = createEl(doc, 'h2', 'facet-detail-heading');
    heading.id = 'facetDetailHeading';
    heading.textContent = formatTitle(copy.FACET_DETAIL_SUCCESS_HEADING, title);
    container.appendChild(heading);

    const meta = createEl(doc, 'div', 'facet-detail-meta');
    if (emoji) {
      const emojiEl = createEl(doc, 'span', 'facet-detail-emoji');
      emojiEl.setAttribute('aria-hidden', 'true');
      emojiEl.textContent = emoji;
      meta.appendChild(emojiEl);
    }
    if (color) {
      const swatch = createEl(doc, 'span', 'facet-detail-swatch');
      swatch.style.backgroundColor = color;
      swatch.setAttribute('aria-label', `${title} color`);
      meta.appendChild(swatch);
    }
    if (muted) {
      const mutedEl = createEl(doc, 'span', 'facet-detail-muted');
      mutedEl.textContent = 'muted';
      meta.appendChild(mutedEl);
    }
    container.appendChild(meta);

    const value = createEl(doc, 'p', 'facet-detail-copy');
    value.textContent = formatTitle(copy.FACET_DETAIL_VALUE_FRAMING, title);
    container.appendChild(value);

    const actions = createEl(doc, 'div', 'facet-detail-actions');
    const primary = createEl(doc, 'a', 'facet-detail-action facet-detail-action--primary');
    primary.href = '/app/entities/';
    primary.dataset.facetSlug = slug;
    primary.id = 'facetDetailPrimary';
    primary.textContent = formatTitle(copy.FACET_DETAIL_PRIMARY_CTA, title);
    primary.addEventListener('click', () => {
      if (!slug) return;
      const expires = new Date();
      expires.setFullYear(expires.getFullYear() + 1);
      doc.cookie = `selectedFacet=${slug}; expires=${expires.toUTCString()}; path=/; SameSite=Lax`;
    });
    actions.appendChild(primary);

    const secondary = createEl(doc, 'a', 'facet-detail-action facet-detail-action--secondary');
    secondary.href = '/app/settings#facets';
    secondary.textContent = copy.FACET_DETAIL_SECONDARY_CTA || '';
    actions.appendChild(secondary);

    const tertiary = createEl(doc, 'a', 'facet-detail-action facet-detail-action--tertiary');
    tertiary.href = '/app/settings';
    tertiary.textContent = copy.FACET_DETAIL_TERTIARY_ESCAPE || '';
    actions.appendChild(tertiary);

    container.appendChild(actions);
    return container;
  }

  const SettingsRender = {
    applyCopy,
    applyCopyAttr,
    applyThinkingSurfaces,
    buildStreamOverridesDrawerProps,
    formatTitle,
    redactionRulesLine,
    renderFacetDetail,
    resolve,
    streamOverridesLine,
    visionCategoriesLine,
  };
  global.SettingsRender = SettingsRender;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = SettingsRender;
  }
})(typeof window !== 'undefined' ? window : globalThis);
