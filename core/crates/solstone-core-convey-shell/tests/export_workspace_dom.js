// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { journalContext } = require('./journal_catalog_dom.js');

const hostJs = fs.readFileSync(path.join(__dirname, '../assets/static/journal-web-host.js'), 'utf8');
const exportJs = fs.readFileSync(path.join(__dirname, '../assets/export/export.js'), 'utf8');

function matches(node, sel) {
  if (sel.startsWith('#')) return node.attrs.id === sel.slice(1);
  if (sel.startsWith('.')) return node.className.split(/\s+/).includes(sel.slice(1));
  if (sel.startsWith('[')) {
    const eq = sel.match(/^\[([^\]]+)="([^"]*)"\]$/);
    if (eq) return node.attrs[eq[1]] === eq[2];
    const has = sel.match(/^\[([^\]]+)\]$/);
    return Boolean(has && Object.prototype.hasOwnProperty.call(node.attrs, has[1]));
  }
  return node.tagName === sel.toUpperCase();
}

function walk(node, out) {
  node.children.forEach((child) => {
    out.push(child);
    walk(child, out);
  });
  return out;
}

function makeNode(tag) {
  const node = {
    tagName: String(tag).toUpperCase(),
    className: '',
    children: [],
    attrs: {},
    listeners: {},
    style: {},
    text: '',
    parent: null,
    checked: false,
    value: '',
  };
  Object.defineProperty(node, 'textContent', {
    get() {
      return node.text + node.children.map((c) => c.textContent).join('');
    },
    set(v) {
      node.text = v == null ? '' : String(v);
      node.children.forEach((c) => { c.parent = null; });
      node.children = [];
    },
  });
  node.setAttribute = (k, v) => { node.attrs[k] = String(v); };
  node.removeAttribute = (k) => { delete node.attrs[k]; };
  node.getAttribute = (k) => (Object.prototype.hasOwnProperty.call(node.attrs, k) ? node.attrs[k] : null);
  node.hasAttribute = (k) => Object.prototype.hasOwnProperty.call(node.attrs, k);
  node.appendChild = (child) => {
    if (child.parent) child.parent.children = child.parent.children.filter((c) => c !== child);
    child.parent = node;
    node.children.push(child);
    return child;
  };
  node.addEventListener = (type, fn) => { (node.listeners[type] = node.listeners[type] || []).push(fn); };
  node.fire = (type, ev) => (node.listeners[type] || []).forEach((fn) => fn(ev || { preventDefault() {} }));
  node.click = () => node.fire('click');
  node.focus = () => {};
  node.querySelectorAll = (sel) => walk(node, []).filter((c) => matches(c, sel));
  node.querySelector = (sel) => node.querySelectorAll(sel)[0] || null;
  return node;
}

