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

  function render(id, args = []) {
    const selected = id === prefix + 'report.privacy.note' ? sourceLocale : language;
    return bundle.render(selected, id, args, { useIsolating: selected !== sourceLocale });
  }

  window.JournalI18n = Object.freeze({
    render,
    locale: language,
    formatLocale: language === sourceLocale ? undefined : language,
  });
  window.CONVEY_COPY = Object.freeze(Object.fromEntries(
    bundle.messageIds.filter(id => id.startsWith(prefix)).map(id => {
      const key = id.slice(prefix.length);
      return [key.replaceAll('.', '_').toUpperCase(), render(id, argumentsByKey[key])];
    }),
  ));
})();
