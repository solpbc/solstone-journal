// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  function countLine(count, singular, plural) {
    const number = Math.max(0, Math.trunc(Number(count) || 0));
    return global.JournalI18n.render(number === 1 ? singular : plural, [['count', String(number)]]);
  }

  function redactionRulesLine(count) {
    return countLine(count, 'journal.window.settings.redaction.one', 'journal.window.settings.redaction.many');
  }

  function visionCategoriesLine(count) {
    return countLine(count, 'journal.window.settings.vision.category.one', 'journal.window.settings.vision.category.many');
  }

  function streamOverridesLine(count) {
    return countLine(count, 'journal.window.settings.stream.override.one', 'journal.window.settings.stream.override.many');
  }

  function buildStreamOverridesDrawerProps(streams, perStream) {
    const streamList = Array.isArray(streams) ? streams : [];
    const streamCount = streamList.length;
    if (streamCount === 0) return null;
    const overrides = perStream && typeof perStream === 'object' ? perStream : {};
    const configuredCount = streamList.filter((stream) => overrides[stream?.name]?.raw_media).length;
    const bodyHtml = '<p style="color: var(--ink-soft); font-size: 0.85em; margin: 0 0 1em 0;">' + global.JournalI18n.renderHtml('journal.window.settings.stream.override.description') + '</p><div id="streamOverridesList"></div>';
    return {
      id: 'stream-overrides',
      open: false,
      label: global.JournalI18n.render('journal.window.settings.stream.override.label'),
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

  function renderFacetDetailPage(root, facet) {
    const container = root?.getElementById?.('settings-facet-detail-view')
      || root?.querySelector?.('#settings-facet-detail-view');
    if (!container) return null;

    const config = facet?.config && typeof facet.config === 'object' ? facet.config : {};
    const slug = String(facet?.facet || facet?.name || '');
    const title = String(config.title || slug);
    const color = String(config.color || '');
    const muted = Boolean(config.muted);
    const heading = container.querySelector('#facetDetailHeading');
    const swatch = container.querySelector('#facetDetailSwatch');
    const muteAction = container.querySelector('#facetMuteAction');

    container.setAttribute('aria-labelledby', 'facetDetailHeading');
    if (container.style && typeof container.style.setProperty === 'function') {
      container.style.setProperty('--facet-color', color);
    } else if (container.style) {
      container.style['--facet-color'] = color;
    }
    if (heading) heading.textContent = title;
    if (swatch) {
      const mark = {
        color,
        emoji: String(config.emoji || ''),
        icon: String(config.icon || ''),
        icon_svg: String(config.icon_svg || ''),
      };
      if (global.ConveyFacetMark) {
        global.ConveyFacetMark.apply(swatch, mark, 'lg', 'facet-detail-swatch');
      } else {
        swatch.style.backgroundColor = color;
        swatch.hidden = !color;
      }
    }
    if (muteAction) {
      muteAction.textContent = muted
        ? global.JournalI18n.render('journal.window.settings.facet.unmute')
        : global.JournalI18n.render('journal.window.settings.facet.mute');
      const actionLabel = muted
        ? global.JournalI18n.render('journal.window.settings.facet.unmute.name', [['title', title]])
        : global.JournalI18n.render('journal.window.settings.facet.mute.name', [['title', title]]);
      muteAction.setAttribute('aria-label', actionLabel);
    }
    return container;
  }

  const SettingsRender = {
    applyCopy,
    applyCopyAttr,
    buildStreamOverridesDrawerProps,
    redactionRulesLine,
    renderFacetDetailPage,
    resolve,
    streamOverridesLine,
    visionCategoriesLine,
  };
  global.SettingsRender = SettingsRender;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = SettingsRender;
  }
})(typeof window !== 'undefined' ? window : globalThis);
