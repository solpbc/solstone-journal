// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Writes cardinal categories from the host's Intl.PluralRules to stdout.
// Run by hand when the language list or the CLDR release moves:
//   node tools/i18n/fixtures/plural-goldens/generate.mjs > tools/i18n/fixtures/plural-goldens/cldr.json

const LANGUAGES = [
  "am", "ar", "az", "bn", "cs", "de", "el", "en", "es", "fa", "fil", "fr", "he", "hi", "hu", "hy",
  "id", "it", "ja", "ka", "kk", "km", "ko", "mr", "ms", "my", "nl", "pa", "pl", "pt", "pt-BR",
  "pt-PT", "ro", "ru", "sv", "sw", "ta", "te", "th", "tr", "uk", "ur", "vi", "yo", "zh",
];

const PROBES = [];
for (let n = 0; n <= 210; n += 1) PROBES.push(n);
for (const n of [
  999, 1000, 1001, 1002, 1005, 1011, 1021, 1101, 10000, 100000, 999999, 1000000, 1000001, 1000002,
  2000000, 2000001, 10000000, 100000000, 1000000000, 2147483647,
]) {
  PROBES.push(n);
}

const categories = {};
for (const language of LANGUAGES) {
  const rules = new Intl.PluralRules(language, { type: "cardinal" });
  if (rules.resolvedOptions().locale.split("-")[0] !== language.split("-")[0]) {
    throw new Error(`no plural data for ${language}`);
  }
  categories[language] = PROBES.map((n) => rules.select(n)).join(" ");
}

const out = { cldr: process.versions.cldr, probes: PROBES, categories };
console.log(JSON.stringify(out, null, 1));
