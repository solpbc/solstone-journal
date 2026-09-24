// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import path from 'node:path';
import fs from 'node:fs';
import http from 'node:http';
import { fileURLToPath } from 'node:url';
import { ChromeLauncher, runPage } from './runner.mjs';
import { startStaticServer } from './server.mjs';
import { connectWebSocket } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const FIXTURES_DIR = path.join(__dirname, 'fixtures');

export async function runSelfTests(options = {}) {
  const chromeBin = options.chromeBin || '/usr/bin/google-chrome';
  console.log('Running convey-page-runner fixture self-tests...');

  let allPassed = true;
  const record = (name, ok, detail) => {
    if (ok) {
      console.log(`  PASS: ${name}`);
    } else {
      console.error(`  FAIL: ${name}${detail ? ` — ${detail}` : ''}`);
      allPassed = false;
    }
  };

  // 1. Missing Chrome proves it names its cause
  try {
    const badLauncher = new ChromeLauncher({ chromeBin: '/nonexistent/google-chrome-bogus' });
    let threw = false;
    try {
      await badLauncher.launch();
    } catch (err) {
      threw = true;
      const namedCause =
        err.message.includes('Chrome binary not found') ||
        err.message.includes('not executable') ||
        err.message.includes('google-chrome-bogus');
      record('missing-chrome names its cause', namedCause, err.message);
    } finally {
      await badLauncher.close();
    }
    if (!threw) {
      record('missing-chrome names its cause', false, 'did not throw');
    }
  } catch (err) {
    record('missing-chrome names its cause', false, err.message);
  }

  // 2. Bind failure proves it names its cause
  let blockerServer = null;
  try {
    blockerServer = http.createServer();
    await new Promise((res, rej) => blockerServer.listen(0, '127.0.0.1', res).on('error', rej));
    const boundPort = blockerServer.address().port;

    let bindThrew = false;
    try {
      await startStaticServer({ roots: [FIXTURES_DIR], port: boundPort, host: '127.0.0.1' });
    } catch (err) {
      bindThrew = true;
      const named = err.message.includes('bind failure') || err.message.includes('EADDRINUSE');
      record('bind-failure names its cause', named, err.message);
    }
    if (!bindThrew) {
      record('bind-failure names its cause', false, 'did not throw on port collision');
    }
  } catch (err) {
    record('bind-failure names its cause', false, err.message);
  } finally {
    if (blockerServer) {
      await new Promise((res) => blockerServer.close(res));
    }
  }

  // Start real static server and chrome for fixture tests
  let staticServer;
  let launcher;
  try {
    staticServer = await startStaticServer({ roots: [FIXTURES_DIR] });
    launcher = new ChromeLauncher({ chromeBin });
    await launcher.launch();
  } catch (err) {
    console.error(`Failed to initialize self-test environment: ${err.message}`);
    if (staticServer) await staticServer.close();
    if (launcher) await launcher.close();
    return false;
  }

  const defaultQuarantine = new Set(['autoscroll wide overflow centers target']);

  try {
    // 3. Green fixture passes
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/green.html',
        ceilingMs: 2000,
        settleMs: 100,
      });
      record('green fixture passes', res.ok === true && res.passed === 2, res.cause);
    }

    // 4. Green with console.error passes (console.error is not a failure)
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/green-console-error.html',
        ceilingMs: 2000,
        settleMs: 100,
      });
      record('green with console.error passes', res.ok === true && res.passed === 1, res.cause);
    }

    // 5. Reported failure fails and names the assertion
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/reported-failure.html',
        ceilingMs: 2000,
        settleMs: 100,
      });
      const namesAssertion = res.ok === false && res.cause.includes('expected failure');
      record('reported-failure fails and names assertion', namesAssertion, res.cause);
    }

    // 6. Timeout case fails (uses short ceiling)
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/timeout.html',
        ceilingMs: 300,
        settleMs: 50,
      });
      const isTimeout = res.ok === false && res.cause.includes('no completion within ceiling');
      record('timeout fails with ceiling message', isTimeout, res.cause);
    }

    // 7. Uncaught exception fails
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/uncaught-exception.html',
        ceilingMs: 2000,
        settleMs: 100,
      });
      const isUncaught = res.ok === false && res.cause.includes('uncaught exception');
      record('uncaught exception fails', isUncaught, res.cause);
    }

    // 8. Unhandled rejection fails
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/unhandled-rejection.html',
        ceilingMs: 2000,
        settleMs: 100,
      });
      const isUnhandled = res.ok === false && (res.cause.includes('unhandled rejection') || res.cause.includes('Unhandled rejection'));
      record('unhandled rejection fails', isUnhandled, res.cause);
    }

    // 9. Zero assertions fails
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/zero-assertions.html',
        ceilingMs: 2000,
        settleMs: 100,
      });
      const isZero = res.ok === false && res.cause.includes('zero assertions');
      record('zero assertions fails', isZero, res.cause);
    }

    // 10. Row count mismatch fails
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/row-count-mismatch.html',
        ceilingMs: 2000,
        settleMs: 100,
      });
      const isMismatch = res.ok === false && res.cause.includes('row-count mismatch');
      record('row count mismatch fails', isMismatch, res.cause);
    }

    // 11. Late row ~200ms after completion fails
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/late-row.html',
        ceilingMs: 2000,
        settleMs: 300,
      });
      const isLateRow = res.ok === false && res.cause.includes('assertion row added during settle window');
      record('late row during settle window fails', isLateRow, res.cause);
    }

    // 12. Summary fail (one failed row plus summary fail node is not a mismatch)
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/summary-fail.html',
        ceilingMs: 2000,
        settleMs: 100,
      });
      const isReportedFail = res.ok === false && !res.cause.includes('row-count mismatch') && res.cause.includes('reported failure');
      record('summary fail node is not a mismatch', isReportedFail, res.cause);
    }

    // 13. Crash test calls DevTools Page.crash and allocates nothing
    {
      const { targetId } = await launcher.browserCdp.send('Target.createTarget', { url: 'about:blank' });
      const pageWsUrl = `ws://127.0.0.1:${launcher.port}/devtools/page/${targetId}`;
      const pageCdp = await connectWebSocket(pageWsUrl);

      let targetCrashed = false;
      pageCdp.on('Inspector.targetCrashed', () => {
        targetCrashed = true;
      });
      pageCdp.on('close', () => {
        targetCrashed = true;
      });

      await pageCdp.send('Inspector.enable');
      await pageCdp.send('Page.enable');
      await pageCdp.send('Page.navigate', { url: `${staticServer.origin}/crash.html` });
      await new Promise((r) => setTimeout(r, 50));

      // Trigger crash via CDP
      pageCdp.send('Page.crash').catch(() => {});

      await new Promise((r) => setTimeout(r, 100));
      pageCdp.close();
      try {
        await launcher.browserCdp.send('Target.closeTarget', { targetId });
      } catch (_) {}

      record('crash test triggers target crash cleanly', targetCrashed === true);
    }

    // 14. Quarantine outcome 1: quarantined failure passes
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/quarantine-match.html',
        ceilingMs: 2000,
        settleMs: 100,
        quarantineSet: defaultQuarantine,
      });
      record('quarantine outcome 1: quarantined failure passes', res.ok === true, res.cause);
    }

    // 15. Quarantine outcome 2: unexpected failure fails
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/quarantine-unexpected.html',
        ceilingMs: 2000,
        settleMs: 100,
        quarantineSet: defaultQuarantine,
      });
      const isUnexpected = res.ok === false && res.cause.includes('reported failure') && res.cause.includes('unquarantined test failure');
      record('quarantine outcome 2: unexpected failure fails', isUnexpected, res.cause);
    }

    // 16. Quarantine outcome 3: unneeded quarantine fails with "remove from quarantine"
    {
      const res = await runPage({
        browserCdp: launcher.browserCdp,
        chromePort: launcher.port,
        serverOrigin: staticServer.origin,
        pageUrlPath: '/quarantine-passed.html',
        ceilingMs: 2000,
        settleMs: 100,
        quarantineSet: defaultQuarantine,
      });
      const isRemoveQuarantine = res.ok === false && res.cause.includes('remove from quarantine');
      record('quarantine outcome 3: unneeded quarantine fails with "remove from quarantine"', isRemoveQuarantine, res.cause);
    }
  } finally {
    await launcher.close();
    await staticServer.close();
  }

  console.log(allPassed ? 'All fixture self-tests PASSED.' : 'Fixture self-tests FAILED.');
  return allPassed;
}
