// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The sun-arc engine's pure day/night math, run under plain Node with no browser and no DOM:
// sunarc.js is loaded into a vm context, its tokens are read from tokens.css, and frame() is
// checked against the worked numbers the in-browser smoke page (assets/static/tests/sunarc.html)
// also carries. The smoke page stays the place for DOM, timers and listeners.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');
const staticDir = path.join(crateDir, 'assets', 'static');

const context = { window: {}, console };
vm.createContext(context);
vm.runInContext(fs.readFileSync(path.join(staticDir, 'sunarc.js'), 'utf8'), context, { filename: 'sunarc.js' });
const SunArc = context.window.SunArc;
assert.ok(SunArc && typeof SunArc.frame === 'function', 'sunarc.js exposes SunArc.frame');

// Light values only: the engine reads its grounds from the fixed --sunarc-* tokens, so the
// dark appearance's token file plays no part here.
const css = fs.readFileSync(path.join(staticDir, 'tokens.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
const props = {};
for (const m of css.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/gi)) props[m[1]] = m[2].trim();
const tokens = SunArc.parseTokens({ getPropertyValue: (name) => props[name] || '' });
assert.ok(SunArc.tokensValid(tokens), 'tokens.css supplies every token the engine requires');

let cases = 0;
const hexDiff = (a, b) => Math.max(...[1, 3, 5].map((k) => Math.abs(parseInt(a.substr(k, 2), 16) - parseInt(b.substr(k, 2), 16))));

// The spec's § 4a worked numbers: Denver 2026-09-23, both appearances, at the spec's 393x852
// and convey's 1280x820. [label, minutes, W, H, appearance, sun opacity, w, halo alpha,
// twilight glow [x, y, r, alpha, corner alpha] or null]
const RISE = 408.25991130150896;
const SET = 1135.6484964224096;
const VECTORS = [
        ['13:00',780,393,852,'light',0.55,0,0.22,null],
        ['13:00',780,393,852,'dark',0.2,0,0.22,null],
        ['13:00',780,1280,820,'light',0.55,0,0.22,null],
        ['13:00',780,1280,820,'dark',0.2,0,0.22,null],
        ['sunset',1136,393,852,'light',0.1444,0.7344,0.0578,[601.86,1019.65,832.39,0.6977,0.3728]],
        ['sunset',1136,393,852,'dark',0.0525,0.7344,0.0578,[601.86,1019.65,832.39,0.4553,0.2433]],
        ['sunset',1136,1280,820,'light',0.1444,0.7344,0.0578,[1688.07,1199.56,1736.79,0.6977,0.3736]],
        ['sunset',1136,1280,820,'dark',0.0525,0.7344,0.0578,[1688.07,1199.56,1736.79,0.4553,0.2439]],
        ['19:41',1181,393,852,'light',0,0.9937,null,[620.44,1086.72,832.39,0.944,0.4161]],
        ['19:41',1181,393,852,'dark',0,0.9937,null,[620.44,1086.72,832.39,0.6161,0.2716]],
        ['19:41',1181,1280,820,'light',0,0.9937,null,[1759.4,1304.68,1736.79,0.944,0.4162]],
        ['19:41',1181,1280,820,'dark',0,0.9937,null,[1759.4,1304.68,1736.79,0.6161,0.2716]],
        ['22:00',1320,393,852,'light',0,0.4341,null,[642.28,1176.83,832.39,0.4124,0.1521]],
        ['22:00',1320,393,852,'dark',0,0.4341,null,[642.28,1176.83,832.39,0.2692,0.0993]],
        ['22:00',1320,1280,820,'light',0,0.4341,null,[1849.86,1447.74,1736.79,0.4124,0.1532]],
        ['22:00',1320,1280,820,'dark',0,0.4341,null,[1849.86,1447.74,1736.79,0.2692,0.1]],
        ['01:00',60,393,852,'light',0,0,null,null],
        ['01:00',60,393,852,'dark',0,0,null,null],
        ['01:00',60,1280,820,'light',0,0,null,null],
        ['01:00',60,1280,820,'dark',0,0,null,null],
        ['05:30',330,393,852,'light',0,0.9381,null,[-249.94,-244.96,832.39,0.8912,0.3749]],
        ['05:30',330,393,852,'dark',0,0.9381,null,[-249.94,-244.96,832.39,0.5816,0.2447]],
        ['05:30',330,1280,820,'light',0,0.9381,null,[-524.3,-489.22,1736.79,0.8912,0.3798]],
        ['05:30',330,1280,820,'dark',0,0.9381,null,[-524.3,-489.22,1736.79,0.5816,0.2478]],
];
for (const [label, m, W, H, ap, op, w, haloA, tw] of VECTORS) {
  const f = SunArc.frame(W, H, m, RISE, SET, ap, tokens);
  const at = `${label} ${W}x${H} ${ap}`;
  assert.ok(Math.abs(f.opacity - op) <= 0.001, `${at}: sun ${f.opacity} != ${op}`);
  assert.ok(Math.abs(f.twilight.w - w) <= 0.001, `${at}: w ${f.twilight.w} != ${w}`);
  if (haloA === null) assert.ok(!f.halo, `${at}: halo drawn, expected none`);
  else assert.ok(f.halo && Math.abs(f.halo.a - haloA) <= 0.001, `${at}: halo ${f.halo && f.halo.a} != ${haloA}`);
  assert.strictEqual(!f.glow, !tw, `${at}: twilight glow ${f.glow ? 'drawn' : 'absent'}`);
  if (tw) {
    const [x, y, r, a, corner] = tw;
    assert.ok(Math.abs(f.glow.x - x) <= 0.5 && Math.abs(f.glow.y - y) <= 0.5, `${at}: glow centre (${f.glow.x}, ${f.glow.y}) != (${x}, ${y})`);
    assert.ok(Math.abs(f.glow.r - r) <= 0.5, `${at}: glow radius ${f.glow.r} != ${r}`);
    assert.ok(Math.abs(f.glow.a - a) <= 0.001, `${at}: glow alpha ${f.glow.a} != ${a}`);
    const c = Math.max(SunArc.glowAt(f.glow, W, H, tokens), SunArc.glowAt(f.glow, 0, 0, tokens));
    assert.ok(Math.abs(c - corner) <= 0.002, `${at}: corner alpha ${c} != ${corner}`);
  }
  cases += 1;
}

// True dark: inside the 180-minute window around solar midnight nothing glows, the sun is out,
// and the ground is the appearance's true-dark ground, minute by minute through a whole day.
const win = SunArc.trueDarkWindow(RISE, SET, tokens.twilightMinutes, tokens.trueDarkMinutes);
const span = ((win.to - win.from) % 1440 + 1440) % 1440;
assert.ok(Math.abs(span - 180) <= 1, `true-dark window is ${span} min, expected 180`);
for (let m = 0; m < 1440; m += 1) {
  const inside = ((m - win.from) % 1440 + 1440) % 1440 < span;
  if (!inside) continue;
  for (const ap of ['light', 'dark']) {
    const f = SunArc.frame(1280, 820, m, RISE, SET, ap, tokens);
    const deep = ap === 'dark' ? tokens.groundDarkDeep : tokens.groundLightDeep;
    assert.ok(!f.glow && !f.halo && f.opacity === 0 && hexDiff(f.ground, deep) <= 1, `true dark at minute ${m} (${ap}) draws light or misses the deep ground`);
  }
}
cases += 1;

// A sunset after local midnight (rise 175, set 3, as in Reykjavik in June) reads as day before
// it and eases, with no one-minute jump in the twilight weight.
{
  const f = (m) => SunArc.frame(393, 852, m, 175, 3, 'dark', tokens).twilight.w;
  assert.ok(Math.abs(f(10) - 0.877) <= 0.002, `wrapped sunset: w at 00:10 is ${f(10)}, expected 0.877`);
  let maxStep = 0;
  for (let m = 1; m < 1440; m += 1) maxStep = Math.max(maxStep, Math.abs(f(m) - f(m - 1)));
  assert.ok(maxStep <= 0.2, `wrapped sunset: w jumps ${maxStep} in one minute`);
  cases += 1;
}

console.log(`SUNARC CASES: ${cases} passed (${VECTORS.length} worked vectors)`);
