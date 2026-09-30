// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const nodeAssert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

let executedCases = 0;
let passedCases = 0;
function recordCase(assertion) {
  executedCases += 1;
  const result = assertion();
  passedCases += 1;
  return result;
}
const assert = new Proxy(nodeAssert, {
  apply(target, thisArg, args) {
    return recordCase(() => Reflect.apply(target, thisArg, args));
  },
  get(target, property) {
    const value = Reflect.get(target, property);
    if (typeof value !== 'function') return value;
    return (...args) => recordCase(() => Reflect.apply(value, target, args));
  },
});

class MockElement {
  constructor(tagName) {
    this.tagName = (tagName || 'DIV').toUpperCase();
    this.children = [];
    this.parentNode = null;
    this.nextSibling = null;
    this.attributes = {};
    this.textContent = '';
    this.className = '';
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }

  getAttribute(name) {
    return this.attributes[name] || null;
  }

  hasAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name);
  }

  appendChild(child) {
    if (child.parentNode) {
      child.parentNode.removeChild(child);
    }
    child.parentNode = this;
    const last = this.children[this.children.length - 1];
    if (last) last.nextSibling = child;
    child.nextSibling = null;
    this.children.push(child);
  }

  insertBefore(newChild, refChild) {
    if (newChild.parentNode) {
      newChild.parentNode.removeChild(newChild);
    }
    newChild.parentNode = this;
    if (!refChild) {
      this.appendChild(newChild);
      return;
    }
    const idx = this.children.indexOf(refChild);
    if (idx === -1) {
      this.appendChild(newChild);
      return;
    }
    const prev = idx > 0 ? this.children[idx - 1] : null;
    if (prev) prev.nextSibling = newChild;
    newChild.nextSibling = refChild;
    this.children.splice(idx, 0, newChild);
  }

  removeChild(child) {
    const idx = this.children.indexOf(child);
    if (idx === -1) return;
    const prev = idx > 0 ? this.children[idx - 1] : null;
    if (prev) {
      prev.nextSibling = child.nextSibling;
    }
    child.parentNode = null;
    child.nextSibling = null;
    this.children.splice(idx, 1);
  }

  get nextElementSibling() {
    return this.nextSibling;
  }

  querySelector(selector) {
    if (selector === '[data-zone-note]') {
      return this.children.find((c) => c.hasAttribute('data-zone-note')) || null;
    }
    return null;
  }
}

const crateDir = process.argv[2];
assert.ok(crateDir, 'crate directory argument is required');
const isReverse = process.argv.includes('reverse');

const copySrc = fs.readFileSync(
  path.join(crateDir, 'assets', 'static', 'convey_copy.js'),
  'utf8'
);
const clockSrc = fs.readFileSync(
  path.join(crateDir, 'assets', 'static', 'journal_clock.js'),
  'utf8'
);

function createBaseContext(customIntl) {
  const windowObj = {
    document: {
      createElement: (tag) => new MockElement(tag),
      addEventListener: () => {},
      removeEventListener: () => {},
    },
    addEventListener: () => {},
    removeEventListener: () => {},
  };
  const context = {
    window: windowObj,
    document: windowObj.document,
    Intl: customIntl || Intl,
    Date,
    console,
  };
  vm.createContext(context);
  vm.runInContext(copySrc, context, { filename: 'convey_copy.js' });
  vm.runInContext(clockSrc, context, { filename: 'journal_clock.js' });
  return context;
}

