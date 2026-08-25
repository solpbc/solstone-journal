// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const fs = require('fs');
const path = require('path');
const vm = require('vm');

class FakeClassList {
  constructor() {
    this._classes = new Set();
  }

  add(name) {
    this._classes.add(name);
  }

  remove(name) {
    this._classes.delete(name);
  }

  toggle(name, force) {
    const shouldAdd = force === undefined ? !this._classes.has(name) : Boolean(force);
    if (shouldAdd) {
      this.add(name);
    } else {
      this.remove(name);
    }
    return shouldAdd;
  }
}

class FakeElement {
  constructor() {
    this.attributes = {};
    this.children = [];
    this.classList = new FakeClassList();
    this.hidden = false;
    this.id = '';
    this.innerHTML = '';
    this.placeholder = '';
    this.style = {};
    this.textContent = '';
  }

  appendChild(child) {
    this.children.push(child);
    return child;
  }
}

function makeDocument(menuItems) {
  const body = new FakeElement();
  const head = new FakeElement();
  return {
    body,
    head,
    title: '',
    createElement() {
      return new FakeElement();
    },
    getElementById() {
      return null;
    },
    querySelector(selector) {
      if (selector === '.menu-bar .menu-items') return menuItems;
      return null;
    },
  };
}

async function flush(times = 1) {
  for (let index = 0; index < times; index += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

async function main() {
  const repoRoot = process.argv[2];
  if (!repoRoot) throw new Error('repo root argument required');

  const source = fs.readFileSync(
    path.join(repoRoot, 'solstone', 'convey', 'static', 'shell_boot.js'),
    'utf8'
  );
  const menuItems = new FakeElement();
  let shellReady = false;
  const mounted = [];
  const shell = {
    apps: [
      {
        name: 'home',
        label: 'Home',
        icon: 'H',
        starred: true,
        app_bar: false,
        facets_enabled: false,
        workspace_url: '/app/home/workspace',
      },
      {
        name: 'backup',
        label: 'Backup',
        icon: 'B',
        starred: false,
        app_bar: false,
        facets_enabled: false,
        workspace_url: '/app/backup/workspace',
      },
      {
        name: 'odd<&"name',
        label: 'Odd <& "name',
        icon: 'O',
        starred: false,
        app_bar: false,
        facets_enabled: false,
        workspace_url: '/app/odd/workspace',
      },
    ],
    facets: [],
    selected_facet: null,
    settings: { reporting_enabled: true },
  };
  const document = makeDocument(menuItems);
  const window = {
    document,
    location: { pathname: '/app/home/' },
    CONVEY_COPY: {},
    apiJson(url) {
      if (url !== '/api/shell') throw new Error(`unexpected apiJson ${url}`);
      return Promise.resolve(shell);
    },
    mountWorkspaceFragment(url, options) {
      mounted.push({ url, appName: options && options.appName });
      return Promise.resolve();
    },
    resolveSolShellReady() {
      shellReady = true;
    },
  };
  window.window = window;

  const context = vm.createContext({
    console,
    document,
    setImmediate,
    window,
  });
  vm.runInContext(source, context, { filename: 'shell_boot.js' });
  await flush(5);

  const rawMenu = menuItems.innerHTML;
  const hrefs = Array.from(rawMenu.matchAll(/href="([^"]*)"/g)).map(
    (match) => match[1]
  );
  process.stdout.write(JSON.stringify({ hrefs, mounted, rawMenu, shellReady }) + '\n');
}

main().catch((error) => {
  process.stderr.write(`${error && error.stack ? error.stack : error}\n`);
  process.exitCode = 1;
});
