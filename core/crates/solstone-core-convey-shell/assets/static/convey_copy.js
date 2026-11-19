// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const bundle = globalThis.SolstoneI18nBundles['journal-window'];
  const sourceLocale = 'en-US';
  const locale = globalThis.document?.documentElement?.lang;
  const language = locale && locale !== 'en' ? locale : sourceLocale;
  const prefix = 'journal.window.chrome.';
  const argumentsByKey = {
    'zone.time': [['label', '{label}']],
    'day.zones.part': [['zones', '{zones}']],
    'day.zones.all': [['zones', '{zones}']],
    'console.snapshot.heading': [['count', '{count}']],
  };

  function renderMessage(selected, id, args = []) {
    return bundle.render(selected, id, args, { useIsolating: selected !== sourceLocale });
  }

  function render(id, args) {
    return renderMessage(language, id, args);
  }

  function renderEnglish(id, args) {
    return renderMessage(sourceLocale, id, args);
  }

  function escapeMarkup(value) {
    return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;').replaceAll('"', '&quot;').replaceAll("'", '&#39;');
  }

  // Only shipped workspace templates contain message markers. Replace them
  // before inserting the workspace so the locale never changes asset bytes.
  function renderWorkspace(markup) {
    return markup.replace(/\{\{(!?)(journal\.window\.[a-z0-9.]+)\}\}/g, (_, restricted, id) =>
      escapeMarkup(restricted ? renderEnglish(id) : render(id)));
  }

  window.JournalI18n = Object.freeze({
    render,
    renderEnglish,
    renderHtml: (id, args) => escapeMarkup(render(id, args)),
    renderEnglishHtml: (id, args) => escapeMarkup(renderEnglish(id, args)),
    renderWorkspace,
    locale: language,
    formatLocale: language === sourceLocale ? undefined : language,
  });
  window.CONVEY_COPY = Object.freeze(Object.fromEntries(
    bundle.messageIds.filter(id => id.startsWith(prefix)).map(id => {
      const key = id.slice(prefix.length);
      const format = key === 'report.privacy.note' ? renderEnglish : render;
      return [key.replaceAll('.', '_').toUpperCase(), format(id, argumentsByKey[key])];
    }),
  ));
})();
