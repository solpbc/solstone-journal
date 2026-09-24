// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { connectWebSocket, CdpClient } from './cdp.mjs';
import { startStaticServer } from './server.mjs';

export function checkNodeVersion() {
  const nodeMajor = parseInt(process.versions.node.split('.')[0], 10);
  if (isNaN(nodeMajor) || nodeMajor < 22) {
    console.error(`Error: Node 22 or higher is required (current: ${process.versions.node})`);
    process.exit(1);
  }
}

export function loadExcludeList(excludePath) {
  if (!excludePath || !fs.existsSync(excludePath)) return new Set();
  const lines = fs.readFileSync(excludePath, 'utf-8').split('\n');
  return new Set(lines.map((l) => l.trim()).filter((l) => l && !l.startsWith('#')));
}

export function loadQuarantineList(quarantinePath) {
  if (!quarantinePath || !fs.existsSync(quarantinePath)) return new Set();
  const lines = fs.readFileSync(quarantinePath, 'utf-8').split('\n');
  return new Set(lines.map((l) => l.trim()).filter((l) => l && !l.startsWith('#')));
}

export class ChromeLauncher {
  constructor({ chromeBin = '/usr/bin/google-chrome' } = {}) {
    this.chromeBin = chromeBin;
    this.process = null;
    this.profileDir = null;
    this.browserWsUrl = null;
    this.browserCdp = null;
    this.port = null;
  }

  async launch() {
    if (!fs.existsSync(this.chromeBin)) {
      throw new Error(`Chrome binary not found or not executable: ${this.chromeBin}`);
    }
    try {
      fs.accessSync(this.chromeBin, fs.constants.X_OK);
    } catch (_) {
      throw new Error(`Chrome binary not found or not executable: ${this.chromeBin}`);
    }

    const tempBase = process.env.TMPDIR || '/var/tmp';
    this.profileDir = fs.mkdtempSync(path.join(tempBase, 'solstone-convey-chrome-'));

    const args = [
      '--headless=new',
      '--window-size=1280,720',
      '--lang=en-US',
      `--user-data-dir=${this.profileDir}`,
      '--remote-debugging-port=0',
      '--disable-gpu',
      '--disable-dev-shm-usage',
      '--no-first-run',
      '--no-default-browser-check',
      'about:blank',
    ];

    const env = {
      ...process.env,
      TZ: 'UTC',
      LANG: 'C.UTF-8',
    };

    return new Promise((resolve, reject) => {
      let stderrAcc = '';
      let resolved = false;

      try {
        this.process = spawn(this.chromeBin, args, {
          env,
          stdio: ['ignore', 'pipe', 'pipe'],
        });
      } catch (err) {
        return reject(new Error(`Failed to spawn Chrome: ${err.message}`));
      }

      const onData = (chunk) => {
        const text = chunk.toString();
        stderrAcc += text;

        const match = stderrAcc.match(/DevTools listening on (ws:\/\/[^\s]+)/);
        if (match && !resolved) {
          resolved = true;
          this.browserWsUrl = match[1];
          const urlObj = new URL(this.browserWsUrl);
          this.port = parseInt(urlObj.port, 10);

          connectWebSocket(this.browserWsUrl)
            .then(async (cdp) => {
              this.browserCdp = cdp;
              try {
                await this.browserCdp.send('Target.setDiscoverTargets', { discover: true });
              } catch (_) {}
              resolve(this);
            })
            .catch(reject);
        }
      };

      this.process.stderr.on('data', onData);

      this.process.on('error', (err) => {
        if (!resolved) {
          resolved = true;
          reject(new Error(`Chrome process error: ${err.message}`));
        }
      });

      this.process.on('exit', (code, signal) => {
        if (!resolved) {
          resolved = true;
          reject(new Error(`Chrome exited prematurely with code ${code}, signal ${signal}`));
        }
      });

      // 10s startup timeout
      setTimeout(() => {
        if (!resolved) {
          resolved = true;
          reject(new Error(`Timed out waiting for Chrome to start DevTools. Stderr: ${stderrAcc}`));
        }
      }, 10000);
    });
  }

  async close() {
    if (this.browserCdp) {
      this.browserCdp.close();
      this.browserCdp = null;
    }
    if (this.process) {
      try {
        this.process.kill('SIGTERM');
      } catch (_) {}
      this.process = null;
    }
    if (this.profileDir && fs.existsSync(this.profileDir)) {
      try {
        fs.rmSync(this.profileDir, { recursive: true, force: true });
      } catch (_) {}
      this.profileDir = null;
    }
  }
}