function createHarness(opts) {
  const root = makeNode('main');
  root.setAttribute('id', 'export-root');
  const winListeners = {};
  const timers = [];
  const doc = {
    activeElement: null,
    createElement: (tag) => makeNode(tag),
    getElementById: (id) => (id === 'export-root' ? root : null),
  };
  const clipboardWrites = [];
  let registeredView = null;
  const sandbox = journalContext({
    document: doc,
    navigator: {
      clipboard: {
        writeText(text) {
          clipboardWrites.push(text);
          return Promise.resolve();
        },
      },
    },
    setTimeout(fn, ms) {
      const id = timers.length + 1;
      timers.push({ id, fn, ms, cleared: false });
      return id;
    },
    clearTimeout(id) {
      const t = timers.find((x) => x.id === id);
      if (t) t.cleared = true;
    },
    addEventListener(type, fn) {
      (winListeners[type] = winListeners[type] || []).push(fn);
    },
    removeEventListener(type, fn) {
      winListeners[type] = (winListeners[type] || []).filter((f) => f !== fn);
    },
    SolViews: {
      register(name, view) {
        if (name === 'export') registeredView = view;
      },
    },
  });
  sandbox.window = sandbox;
  sandbox.top = sandbox;
  vm.runInContext(hostJs, sandbox, { filename: 'journal-web-host.js' });
  if (opts.olderPairedHost) {
    sandbox.solstoneJournalHost = { version: 2, types: ['import.pick'], send() {} };
  }
  if (opts.nativeHost) {
    sandbox.solstoneJournalHost = opts.nativeHost;
  }
  vm.runInContext(exportJs, sandbox, { filename: 'export.js' });
  assert.ok(registeredView, 'export view registered');

  const disposers = [];
  let readyState = null;
  const ctx = {
    disposed: false,
    fetch: opts.fetch,
    now: () => opts.nowMs || Date.parse('2026-11-26T17:15:00Z'),
    ready(kind) { readyState = kind; },
    on(target, type, fn) {
      target.addEventListener(type, fn);
      disposers.push(() => target.removeEventListener(type, fn));
    },
    onDispose(fn) { disposers.push(fn); },
  };
  registeredView.mount(ctx);

  return {
    root,
    sandbox,
    clipboardWrites,
    timers,
    getReady: () => readyState,
    emitNative(detail) {
      const evName = sandbox.solstoneExportContract.event;
      (winListeners[evName] || []).forEach((fn) => fn({ detail }));
    },
    dispose() {
      ctx.disposed = true;
      disposers.forEach((fn) => fn());
    },
  };
}

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

function jsonResp(status, body) {
  return Promise.resolve({
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  });
}

function sampleEstimate(overrides) {
  return Object.assign({
    status: 'ready',
    is_estimate: true,
    measured_at: '2026-11-26T17:15:00Z',
    days_counted: 642,
    progress: { segments_scanned: 10, segments_total: 10, full_bytes_so_far: 298400000000, light_bytes_so_far: 18600000000 },
    breakdown: {
      full_bytes: 298400000000,
      full_files: 108400,
      light_bytes: 18600000000,
      light_files: 6259,
      processed_media_omitted: { bytes: 279800000000, files: 4812 },
      unprocessed_or_failed_media: { bytes: 1400000000, files: 19 },
      kept_non_media: { bytes: 17200000000, files: 6240 },
      already_offloaded: { days: 0, files: 0, bytes: 0, degraded: false },
      measurement_degraded: false,
    },
    journal_disk_free_bytes: 412000000000,
    journal_disk_total_bytes: 1000000000000,
    full_required_bytes: 313320000000,
    light_required_bytes: 19673741824,
    full_fits: true,
    light_fits: true,
    backup: { state: 'healthy', enabled: true, paused_at: null },
    journal_host_os: 'macos',
    cli_command_full: 'solstone journal archive export --out /Volumes/MyDrive/solstone-journal.zip',
  }, overrides || {});
}

let passed = 0;

async function run(name, fn) {
  await fn();
  passed += 1;
}

