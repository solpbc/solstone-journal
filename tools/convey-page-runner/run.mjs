// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import path from 'node:path';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  checkNodeVersion,
  ChromeLauncher,
  loadExcludeList,
  loadQuarantineList,
  runPage,
} from './runner.mjs';
import { startStaticServer } from './server.mjs';

checkNodeVersion();

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '../..');
const ASSETS_ROOT = path.join(REPO_ROOT, 'core/crates/solstone-core-convey-shell/assets');
const EXCLUDE_FILE = path.join(__dirname, 'exclude.txt');
const QUARANTINE_FILE = path.join(__dirname, 'quarantine.txt');

function parseArgs() {
  const args = process.argv.slice(2);
  const options = {
    snapshot: false,
    selfTest: false,
    chromeBin: '/usr/bin/google-chrome',
    ceiling: 10000,
    settle: 500,
    dirs: [],
    files: [],
  };

  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--snapshot') {
      options.snapshot = true;
    } else if (arg === '--self-test') {
      options.selfTest = true;
    } else if (arg === '--chrome-bin' && i + 1 < args.length) {
      options.chromeBin = args[++i];
    } else if (arg.startsWith('--chrome-bin=')) {
      options.chromeBin = arg.split('=')[1];
    } else if (arg === '--ceiling' && i + 1 < args.length) {
      options.ceiling = parseInt(args[++i], 10);
    } else if (arg.startsWith('--ceiling=')) {
      options.ceiling = parseInt(arg.split('=')[1], 10);
    } else if (arg === '--settle' && i + 1 < args.length) {
      options.settle = parseInt(args[++i], 10);
    } else if (arg.startsWith('--settle=')) {
      options.settle = parseInt(arg.split('=')[1], 10);
    } else if (arg.endsWith('.html')) {
      options.files.push(arg);
    } else if (!arg.startsWith('-')) {
      options.dirs.push(arg);
    }
  }

  return options;
}

function findHtmlFiles(dirs, excludes) {
  const files = [];
  for (const dir of dirs) {
    if (!fs.existsSync(dir)) continue;
    const entries = fs.readdirSync(dir, { withFileTypes: true });
    for (const entry of entries) {
      if (entry.isFile() && entry.name.endsWith('.html')) {
        if (!excludes.has(entry.name)) {
          files.push(path.join(dir, entry.name));
        }
      }
    }
  }
  return files.sort();
}

async function main() {
  const options = parseArgs();

  if (options.selfTest) {
    const { runSelfTests } = await import('./self_test.mjs');
    const ok = await runSelfTests(options);
    process.exit(ok ? 0 : 1);
  }

  const defaultDirs = [
    path.join(ASSETS_ROOT, 'static/tests'),
    path.join(ASSETS_ROOT, 'network/tests'),
  ];

  const excludes = loadExcludeList(EXCLUDE_FILE);
  const quarantine = loadQuarantineList(QUARANTINE_FILE);

  let targetFiles = [];
  if (options.files.length > 0) {
    targetFiles = options.files.filter((f) => !excludes.has(path.basename(f)));
  } else {
    const dirsToScan = options.dirs.length > 0 ? options.dirs : defaultDirs;
    targetFiles = findHtmlFiles(dirsToScan, excludes);
  }

  // Start Static Loopback Server
  let staticServer;
  try {
    staticServer = await startStaticServer({
      roots: [ASSETS_ROOT, REPO_ROOT],
    });
  } catch (err) {
    console.error(`Error: ${err.message}`);
    process.exit(1);
  }

  // Launch Chrome
  const launcher = new ChromeLauncher({ chromeBin: options.chromeBin });
  try {
    await launcher.launch();
  } catch (err) {
    console.error(`Error: ${err.message}`);
    await staticServer.close();
    process.exit(1);
  }

  let failedPages = 0;

  try {
    if (options.snapshot) {
      console.log('--- CONVEY TEST SNAPSHOT ---');
      console.log('Page | Pass | Fail | Total | Exceptions');
      console.log('-----|------|------|-------|-----------');
      for (const filePath of targetFiles) {
        const relToAssets = path.relative(ASSETS_ROOT, filePath);
        const pageUrl = `/${relToAssets.split(path.sep).join('/')}`;
        const basename = path.basename(filePath);

        const res = await runPage({
          browserCdp: launcher.browserCdp,
          chromePort: launcher.port,
          serverOrigin: staticServer.origin,
          pageUrlPath: pageUrl,
          ceilingMs: options.ceiling,
          settleMs: options.settle,
          snapshotMode: true,
          quarantineSet: quarantine,
        });

        const exDesc = res.exceptions.length > 0 ? res.exceptions.join('; ') : 'none';
        console.log(`${basename} | ${res.pass} | ${res.fail} | ${res.total} | ${exDesc}`);
      }
      console.log('--- END SNAPSHOT ---');
    } else {
      for (const filePath of targetFiles) {
        const relToAssets = path.relative(ASSETS_ROOT, filePath);
        const pageUrl = `/${relToAssets.split(path.sep).join('/')}`;
        const basename = path.basename(filePath);

        const res = await runPage({
          browserCdp: launcher.browserCdp,
          chromePort: launcher.port,
          serverOrigin: staticServer.origin,
          pageUrlPath: pageUrl,
          ceilingMs: options.ceiling,
          settleMs: options.settle,
          snapshotMode: false,
          quarantineSet: quarantine,
        });

        if (res.ok) {
          console.log(`${basename}: ${res.passed || res.total || 0} passed`);
        } else {
          failedPages++;
          console.error(`FAIL: ${basename}: ${res.cause}`);
        }
      }
    }
  } finally {
    await launcher.close();
    await staticServer.close();
  }

  if (!options.snapshot && failedPages > 0) {
    process.exit(1);
  }
}

main().catch((err) => {
  console.error(`Fatal runner error: ${err.stack || err.message}`);
  process.exit(1);
});