export async function runPage({
  browserCdp,
  chromePort,
  serverOrigin,
  pageUrlPath,
  freezeDate = true,
  ceilingMs = 10000,
  settleMs = 500,
  snapshotMode = false,
  quarantineSet = new Set(),
}) {
  const fullUrl = `${serverOrigin}${pageUrlPath.startsWith('/') ? '' : '/'}${pageUrlPath}`;

  // 1. Create target
  const { targetId } = await browserCdp.send('Target.createTarget', { url: 'about:blank' });
  const pageWsUrl = `ws://127.0.0.1:${chromePort}/devtools/page/${targetId}`;
  const pageCdp = await connectWebSocket(pageWsUrl);

  const uncaughtExceptions = [];
  let targetCrashed = false;
  let socketClosed = false;

  const onBrowserTargetCrashed = (params) => {
    if (params.targetId === targetId) {
      targetCrashed = true;
    }
  };
  browserCdp.on('Target.targetCrashed', onBrowserTargetCrashed);

  pageCdp.on('Runtime.exceptionThrown', (params) => {
    const details = params.exceptionDetails;
    let desc = details.text || 'Uncaught exception';
    if (details.exception && details.exception.description) {
      desc = details.exception.description;
    } else if (details.message) {
      desc = details.message;
    }
    uncaughtExceptions.push(desc);
  });

  pageCdp.on('Inspector.targetCrashed', () => {
    targetCrashed = true;
  });

  pageCdp.on('close', () => {
    socketClosed = true;
  });

  try {
    await pageCdp.send('Inspector.enable');
    await pageCdp.send('Page.enable');
    await pageCdp.send('Runtime.enable');

    const injectionCode = `
      (function() {
        ${
          freezeDate
            ? `
        const FROZEN_TIME = 1788696000000;
        const OriginalDate = window.Date;
        function MockDate(...args) {
          if (!(this instanceof MockDate)) {
            return new OriginalDate(FROZEN_TIME).toString();
          }
          if (args.length === 0) {
            return new OriginalDate(FROZEN_TIME);
          }
          return new OriginalDate(...args);
        }
        MockDate.prototype = OriginalDate.prototype;
        MockDate.now = function() { return FROZEN_TIME; };
        MockDate.parse = OriginalDate.parse;
        MockDate.UTC = OriginalDate.UTC;
        window.Date = MockDate;
        `
            : ''
        }

        window.__solstoneUnhandledRejections = [];
        window.addEventListener('unhandledrejection', function(event) {
          let reason = event.reason;
          let text = 'Unhandled rejection';
          if (reason instanceof Error) {
            text = reason.stack || reason.message;
          } else if (reason !== undefined && reason !== null) {
            text = String(reason);
          }
          window.__solstoneUnhandledRejections.push(text);
        });
      })();
    `;

    await pageCdp.send('Page.addScriptToEvaluateOnNewDocument', { source: injectionCode });

    // Navigate to page
    await pageCdp.send('Page.navigate', { url: fullUrl });

    const readDomRows = async () => {
      if (socketClosed || targetCrashed) return { pass: 0, fail: 0, total: 0, rows: [] };
      try {
        const evalRes = await pageCdp.send('Runtime.evaluate', {
          expression: `(() => {
            const containers = [
              document.getElementById('results'),
              document.getElementById('assertions')
            ].filter(Boolean);
            let pass = 0;
            let fail = 0;
            const rows = [];
            for (const c of containers) {
              for (const child of c.children) {
                if (child.id === 'test-results') continue;
                const isP = child.classList.contains('pass');
                const isF = child.classList.contains('fail');
                if (isP) pass++;
                if (isF) fail++;
                if (isP || isF) {
                  rows.push({
                    status: isP ? 'pass' : 'fail',
                    text: (child.textContent || '').trim()
                  });
                }
              }
            }
            return { pass, fail, total: pass + fail, rows };
          })()`,
          returnByValue: true,
        });
        return evalRes?.result?.value || { pass: 0, fail: 0, total: 0, rows: [] };
      } catch (_) {
        return { pass: 0, fail: 0, total: 0, rows: [] };
      }
    };

    const readRejections = async () => {
      if (socketClosed || targetCrashed) return [];
      try {
        const evalRes = await pageCdp.send('Runtime.evaluate', {
          expression: 'window.__solstoneUnhandledRejections || []',
          returnByValue: true,
        });
        return evalRes?.result?.value || [];
      } catch (_) {
        return [];
      }
    };

    const readCompletion = async () => {
      if (socketClosed || targetCrashed) return null;
      try {
        const evalRes = await pageCdp.send('Runtime.evaluate', {
          expression: 'window.__solstoneConveyTest || null',
          returnByValue: true,
        });
        return evalRes?.result?.value || null;
      } catch (_) {
        return null;
      }
    };

    if (snapshotMode) {
      await new Promise((r) => setTimeout(r, settleMs));
      const dom = await readDomRows();
      const rejections = await readRejections();
      const allExceptions = [...uncaughtExceptions, ...rejections];
      return {
        ok: allExceptions.length === 0 && dom.fail === 0,
        pageUrlPath,
        pass: dom.pass,
        fail: dom.fail,
        total: dom.total,
        exceptions: allExceptions,
        rows: dom.rows,
      };
    }

    // Normal Test Execution Mode
    const startTime = Date.now();
    let completion = null;

    while (Date.now() - startTime < ceilingMs) {
      if (targetCrashed || socketClosed) {
        return { ok: false, pageUrlPath, cause: 'renderer crash' };
      }

      const rejections = await readRejections();
      if (uncaughtExceptions.length > 0 || rejections.length > 0) {
        const errs = [...uncaughtExceptions, ...rejections].join('; ');
        return { ok: false, pageUrlPath, cause: `uncaught exception / unhandled rejection: ${errs}` };
      }

      completion = await readCompletion();
      if (completion && completion.complete === true) {
        break;
      }

      await new Promise((r) => setTimeout(r, 50));
    }

    if (!completion || completion.complete !== true) {
      if (targetCrashed || socketClosed) {
        return { ok: false, pageUrlPath, cause: 'renderer crash' };
      }
      const rejections = await readRejections();
      if (uncaughtExceptions.length > 0 || rejections.length > 0) {
        const errs = [...uncaughtExceptions, ...rejections].join('; ');
        return { ok: false, pageUrlPath, cause: `uncaught exception / unhandled rejection: ${errs}` };
      }
      return { ok: false, pageUrlPath, cause: `no completion within ceiling (${ceilingMs}ms)` };
    }

    // Snapshot rows before settle
    const beforeSettle = await readDomRows();

    // Settle window
    await new Promise((r) => setTimeout(r, settleMs));

    // Snapshot rows after settle
    const afterSettle = await readDomRows();

    // Check exceptions after settle
    const lateRejections = await readRejections();
    if (uncaughtExceptions.length > 0 || lateRejections.length > 0) {
      const errs = [...uncaughtExceptions, ...lateRejections].join('; ');
      return { ok: false, pageUrlPath, cause: `uncaught exception / unhandled rejection during settle: ${errs}` };
    }

    if (afterSettle.total !== beforeSettle.total) {
      return {
        ok: false,
        pageUrlPath,
        cause: `assertion row added during settle window (before: ${beforeSettle.total}, after: ${afterSettle.total})`,
      };
    }

    if (afterSettle.total === 0) {
      return { ok: false, pageUrlPath, cause: 'zero assertions' };
    }

    if (afterSettle.fail !== completion.failed) {
      return {
        ok: false,
        pageUrlPath,
        cause: `row-count mismatch: DOM .fail rows (${afterSettle.fail}) != reported failed (${completion.failed})`,
      };
    }

    // Quarantine & failure evaluation
    const reportedFailures = completion.failures || [];
    const failedNames = reportedFailures.map((f) => f.name);

    // Find all assertions on this page that match quarantine
    const quarantinedOnPage = [];
    for (const q of quarantineSet) {
      const matchesRow = afterSettle.rows.some((r) => r.text.includes(q));
      const matchesFail = failedNames.includes(q);
      if (matchesRow || matchesFail) {
        quarantinedOnPage.push(q);
      }
    }

    if (quarantinedOnPage.length > 0) {
      const allFailedAreQuarantined =
        failedNames.length === quarantinedOnPage.length &&
        failedNames.every((f) => quarantinedOnPage.includes(f));

      if (allFailedAreQuarantined) {
        return {
          ok: true,
          pageUrlPath,
          passed: completion.passed,
          failed: completion.failed,
          total: afterSettle.total,
          quarantined: quarantinedOnPage,
        };
      }

      if (failedNames.length === 0) {
        return {
          ok: false,
          pageUrlPath,
          cause: `remove from quarantine: ${quarantinedOnPage.join(', ')}`,
        };
      }

      const unexpected = failedNames.filter((f) => !quarantinedOnPage.includes(f));
      const passedQuarantined = quarantinedOnPage.filter((q) => !failedNames.includes(q));
      let cause = '';
      if (unexpected.length > 0) {
        cause = `reported failure: ${unexpected.join(', ')}`;
      } else if (passedQuarantined.length > 0) {
        cause = `remove from quarantine: ${passedQuarantined.join(', ')}`;
      }
      return { ok: false, pageUrlPath, cause };
    }

    if (reportedFailures.length > 0) {
      const names = reportedFailures.map((f) => (f.detail ? `${f.name} (${f.detail})` : f.name)).join(', ');
      return { ok: false, pageUrlPath, cause: `reported failure: ${names}` };
    }

    return {
      ok: true,
      pageUrlPath,
      passed: completion.passed,
      failed: completion.failed,
      total: afterSettle.total,
    };
  } finally {
    pageCdp.close();
    try {
      await browserCdp.send('Target.closeTarget', { targetId });
    } catch (_) {}
  }
}