(async () => {
  // 1. Measuring -> ready -> mode switch + 4-population breakdown
  await run('measuring and mode switch', async () => {
    let est = sampleEstimate({
      status: 'measuring',
      breakdown: null,
      progress: { segments_scanned: 5, segments_total: 10, full_bytes_so_far: 142800000000, light_bytes_so_far: 9100000000 },
    });
    const h = createHarness({
      fetch: (url) => url.includes('/estimate') ? jsonResp(200, est) : jsonResp(200, { status: 'idle', other_requester_busy: false, active_job: null, ready: null }),
    });
    await flush();
    assert.strictEqual(h.getReady(), 'content');
    assert.ok(h.root.textContent.includes('measuring your journal'));
    assert.ok(h.root.querySelector('#exportPrepareBtn').hasAttribute('disabled'));

    est = sampleEstimate();
    h.timers[0].fn();
    await flush();
    assert.ok(!h.root.querySelector('#exportPrepareBtn').hasAttribute('disabled'));
    assert.ok(h.root.querySelector('#exportPrepareBtn').textContent.includes('prepare full copy'));

    const lightRadio = h.root.querySelector('[value="without_processed_media"]');
    lightRadio.checked = true;
    lightRadio.fire('change');
    assert.ok(h.root.querySelector('#exportPrepareBtn').textContent.includes('prepare copy without processed media'));
    assert.ok(h.root.querySelector('[data-export-fold="breakdown"]').hasAttribute('open'));
    h.dispose();
  });

  // 2. Offload disclosures: healthy, paused, unreachable, degraded
  await run('offload disclosures', async () => {
    for (const [backupState, off, expectedNotice, expectedSnippet] of [
      ['healthy', { days: 42, files: 100, bytes: 118400000000, degraded: false }, 'offload-healthy', 'restore days from backup first'],
      ['paused', { days: 42, files: 100, bytes: 118400000000, degraded: false }, 'offload-paused', 'paused backup'],
      ['unreachable', { days: 42, files: 100, bytes: 118400000000, degraded: false }, 'offload-unreachable', 'unreachable backup'],
      ['healthy', { days: 0, files: 0, bytes: 0, degraded: true }, 'offload-degraded', "couldn't read the full offload record"],
    ]) {
      const est = sampleEstimate({
        backup: { state: backupState, enabled: true, paused_at: backupState === 'paused' ? 1 : null },
        breakdown: Object.assign({}, sampleEstimate().breakdown, { already_offloaded: off }),
      });
      const h = createHarness({
        fetch: (url) => url.includes('/estimate') ? jsonResp(200, est) : jsonResp(200, { status: 'idle', other_requester_busy: false, active_job: null, ready: null }),
      });
      await flush();
      const notice = h.root.querySelector(`[data-export-notice="${expectedNotice}"]`);
      assert.ok(notice, `missing ${expectedNotice}`);
      assert.ok(notice.textContent.includes(expectedSnippet), notice.textContent);
      h.dispose();
    }
  });

  // 3. Disk space preflight: tight (macOS admin terminal vs Linux terminal) & switch-to-light + both-tight
  await run('disk space preflight', async () => {
    const tightMac = sampleEstimate({ journal_disk_free_bytes: 84200000000, full_fits: false, light_fits: true, journal_host_os: 'macos' });
    const h = createHarness({
      fetch: (url) => url.includes('/estimate') ? jsonResp(200, tightMac) : jsonResp(200, { status: 'idle', other_requester_busy: false, active_job: null, ready: null }),
    });
    await flush();
    assert.ok(h.root.querySelector('[data-export-space="full-tight"]'));
    assert.ok(h.root.textContent.includes('admin terminal'));
    assert.ok(h.root.querySelector('#exportPrepareBtn').hasAttribute('disabled'));
    h.root.querySelector('#exportSwitchLightBtn').click();
    assert.ok(!h.root.querySelector('#exportPrepareBtn').hasAttribute('disabled'));
    h.dispose();

    const bothLinux = sampleEstimate({ journal_disk_free_bytes: 6100000000, full_fits: false, light_fits: false, journal_host_os: 'linux' });
    const h2 = createHarness({
      fetch: (url) => url.includes('/estimate') ? jsonResp(200, bothLinux) : jsonResp(200, { status: 'idle', other_requester_busy: false, active_job: null, ready: null }),
    });
    await flush();
    assert.ok(h2.root.querySelector('[data-export-space="both-tight"]'));
    assert.ok(h2.root.textContent.includes("terminal on your journal's computer"));
    assert.ok(h2.root.querySelector('#exportPrepareBtn').hasAttribute('disabled'));
    h2.dispose();
  });

  // 4. Prepare -> cancel + ready card in browser + inline discard + 409 conflict
  await run('ready browser download and discard', async () => {
    let state = {
      status: 'ready',
      other_requester_busy: false,
      active_job: null,
      ready: {
        export_id: 'exp_abc',
        mode: 'full',
        filename: 'solstone-journal-2026-11-26.zip',
        size_bytes: 296100000000,
        sha256: 'deadbeef'.repeat(8),
        etag: '"' + 'deadbeef'.repeat(8) + '"',
        created_at: '2026-11-26T17:14:00Z',
        contents_changed_since: true,
        days_count: 642,
        files_count: 108400,
        download_path: '/app/export/api/ready/exp_abc/download',
        legacy_download_path: '/app/import/api/journal-archive/export?id=exp_abc',
      },
    };
    let deleteStatus = 409;
    const h = createHarness({
      fetch: (url, init) => {
        if (init && init.method === 'DELETE') {
          if (deleteStatus === 200) state = { status: 'idle', other_requester_busy: false, active_job: null, ready: null };
          return jsonResp(deleteStatus, {});
        }
        return url.includes('/estimate') ? jsonResp(200, sampleEstimate()) : jsonResp(200, state);
      },
    });
    await flush();
    assert.ok(h.root.querySelector('[data-export-changed="1"]'));
    const dl = h.root.querySelector('#exportPrimaryDownloadLink');
    assert.strictEqual(dl.getAttribute('href'), '/app/export/api/ready/exp_abc/download');
    dl.click();
    assert.ok(h.root.querySelector('[data-export-browser-started="1"]'));
    assert.ok(h.root.textContent.includes('download started'));

    h.root.querySelector('#exportCopyShaBtn').click();
    assert.strictEqual(h.clipboardWrites[0], 'deadbeef'.repeat(8));

    h.root.querySelector('#exportDiscardOpenBtn').click();
    assert.ok(h.root.querySelector('#exportDiscardConfirm'));
    h.root.querySelector('#exportDiscardConfirmBtn').click();
    await flush();
    assert.ok(h.root.textContent.includes('download is currently in progress'));

    deleteStatus = 200;
    h.root.querySelector('#exportDiscardOpenBtn').click();
    h.root.querySelector('#exportDiscardConfirmBtn').click();
    await flush();
    await flush();
    assert.ok(h.root.querySelector('[data-export-discarded="1"]'));
    h.dispose();
  });

  // 5. Older paired host uses legacy ?id= download path
  await run('older paired host fallback', async () => {
    const state = {
      status: 'ready',
      other_requester_busy: false,
      active_job: null,
      ready: {
        export_id: 'exp_old',
        mode: 'full',
        filename: 'solstone-journal-2026-11-26.zip',
        size_bytes: 1024,
        sha256: 'a'.repeat(64),
        etag: '"' + 'a'.repeat(64) + '"',
        created_at: '2026-11-26T17:14:00Z',
        download_path: '/app/export/api/ready/exp_old/download',
        legacy_download_path: '/app/import/api/journal-archive/export?id=exp_old',
      },
    };
    const h = createHarness({
      olderPairedHost: true,
      fetch: (url) => url.includes('/estimate') ? jsonResp(200, sampleEstimate()) : jsonResp(200, state),
    });
    await flush();
    const dl = h.root.querySelector('#exportPrimaryDownloadLink');
    assert.strictEqual(dl.getAttribute('href'), '/app/import/api/journal-archive/export?id=exp_old');
    h.dispose();
  });

  // 6. Native v2 host lifecycle: device space preflight, start, transferring, paused(relay), saved + reveal, handed_off, native errors
  await run('native v2 host lifecycle', async () => {
    const sent = [];
    const nativeHost = {
      version: 2,
      types: ['export.start', 'export.status', 'export.pause', 'export.resume', 'export.cancel', 'export.reveal', 'export.device_space'],
      send(msg) {
        sent.push(msg);
        if (msg.type === 'export.device_space') {
          return { free_bytes: 48200000000, platform: 'ios' };
        }
        return { phase: 'idle' };
      },
    };
    const readyObj = {
      export_id: 'exp_nat',
      mode: 'full',
      filename: 'solstone-journal-2026-11-26.zip',
      size_bytes: 296100000000,
      sha256: 'b'.repeat(64),
      etag: '"' + 'b'.repeat(64) + '"',
      created_at: '2026-11-26T17:14:00Z',
      download_path: '/app/export/api/ready/exp_nat/download',
      legacy_download_path: '/app/import/api/journal-archive/export?id=exp_nat',
    };
    let state = { status: 'idle', other_requester_busy: false, active_job: null, ready: null };
    const h = createHarness({
      nativeHost,
      fetch: (url) => url.includes('/estimate') ? jsonResp(200, sampleEstimate()) : jsonResp(200, state),
    });
    await flush();
    assert.ok(h.root.querySelector('[data-export-device-space="tight"]'), 'mobile space warning rendered');

    state = { status: 'ready', other_requester_busy: false, active_job: null, ready: readyObj };
    const h2 = createHarness({
      nativeHost,
      fetch: (url) => url.includes('/estimate') ? jsonResp(200, sampleEstimate()) : jsonResp(200, state),
    });
    await flush();
    const saveBtn = h2.root.querySelector('#exportPrimarySaveBtn');
    assert.ok(saveBtn && saveBtn.textContent.includes('save copy…'));
    saveBtn.click();
    const startMsg = sent.find((m) => m.type === 'export.start');
    assert.ok(startMsg && startMsg.export_id === 'exp_nat' && startMsg.sha256 === 'b'.repeat(64));

    h2.emitNative({
      phase: 'transferring',
      export_id: 'exp_nat',
      platform: 'macos',
      bytes_written: 142800000000,
      total_bytes: 296100000000,
      rate_bytes_per_sec: 38400000,
      transport: 'local',
      destination_display: '/Volumes/Backup/solstone-journal-2026-11-26.zip',
    });
    assert.ok(h2.root.textContent.includes('saving to this mac'));
    assert.ok(h2.root.textContent.includes('over local network'));

    h2.emitNative({
      phase: 'paused',
      export_id: 'exp_nat',
      platform: 'macos',
      bytes_written: 142800000000,
      total_bytes: 296100000000,
      pause_reason: 'relay_switched',
    });
    assert.ok(h2.root.textContent.includes('transfer paused'));
    assert.ok(h2.root.textContent.includes('switched to relay'));

    h2.emitNative({
      phase: 'saved',
      export_id: 'exp_nat',
      platform: 'macos',
      bytes_written: 296100000000,
      total_bytes: 296100000000,
      destination_display: '/Volumes/Backup/solstone-journal-2026-11-26.zip',
      can_reveal: true,
    });
    assert.ok(h2.root.querySelector('[data-export-native-state="saved"]'));
    assert.ok(h2.root.textContent.includes('show in Finder'));
    assert.ok(h2.root.querySelector('[data-export-cleanup-prompt="1"]'));

    h2.emitNative({
      phase: 'handed_off',
      export_id: 'exp_nat',
      platform: 'ios_host',
    });
    assert.ok(h2.root.querySelector('[data-export-native-state="handed_off"]'));
    assert.ok(h2.root.textContent.includes('copy handed to Files'));

    h2.emitNative({
      phase: 'error',
      export_id: 'exp_nat',
      platform: 'macos',
      error_code: 'checksum_mismatch',
    });
    assert.ok(h2.root.querySelector('[data-export-error="checksum_mismatch"]'));
    h.dispose();
    h2.dispose();
  });

  // 7. Server terminal errors: source_changed, expired, erased_invalidated
  await run('server error cards', async () => {
    for (const [status, code, snippet] of [
      ['failed', 'source_changed', 'your journal changed while packing'],
      ['expired', 'export_expired', 'prepared copy expired'],
      ['erased_invalidated', 'export_invalidated_by_erasure', 'trash erasure'],
    ]) {
      const state = { status, other_requester_busy: false, active_job: { error_code: code }, ready: null };
      const h = createHarness({
        fetch: (url) => url.includes('/estimate') ? jsonResp(200, sampleEstimate()) : jsonResp(200, state),
      });
      await flush();
      assert.ok(h.root.querySelector(`[data-export-error="${code}"]`), code);
      assert.ok(h.root.textContent.includes(snippet), h.root.textContent);
      h.dispose();
    }
  });

  console.log(`DOM CASES: ${passed} passed`);
})().catch((err) => {
  console.error(err);
  process.exit(1);
});
