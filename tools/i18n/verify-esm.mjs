// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import fs from 'node:fs/promises';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import crypto from 'node:crypto';

const [directory, expectedFile] = process.argv.slice(2);
if (!directory || !expectedFile) throw new Error('usage: node verify-esm.mjs GENERATED_ROOT EXPECTED_JSON');
const root = path.resolve(directory);
const bytes = await fs.readFile(expectedFile);
const spec = JSON.parse(bytes.toString('utf8'));
if (typeof spec.entry !== 'string' || !Array.isArray(spec.cases) || !spec.cases.length) throw new Error('Nonempty exact case fixture required');
const entry = path.resolve(root, spec.entry);
if (!entry.startsWith(root + path.sep)) throw new Error('Entry must be inside generated output');
if (spec.cases.some(c => typeof c.expected !== 'string' && c.expected_error !== true)) throw new Error('Pending expectations cannot qualify a runtime');
const api = await import(pathToFileURL(entry));
const results = [];
for (const item of spec.cases) {
  try {
    const args = item.arguments.map(([name, value]) => [name,
      value && typeof value === 'object' && value.type === 'integer' ? api.integer(value.value) :
      value && typeof value === 'object' && value.type === 'cardinal' ? api.cardinal(value.value) : value]);
    const actual = await api.render(item.locale, item.message_id, args);
    results.push({case:item, actual, pass:!item.expected_error && actual === item.expected});
  } catch (error) {
    const text = String(error);
    results.push({case:item, error:text, pass:item.expected_error === true && (!item.error_contains || text.includes(item.error_contains))});
  }
}
const passed = results.every(c => c.pass);
console.log(JSON.stringify({reader:'Node generated ESM API', node:process.version, icu:process.versions.icu,
  expected_sha256:crypto.createHash('sha256').update(bytes).digest('hex'), results, aggregate_pass:passed}, null, 2));
if (!passed) process.exitCode = 1;