if (!isReverse) {
  // Denver-TZ child cases (TZ = America/Denver)
  const fixedInstant = Date.parse('2026-09-30T18:00:00.000Z');

  // 1. No seed. today() is Denver's day (20260930). parts hour 12. differs() is false.
  {
    const ctx = createBaseContext();
    const Clock = ctx.window.JournalClock;
    Clock.setNow(() => fixedInstant);

    assert.strictEqual(Clock.today(), '20260930');
    const p = Clock.parts(Clock.now());
    assert.strictEqual(p.hour, '12');
    assert.strictEqual(Clock.differs(), false);

    const parent = new MockElement('div');
    const anchor = new MockElement('span');
    parent.appendChild(anchor);
    Clock.placeNote(anchor, { inside: false });
    assert.strictEqual(anchor.nextSibling, null);
  }

  // 2. Seed {tz:'Asia/Tokyo', label:'Tokyo'}.
  {
    const ctx = createBaseContext();
    const Clock = ctx.window.JournalClock;
    Clock.setNow(() => fixedInstant);
    Clock.seed({ tz: 'Asia/Tokyo', label: 'Tokyo' });

    assert.strictEqual(Clock.today(), '20261001');
    assert.strictEqual(Clock.dayStart('20261001'), Date.parse('2026-09-30T15:00:00.000Z'));
    const p = Clock.parts(Clock.now());
    assert.strictEqual(p.month, '10');
    assert.strictEqual(p.day, '01');
    assert.strictEqual(p.hour, '03');
    assert.strictEqual(Clock.differs(), true);

    const anchor = new MockElement('span');
    Clock.placeNote(anchor, { inside: true });
    const note = anchor.querySelector('[data-zone-note]');
    assert.ok(note, 'creates one [data-zone-note] inside the anchor');
  }

  // 3. Seed {tz:'America/Denver', label:'Denver'}.
  {
    const ctx = createBaseContext();
    const Clock = ctx.window.JournalClock;
    Clock.setNow(() => fixedInstant);
    Clock.seed({ tz: 'Asia/Tokyo', label: 'Tokyo' });

    const parent = new MockElement('div');
    const anchor = new MockElement('span');
    parent.appendChild(anchor);
    Clock.placeNote(anchor, { inside: false });
    assert.ok(anchor.nextSibling && anchor.nextSibling.hasAttribute('data-zone-note'));

    Clock.seed({ tz: 'America/Denver', label: 'Denver' });
    assert.strictEqual(Clock.differs(), false);
    Clock.placeNote(anchor, { inside: false });
    assert.strictEqual(anchor.nextSibling, null);
  }

  // 4. Phoenix July differs true. Phoenix January differs false.
  {
    const ctx = createBaseContext();
    const Clock = ctx.window.JournalClock;
    Clock.seed({ tz: 'America/Phoenix', label: 'Phoenix' });

    Clock.setNow(() => Date.parse('2026-07-15T18:00:00.000Z'));
    assert.strictEqual(Clock.differs(), true);

    Clock.setNow(() => Date.parse('2026-01-15T18:00:00.000Z'));
    assert.strictEqual(Clock.differs(), false);
  }

  // 5. Denver spring-forward & fall-back, Santiago day start & hour 01
  {
    const ctx = createBaseContext();
    const Clock = ctx.window.JournalClock;
    Clock.seed({ tz: 'America/Denver', label: 'Denver' });

    assert.strictEqual(Clock.dayStart('20260308'), Date.parse('2026-03-08T07:00:00.000Z'));
    const pSpring = Clock.parts(new Date('2026-03-08T09:00:00.000Z'));
    assert.strictEqual(pSpring.hour, '03');

    assert.strictEqual(Clock.dayStart('20261101'), Date.parse('2026-11-01T06:00:00.000Z'));

    Clock.seed({ tz: 'America/Santiago', label: 'Santiago' });
    const santiagoStart = Date.parse('2024-09-08T04:00:00.000Z');
    assert.strictEqual(Clock.dayStart('20240908'), santiagoStart);
    const pSantiago = Clock.parts(new Date(santiagoStart));
    assert.strictEqual(pSantiago.hour, '01');
  }

  // 6. Stub Intl.DateTimeFormat RangeError on Asia/Tokyo
  {
    const OrigDTF = Intl.DateTimeFormat;
    function CustomDTF(locales, options) {
      if (options && options.timeZone === 'Asia/Tokyo') {
        throw new RangeError('Invalid time zone specified: Asia/Tokyo');
      }
      return new OrigDTF(locales, options);
    }
    CustomDTF.supportedLocalesOf = OrigDTF.supportedLocalesOf;
    CustomDTF.prototype = OrigDTF.prototype;

    const customIntl = Object.create(Intl, {
      DateTimeFormat: { value: CustomDTF },
    });

    const ctx = createBaseContext(customIntl);
    let logErrorCount = 0;
    ctx.window.logError = () => {
      logErrorCount += 1;
    };

    const Clock = ctx.window.JournalClock;
    Clock.seed({ tz: 'Asia/Tokyo', label: 'Tokyo' });
    assert.strictEqual(Clock.zone(), null);
    assert.strictEqual(Clock.differs(), false);

    for (let i = 0; i < 50; i++) {
      Clock.formatInstant(fixedInstant);
    }
    assert.strictEqual(logErrorCount, 1);
  }

  // 7. span
  {
    const ctx = createBaseContext();
    const Clock = ctx.window.JournalClock;
    Clock.seed({ tz: 'Asia/Tokyo', label: 'Tokyo' });

    const spanTokyo = Clock.span('20261001', 0, 1440, null);
    assert.strictEqual(spanTokyo.fromMs, Clock.dayStart('20261001'));
    assert.strictEqual(spanTokyo.toMs, Clock.dayStart('20261002'));

    const spanTranscripts = Clock.span('20260930', 0, 1440, -21600);
    assert.strictEqual(spanTranscripts.fromMs, Date.parse('2026-09-30T06:00:00.000Z'));
    assert.strictEqual(spanTranscripts.toMs, Date.parse('2026-10-01T06:00:00.000Z'));
  }
} else {
  // Tokyo-TZ child (reverse)
  const ctx = createBaseContext();
  const Clock = ctx.window.JournalClock;
  Clock.seed({ tz: 'America/Denver', label: 'Denver' });
  Clock.setNow(() => Date.parse('2026-10-20T18:00:00.000Z'));

  const dateFormatSrc = fs.readFileSync(
    path.join(crateDir, 'assets', 'static', 'date_format.js'),
    'utf8'
  );
  const dateNavSrc = fs.readFileSync(
    path.join(crateDir, 'assets', 'static', 'date-nav.js'),
    'utf8'
  );
  const networkSrc = fs.readFileSync(
    path.join(crateDir, 'assets', 'network', 'network.js'),
    'utf8'
  );

  vm.runInContext(dateFormatSrc, ctx, { filename: 'date_format.js' });
  vm.runInContext(dateNavSrc, ctx, { filename: 'date-nav.js' });
  vm.runInContext(networkSrc, ctx, { filename: 'network.js' });

  const Format = ctx.window.JournalFormat;
  const DateNav = ctx.window.DateNav;
  const NetworkRender = ctx.window.NetworkRender;

  const shortStr = Format.day('20260930');
  const fullStr = Format.dayFull('20260930');
  assert.ok(shortStr.includes('30'), `formatDateShort must contain 30: ${shortStr}`);
  assert.ok(!shortStr.includes('29'), `formatDateShort must not contain 29: ${shortStr}`);
  assert.ok(fullStr.includes('30'), `formatDateFull must contain 30: ${fullStr}`);
  assert.ok(!fullStr.includes('29'), `formatDateFull must not contain 29: ${fullStr}`);

  const segTime = Format.segmentTime('143000');
  assert.ok(segTime.includes('14:30'), `segmentTime must contain 14:30: ${segTime}`);

  const segDisplay = NetworkRender.segmentDisplay({ day: '20260930', name: '143000' });
  assert.ok(segDisplay && segDisplay.readable.includes('14:30'), `segmentDisplay must contain 14:30: ${segDisplay?.readable}`);

  const heading = DateNav.headingLabel('20260930');
  assert.ok(heading.includes('30'), `headingLabel must contain 30: ${heading}`);
  assert.ok(!heading.includes('29'), `headingLabel must not contain 29: ${heading}`);
}

console.log(`CLOCK CASES: ${passedCases} passed`);
