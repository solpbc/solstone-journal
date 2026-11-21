// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

export const locale = document.documentElement.lang;
const catalog = globalThis.SolstoneI18nBundles['journal-windows-app'];

export function t(key, args = {}, tag = locale) {
  return catalog.render(tag, 'journal.windows.' + key, Object.entries(args), { useIsolating: tag !== 'en-US' });
}
