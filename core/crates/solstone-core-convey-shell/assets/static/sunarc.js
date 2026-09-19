// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  'use strict';

  function parseNumeric(val) {
    if (typeof val === 'number') {
      return Number.isFinite(val) ? val : null;
    }
    if (typeof val !== 'string') return null;
    val = val.trim();
    if (!val) return null;
    if (val.endsWith('%')) {
      const num = parseFloat(val.slice(0, -1));
      return Number.isFinite(num) ? num / 100 : null;
    }
    if (val.endsWith('deg')) {
      const num = parseFloat(val.slice(0, -3));
      return Number.isFinite(num) ? num : null;
    }
    const num = parseFloat(val);
    return Number.isFinite(num) ? num : null;
  }

  function validateColor(raw) {
    if (typeof raw !== 'string') return null;
    raw = raw.trim();
    if (!raw) return null;
    const lower = raw.toLowerCase();
    if (['inherit', 'initial', 'unset', 'revert', 'revert-layer', 'none'].includes(lower)) {
      return null;
    }
    if (typeof CSS !== 'undefined' && typeof CSS.supports === 'function') {
      if (!CSS.supports('color', raw)) return null;
    }
    if (typeof document !== 'undefined' && document.createElement) {
      const probe = document.createElement('span');
      probe.style.color = 'rgb(1, 2, 3)';
      probe.style.color = raw;
      if (!probe.style.color) return null;
      if (probe.style.color === 'rgb(1, 2, 3)' && lower !== 'rgb(1, 2, 3)' && lower !== 'rgb(1,2,3)') {
        return null;
      }
    }
    return raw;
  }

  function parseTokens(source) {
    let getProp = null;
    if (source && typeof source.getPropertyValue === 'function') {
      getProp = (name) => source.getPropertyValue(name);
    } else if (source && source.nodeType === 1 && typeof window !== 'undefined' && window.getComputedStyle) {
      const style = window.getComputedStyle(source);
      getProp = (name) => style.getPropertyValue(name);
    } else if (source && typeof source === 'object') {
      getProp = (name) => source[name];
    } else if (typeof document !== 'undefined' && document.documentElement && typeof window !== 'undefined' && window.getComputedStyle) {
      const style = window.getComputedStyle(document.documentElement);
      getProp = (name) => style.getPropertyValue(name);
    } else {
      return { valid: false };
    }

    const raw = {
      angle: parseNumeric(getProp('--sunarc-angle')),
      bowRatio: parseNumeric(getProp('--sunarc-bow-ratio')),
      diameterRatio: parseNumeric(getProp('--sunarc-diameter-ratio')),
      cornerOvershoot: parseNumeric(getProp('--sunarc-corner-overshoot')),
      envelopeEdge: parseNumeric(getProp('--sunarc-envelope-edge')),
      mixInk: parseNumeric(getProp('--sunarc-mix-ink')),
      mixWarm: parseNumeric(getProp('--sunarc-mix-warm')),
      glowNightFloor: parseNumeric(getProp('--sunarc-glow-night-floor')),
      peakOpacity: parseNumeric(getProp('--sunarc-peak-opacity')),
      twilightMinutes: parseNumeric(getProp('--sunarc-twilight-minutes')),
      glowRadiusRatio: parseNumeric(getProp('--sunarc-glow-radius-ratio')),
      glowDayAlpha: parseNumeric(getProp('--sunarc-glow-day-alpha')),
      glowNightAlpha: parseNumeric(getProp('--sunarc-glow-night-alpha')),
      glowMidStop: parseNumeric(getProp('--sunarc-glow-mid-stop')),
      glowMidRatio: parseNumeric(getProp('--sunarc-glow-mid-ratio')),
      appearanceFlipL: parseNumeric(getProp('--sunarc-appearance-flip-l')),
      warmDark: validateColor(getProp('--sunarc-warm-dark')),
      glowColor: validateColor(getProp('--sunarc-glow-color')),
      nightGround: validateColor(getProp('--sunarc-night-ground')),
      creamBright: validateColor(getProp('--cream-bright')),
      ink: validateColor(getProp('--ink')),
    };

    return raw;
  }

  function tokensValid(tokens) {
    if (!tokens || typeof tokens !== 'object') return false;
    const requiredNumbers = [
      'angle', 'bowRatio', 'diameterRatio', 'cornerOvershoot', 'envelopeEdge',
      'mixInk', 'mixWarm', 'glowNightFloor', 'peakOpacity', 'twilightMinutes',
      'glowRadiusRatio', 'glowDayAlpha', 'glowNightAlpha', 'glowMidStop',
      'glowMidRatio', 'appearanceFlipL'
    ];
    for (const key of requiredNumbers) {
      if (typeof tokens[key] !== 'number' || !Number.isFinite(tokens[key])) {
        return false;
      }
    }
    const requiredColors = ['warmDark', 'glowColor', 'nightGround', 'creamBright', 'ink'];
    for (const key of requiredColors) {
      if (!validateColor(tokens[key])) {
        return false;
      }
    }
    return true;
  }

  function geometry(W, H, t, tokens) {
    const side = Math.min(W, H);
    const diameterRatio = tokens.diameterRatio;
    const diameter = diameterRatio * side;
    const R = diameter / 2;
    const k = tokens.cornerOvershoot;
    const delta = (k * R) / Math.SQRT2;

    const A = { x: -delta, y: -delta };
    const B = { x: W + delta, y: H + delta };
    const dx = B.x - A.x;
    const dy = B.y - A.y;
    const c = Math.hypot(dx, dy);
    const s = tokens.bowRatio * c;
    const Rarc = (c * c) / (8 * s) + s / 2;

    const M = { x: (A.x + B.x) / 2, y: (A.y + B.y) / 2 };
    const ux = dx / c;
    const uy = dy / c;
    // Perpendicular pointing toward larger y (bottom of viewport)
    let vx = -uy;
    let vy = ux;
    if (vy < 0) {
      vx = -vx;
      vy = -vy;
    }

    const O = {
      x: M.x + (Rarc - s) * vx,
      y: M.y + (Rarc - s) * vy,
    };

    const thetaA = Math.atan2(A.y - O.y, A.x - O.x);
    const thetaB = Math.atan2(B.y - O.y, B.x - O.x);
    const angleDeg = 2 * Math.asin(c / (2 * Rarc)) * (180 / Math.PI);

    const theta = thetaA + (thetaB - thetaA) * t;
    const pos = {
      x: O.x + Rarc * Math.cos(theta),
      y: O.y + Rarc * Math.sin(theta),
    };

    const thetaMid = (thetaA + thetaB) / 2;
    const center = {
      x: O.x + Rarc * Math.cos(thetaMid),
      y: O.y + Rarc * Math.sin(thetaMid),
    };

    return {
      side,
      diameter,
      R,
      A,
      B,
      c,
      s,
      Rarc,
      O,
      center,
      pos,
      angleDeg,
    };
  }

  function env(t, e) {
    if (t < 0 || t > 1) return 0;
    if (e <= 0) return 1;
    let val;
    if (t < e) {
      val = Math.sin((t / e) * (Math.PI / 2));
    } else if (t <= 1 - e) {
      val = 1;
    } else {
      val = Math.sin(((1 - t) / e) * (Math.PI / 2));
    }
    return Math.max(0, Math.min(1, val));
  }

  function nightAmount(m, rise, set, tw) {
    const dawn = rise - tw;
    const dusk = set + tw;
    if (m < dawn) return 1;
    if (m < rise) return (rise - m) / tw;
    if (m <= set) return 0;
    if (m <= dusk) return (m - set) / tw;
    return 1;
  }

  function dayProgress(m, rise, set, tw) {
    const dawn = rise - tw;
    const dusk = set + tw;
    return (m - dawn) / (dusk - dawn);
  }

  function nightProgress(m, rise, set, tw) {
    const dawn = rise - tw;
    const dusk = set + tw;
    const nightSpan = (dawn + 1440) - dusk;
    if (nightSpan <= 0) return 0;
    if (m >= dusk) {
      return (m - dusk) / nightSpan;
    }
    if (m <= dawn) {
      return ((m + 1440) - dusk) / nightSpan;
    }
    return 0;
  }

  function sunOpacity(params) {
    const { t, env: envVal, nightAmount: nightAmt, peakOpacity } = params;
    if (t < -0.02 || t > 1.02) return 0;
    return peakOpacity * envVal * (1 - nightAmt);
  }

  function glowAlpha(isDay, envVal, nightAmt, q, tokens) {
    if (isDay && nightAmt < 1) {
      return tokens.glowDayAlpha * envVal * (1 - nightAmt);
    }
    const k = q < 0.5 ? (1 - 2 * q) : (2 * q - 1);
    return tokens.glowNightFloor + (tokens.glowNightAlpha - tokens.glowNightFloor) * Math.pow(k, 1.6);
  }

  function glowPosition(isDay, sunPos, ptA, ptB, q) {
    if (isDay) return sunPos;
    return q < 0.5 ? ptB : ptA;
  }

  function degToRad(deg) { return (deg * Math.PI) / 180; }
  function radToDeg(rad) { return (rad * 180) / Math.PI; }
  function wrap(v, max) { return ((v % max) + max) % max; }

  function dayOfYear(d) {
    const start = new Date(Date.UTC(d.getUTCFullYear(), 0, 1));
    return Math.floor((d.getTime() - start.getTime()) / 86400000) + 1;
  }

  function noaaSunriseSunset(lat, lon, date, utcOffsetMinutes) {
    const d = date instanceof Date ? date : new Date(date);
    const N = dayOfYear(d);
    const lngHour = lon / 15;

    function calc(isRise) {
      const t = N + ((isRise ? 6 : 18) - lngHour) / 24;
      const M = 0.9856 * t - 3.289;
      const Mrad = degToRad(M);
      let L = M + 1.916 * Math.sin(Mrad) + 0.020 * Math.sin(2 * Mrad) + 282.634;
      L = wrap(L, 360);
      const Lrad = degToRad(L);

      let RA = radToDeg(Math.atan(0.91764 * Math.tan(Lrad)));
      RA = wrap(RA, 360);
      const Lquad = Math.floor(L / 90) * 90;
      const RAquad = Math.floor(RA / 90) * 90;
      RA = (RA + (Lquad - RAquad)) / 15;

      const sinDec = 0.39782 * Math.sin(Lrad);
      const cosDec = Math.cos(Math.asin(sinDec));

      const latRad = degToRad(lat);
      const cosH = (Math.cos(degToRad(90.833)) - sinDec * Math.sin(latRad)) / (cosDec * Math.cos(latRad));
      if (cosH > 1 || cosH < -1) return null;

      const H = isRise ? (360 - radToDeg(Math.acos(cosH))) / 15 : radToDeg(Math.acos(cosH)) / 15;
      const T = H + RA - 0.06571 * t - 6.622;
      const UT = wrap(T - lngHour, 24);
      const localMinutes = wrap(UT * 60 + utcOffsetMinutes, 1440);
      return localMinutes;
    }

    const rise = calc(true);
    const set = calc(false);
    if (rise === null || set === null) return null;
    return { rise, set };
  }

  function parseColorToRgb(colorStr) {
    if (!colorStr || typeof colorStr !== 'string') return [0, 0, 0];
    colorStr = colorStr.trim();
    if (colorStr.startsWith('#')) {
      let hex = colorStr.slice(1);
      if (hex.length === 3) {
        hex = hex.split('').map(c => c + c).join('');
      }
      const num = parseInt(hex, 16);
      return [(num >> 16) & 255, (num >> 8) & 255, num & 255];
    }
    const match = colorStr.match(/rgba?\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)/i);
    if (match) {
      return [parseFloat(match[1]), parseFloat(match[2]), parseFloat(match[3])];
    }
    if (typeof document !== 'undefined' && document.createElement) {
      const probe = document.createElement('span');
      probe.style.color = colorStr;
      document.body?.appendChild(probe);
      const computed = window.getComputedStyle ? window.getComputedStyle(probe).color : '';
      probe.remove();
      const compMatch = computed.match(/rgba?\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)/i);
      if (compMatch) {
        return [parseFloat(compMatch[1]), parseFloat(compMatch[2]), parseFloat(compMatch[3])];
      }
    }
    return [0, 0, 0];
  }

  function srgbToLinear(c) {
    c = c / 255;
    return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  }

  function linearToSrgb(c) {
    c = Math.max(0, Math.min(1, c));
    return c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055;
  }

  function rgbToOklab(r, g, b) {
    const lr = srgbToLinear(r);
    const lg = srgbToLinear(g);
    const lb = srgbToLinear(b);
    const l = 0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb;
    const m = 0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb;
    const s = 0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb;
    const l_ = Math.cbrt(l);
    const m_ = Math.cbrt(m);
    const s_ = Math.cbrt(s);
    return [
      0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
      1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
      0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    ];
  }

  function oklabToRgb(L, a, b) {
    const l_ = L + 0.3963377774 * a + 0.2158037573 * b;
    const m_ = L - 0.1055613458 * a - 0.0638541728 * b;
    const s_ = L - 0.0894841775 * a - 1.2914855480 * b;
    const l = l_ * l_ * l_;
    const m = m_ * m_ * m_;
    const s = s_ * s_ * s_;
    const lr = +4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
    const lg = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
    const lb = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;
    return [linearToSrgb(lr) * 255, linearToSrgb(lg) * 255, linearToSrgb(lb) * 255];
  }

  function rgbToHex(r, g, b) {
    const clamp = (v) => Math.max(0, Math.min(255, Math.round(v)));
    return '#' + [clamp(r), clamp(g), clamp(b)].map(x => x.toString(16).padStart(2, '0')).join('').toUpperCase();
  }

  function mixOkLab(color1, color2, t) {
    const [r1, g1, b1] = parseColorToRgb(color1);
    const [r2, g2, b2] = parseColorToRgb(color2);
    const [L1, a1, b_1] = rgbToOklab(r1, g1, b1);
    const [L2, a2, b_2] = rgbToOklab(r2, g2, b2);
    const L = L1 + (L2 - L1) * t;
    const a = a1 + (a2 - a1) * t;
    const b = b_1 + (b_2 - b_1) * t;
    const [r, g, b_out] = oklabToRgb(L, a, b);
    return rgbToHex(r, g, b_out);
  }

  function nightGroundColor(dayGround, ink, warmDark, mixInk, mixWarm) {
    const step1 = mixOkLab(dayGround, ink, mixInk);
    const step2 = mixOkLab(step1, warmDark, mixWarm);
    return step2;
  }

  function mixedGround(dayGround, nightGround, nightAmt) {
    return mixOkLab(dayGround, nightGround, nightAmt);
  }

  function oklabLightness(colorStr) {
    const [r, g, b] = parseColorToRgb(colorStr);
    const [L] = rgbToOklab(r, g, b);
    return L;
  }

  function appearanceFromLightness(L, threshold) {
    return L >= threshold ? 'light' : 'dark';
  }

  // Runtime state
  let currentInstance = null;

  function mount(root, options) {
    if (!root) return null;
    if (currentInstance) {
      currentInstance.teardown();
    }

    const opts = options || {};
    const getNow = opts.getNow || (() => new Date());
    const customSetTimeout = opts.setTimeout || setTimeout;
    const customClearTimeout = opts.clearTimeout || clearTimeout;
    const customSetInterval = opts.setInterval || setInterval;
    const customClearInterval = opts.clearInterval || clearInterval;
    const resizeDebounceMs = typeof opts.resizeDebounceMs === 'number' ? opts.resizeDebounceMs : 250;
    const perms = opts.permissions || (typeof navigator !== 'undefined' ? navigator.permissions : null);
    const geoloc = opts.geolocation || (typeof navigator !== 'undefined' ? navigator.geolocation : null);

    let glowEl = root.querySelector('.sunarc-glow');
    if (!glowEl) {
      glowEl = document.createElement('div');
      glowEl.className = 'sunarc-glow';
      glowEl.setAttribute('aria-hidden', 'true');
      root.appendChild(glowEl);
    }

    let sunEl = root.querySelector('.sunarc-sun');
    if (!sunEl) {
      sunEl = document.createElement('img');
      sunEl.className = 'sunarc-sun';
      sunEl.src = '/static/sol-status/mark.svg';
      sunEl.alt = '';
      sunEl.setAttribute('aria-hidden', 'true');
      sunEl.draggable = false;
      root.appendChild(sunEl);
    }

    let heldCoords = null;
    let lastValidSunTimes = null;
    let currentSunTimes = null;
    let geoQueried = false;
    let geoPositionAsked = false;
    let intervalId = null;
    let resizeTimer = null;

    function recompute() {
      const tokens = parseTokens(root);
      if (!tokensValid(tokens)) {
        root.style.backgroundColor = 'transparent';
        if (glowEl) glowEl.style.display = 'none';
        if (sunEl) sunEl.style.display = 'none';
        return;
      }

      if (glowEl) glowEl.style.display = '';
      if (sunEl) sunEl.style.display = '';

      const now = getNow();
      const m = now.getHours() * 60 + now.getMinutes() + now.getSeconds() / 60;
      const tw = tokens.twilightMinutes;

      if (heldCoords) {
        const offset = -now.getTimezoneOffset();
        const computed = noaaSunriseSunset(heldCoords.lat, heldCoords.lon, now, offset);
        if (computed) {
          lastValidSunTimes = computed;
        }
      }
      const sunTimes = lastValidSunTimes || { rise: 390, set: 1170 };
      currentSunTimes = sunTimes;

      const rise = sunTimes.rise;
      const set = sunTimes.set;
      const t = dayProgress(m, rise, set, tw);
      const q = nightProgress(m, rise, set, tw);
      const nightAmt = nightAmount(m, rise, set, tw);
      const envVal = env(t, tokens.envelopeEdge);

      const W = window.innerWidth || (document.documentElement ? document.documentElement.clientWidth : 1280);
      const H = window.innerHeight || (document.documentElement ? document.documentElement.clientHeight : 820);
      const geom = geometry(W, H, t, tokens);

      const isDrawn = t >= -0.02 && t <= 1.02;
      const sunOp = sunOpacity({ t, env: envVal, nightAmount: nightAmt, peakOpacity: tokens.peakOpacity });

      const isDay = isDrawn && nightAmt < 1;
      const glowAlphaVal = glowAlpha(isDay, envVal, nightAmt, q, tokens);
      const glowPosVal = glowPosition(isDay, geom.pos, geom.A, geom.B, q);

      // Night ground computed via 2-step OKLab mix
      const nightGroundHex = nightGroundColor(tokens.creamBright, tokens.ink, tokens.warmDark, tokens.mixInk, tokens.mixWarm);
      const currentGroundHex = mixedGround(tokens.creamBright, nightGroundHex, nightAmt);
      root.style.backgroundColor = currentGroundHex;

      const currentL = oklabLightness(currentGroundHex);
      const appearance = appearanceFromLightness(currentL, tokens.appearanceFlipL);
      if (typeof document !== 'undefined' && document.documentElement) {
        document.documentElement.dataset.sunarcAppearance = appearance;
      }

      // Position sun
      if (sunEl) {
        sunEl.style.width = geom.diameter + 'px';
        sunEl.style.height = geom.diameter + 'px';
        sunEl.style.left = (geom.pos.x - geom.R) + 'px';
        sunEl.style.top = (geom.pos.y - geom.R) + 'px';
        sunEl.style.opacity = isDrawn ? String(sunOp) : '0';
      }

      // Position glow
      if (glowEl) {
        const glowRadius = tokens.glowRadiusRatio * geom.R;
        const glowDiameter = 2 * glowRadius;
        glowEl.style.width = glowDiameter + 'px';
        glowEl.style.height = glowDiameter + 'px';
        glowEl.style.left = (glowPosVal.x - glowRadius) + 'px';
        glowEl.style.top = (glowPosVal.y - glowRadius) + 'px';

        const [gr, gg, gb] = parseColorToRgb(tokens.glowColor);
        const a0 = glowAlphaVal;
        const aMid = glowAlphaVal * tokens.glowMidRatio;
        const stopPct = (tokens.glowMidStop * 100).toFixed(1) + '%';
        glowEl.style.background = `radial-gradient(circle, rgba(${gr},${gg},${gb},${a0}) 0%, rgba(${gr},${gg},${gb},${aMid}) ${stopPct}, rgba(${gr},${gg},${gb},0) 100%)`;
      }
    }

    function initGeolocation() {
      if (geoQueried || !perms || typeof perms.query !== 'function') return;
      geoQueried = true;
      try {
        perms.query({ name: 'geolocation' }).then((status) => {
          if (!status) return;
          const handleStatus = () => {
            if (status.state === 'granted' && !geoPositionAsked && geoloc && typeof geoloc.getCurrentPosition === 'function') {
              geoPositionAsked = true;
              try {
                geoloc.getCurrentPosition(
                  (pos) => {
                    if (pos && pos.coords) {
                      heldCoords = {
                        lat: pos.coords.latitude,
                        lon: pos.coords.longitude,
                      };
                      recompute();
                    }
                  },
                  () => { /* keep fallback */ },
                  { timeout: 5000, enableHighAccuracy: false }
                );
              } catch (e) { /* ignore */ }
            }
          };
          handleStatus();
          if (typeof status.addEventListener === 'function') {
            status.addEventListener('change', handleStatus);
          } else {
            status.onchange = handleStatus;
          }
        }).catch(() => { /* keep fallback */ });
      } catch (e) { /* keep fallback */ }
    }

    function onResize() {
      if (resizeTimer) {
        customClearTimeout(resizeTimer);
      }
      resizeTimer = customSetTimeout(() => {
        resizeTimer = null;
        recompute();
      }, resizeDebounceMs);
    }

    function onVisibilityChange() {
      if (typeof document !== 'undefined' && document.visibilityState === 'visible') {
        recompute();
      }
    }

    // Initial paint
    recompute();
    initGeolocation();

    intervalId = customSetInterval(recompute, 60000);

    if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
      window.addEventListener('resize', onResize);
    }
    if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
      document.addEventListener('visibilitychange', onVisibilityChange);
    }

    function teardown() {
      if (intervalId) {
        customClearInterval(intervalId);
        intervalId = null;
      }
      if (resizeTimer) {
        customClearTimeout(resizeTimer);
        resizeTimer = null;
      }
      if (typeof window !== 'undefined' && typeof window.removeEventListener === 'function') {
        window.removeEventListener('resize', onResize);
      }
      if (typeof document !== 'undefined' && typeof document.removeEventListener === 'function') {
        document.removeEventListener('visibilitychange', onVisibilityChange);
      }
      if (currentInstance === instance) {
        currentInstance = null;
      }
    }

    const instance = {
      recompute,
      teardown,
      getHeldCoords: () => heldCoords,
      getSunTimes: () => currentSunTimes,
    };
    currentInstance = instance;
    return instance;
  }

  function recompute() {
    if (currentInstance) {
      currentInstance.recompute();
    }
  }

  function teardown() {
    if (currentInstance) {
      currentInstance.teardown();
    }
  }

  const SunArc = {
    parseNumeric,
    validateColor,
    parseTokens,
    tokensValid,
    geometry,
    env,
    nightAmount,
    dayProgress,
    nightProgress,
    sunOpacity,
    glowAlpha,
    glowPosition,
    noaaSunriseSunset,
    srgbToLinear,
    linearToSrgb,
    rgbToOklab,
    oklabToRgb,
    rgbToHex,
    mixOkLab,
    nightGroundColor,
    mixedGround,
    oklabLightness,
    appearanceFromLightness,
    mount,
    recompute,
    teardown,
  };

  if (typeof window !== 'undefined') {
    window.SunArc = SunArc;
  }

  // Auto-mount if #sunarc exists at script load
  if (typeof document !== 'undefined') {
    const el = document.getElementById('sunarc');
    if (el) {
      mount(el);
    }
  }
})();
