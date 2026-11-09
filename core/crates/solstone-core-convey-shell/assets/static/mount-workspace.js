// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  let activeMount = null;

  // Capture this mount before starting asynchronous reads. A late completion
  // from an earlier mount cannot mark the replacement view ready.
  window.createViewReady = function (view) {
    const mount = activeMount;
    return function (state) {
      if (!mount || mount !== activeMount || mount.ready || mount.view !== view) return;
      if (!['content', 'empty', 'error'].includes(state)) {
        console.warn('Unknown view readiness state');
        return;
      }
      mount.ready = true;
      performance.mark('view:ready', { detail: { view, state } });
      if (state !== 'error') prefetchDateViewCode();
    };
  };

  // The views with a date picker name this code themselves. Bringing it into
  // the browser's cache once the page is quiet means the first of those views
  // has nothing extra to wait for. Where it's cached already, this costs no
  // request. A failure here is ignored: the view makes its own request and
  // reports its own error.
  const DATE_VIEW_CODE = ['date-nav.css', 'date-nav.js', 'day-grid.css', 'day-grid.js'];
  // Beside this script's own address: a file can't name its own hashed tree.
  const OWN_DIRECTORY = (() => {
    const src = document.currentScript && document.currentScript.src;
    return src ? src.slice(0, src.lastIndexOf('/') + 1) : '/static/';
  })();
  const PREFETCH_WAIT_MS = 2000;
  let prefetchScheduled = false;

  function prefetchDateViewCode() {
    if (prefetchScheduled) return;
    prefetchScheduled = true;
    const start = () => {
      for (const name of DATE_VIEW_CODE) {
        fetch(OWN_DIRECTORY + name, { credentials: 'same-origin', priority: 'low' })
          .then((response) => response.arrayBuffer())
          .catch(() => {});
      }
    };
    if (window.requestIdleCallback) {
      window.requestIdleCallback(start, { timeout: PREFETCH_WAIT_MS });
    } else {
      setTimeout(start, PREFETCH_WAIT_MS);
    }
  }

  function getTarget(options) {
    if (options && options.target) return options.target;
    return document.getElementById('main-content');
  }

  function renderFailure(target, retry, detail) {
    if (window.SurfaceState) {
      target.innerHTML = window.SurfaceState.error({ retry: true, detail });
    } else {
      target.innerHTML =
        '<div class="surface-state surface-state--error" role="alert">' +
        '<h2 class="surface-state-heading">couldn\'t load this section</h2>' +
        '<p class="surface-state-desc">reload to try again.</p>' +
        '<button type="button" class="surface-state-retry">try again</button>' +
        '</div>';
    }
    const button = target.querySelector('.surface-state-retry');
    if (button) {
      button.addEventListener('click', retry, { once: true });
    }
  }

  function cloneScript(script) {
    const clone = document.createElement('script');
    for (const attr of script.attributes) {
      clone.setAttribute(attr.name, attr.value);
    }
    if (!script.src) {
      clone.text = script.textContent;
    }
    return clone;
  }

  // A script element shows no bytes to the page, so its bound is on the whole
  // load, set as high as the longest read bound. The largest view script
  // takes a few seconds on a weak signal.
  const SCRIPT_LOAD_BOUND_MS = 60000;

  function runScript(script, chain) {
    return new Promise((resolve, reject) => {
      const clone = cloneScript(script);
      if (clone.src) {
        clone.async = false;
        const timer = setTimeout(() => {
          reject(new Error(`Script did not load within ${SCRIPT_LOAD_BOUND_MS} ms: ${clone.src}`));
        }, SCRIPT_LOAD_BOUND_MS);
        clone.addEventListener('load', () => {
          clearTimeout(timer);
          chain.ran = true;
          resolve();
        }, { once: true });
        clone.addEventListener(
          'error',
          () => {
            clearTimeout(timer);
            // A view script from an older build: reload once for the new one.
            // A second failure stays on this view's error state.
            window.recoverFromStaleAsset?.(clone.src);
            reject(new Error(`Failed to load script ${clone.src}`));
          },
          { once: true }
        );
        script.replaceWith(clone);
        return;
      }
      chain.ran = true;
      script.replaceWith(clone);
      resolve();
    });
  }

  // A script can name the one route it belongs to, so a view with several
  // pages doesn't load every page's code on each of them.
  function scriptRunsHere(script) {
    const prefix = script.getAttribute('data-route-prefix');
    return !prefix || (window.location.pathname || '').startsWith(prefix);
  }

  async function replayScripts(target, chain) {
    const scripts = Array.from(target.querySelectorAll('script'));
    for (const script of scripts) {
      if (!scriptRunsHere(script)) {
        script.remove();
        continue;
      }
      await runScript(script, chain);
    }
  }

  function parseFragment(html) {
    const template = document.createElement('template');
    template.innerHTML = html;
    return template.content;
  }

  // Every script starts downloading now, together. They still run one at a
  // time, each waiting for the one before it (replayScripts), so a failed
  // script stops the ones after it exactly as it always has. A preload that
  // fails is ignored: the script element makes its own request and reports
  // its own error.
  function preloadScripts(fragment) {
    const links = [];
    for (const script of fragment.querySelectorAll('script[src]')) {
      if (!scriptRunsHere(script)) continue;
      const link = document.createElement('link');
      link.setAttribute('rel', 'preload');
      link.setAttribute('as', 'script');
      link.setAttribute('href', script.getAttribute('src'));
      document.head.appendChild(link);
      links.push(link);
    }
    return links;
  }

  const STYLESHEET_WAIT_MS = 3000;
  const loadedStylesheets = new Set();

  // A view's stylesheets are in place before its markup is shown, so it never
  // paints unstyled. A sheet that fails or hangs doesn't hold the view back;
  // a stale one is recovered by the listener in api.js.
  function loadStylesheets(fragment) {
    const pending = [];
    for (const source of fragment.querySelectorAll('link[rel="stylesheet"]')) {
      const href = source.getAttribute('href');
      if (loadedStylesheets.has(href)) continue;
      loadedStylesheets.add(href);
      const link = document.createElement('link');
      link.setAttribute('rel', 'stylesheet');
      link.setAttribute('href', href);
      pending.push(new Promise((resolve) => {
        link.addEventListener('load', resolve, { once: true });
        link.addEventListener('error', () => {
          // Forget it, so "try again" asks for it again.
          loadedStylesheets.delete(href);
          link.remove();
          resolve();
        }, { once: true });
      }));
      document.head.appendChild(link);
    }
    if (!pending.length) return Promise.resolve();
    return Promise.race([
      Promise.all(pending),
      new Promise((resolve) => setTimeout(resolve, STYLESHEET_WAIT_MS)),
    ]);
  }

  // Once any of a view's scripts has run, its listeners and timers are on the
  // page, and running the fragment again would stack a second set on them.
  // So "try again" after that point loads the whole page; before it, nothing
  // was attached and the fragment is simply asked for again.
  function retryAfter(chain, again) {
    if (chain.ran) {
      window.location.reload();
      return;
    }
    again();
  }

  const BUILD_RELOAD_STAMP_KEY = 'solstone.serverBuildReloadAt';

  async function mountWorkspaceFragment(url, options = {}) {
    const target = getTarget(options);
    if (!target) {
      throw new Error('mountWorkspaceFragment requires a target');
    }

    // The journal was upgraded under this page. A mount here would mix this
    // page's code with the new build's, so load the page from the new build.
    if (window.appEvents?.buildChanged?.() && window.reloadOnce?.(BUILD_RELOAD_STAMP_KEY)) {
      return;
    }

    window.beginViewGeneration?.();
    activeMount = { view: options.appName || null, ready: false };
    const reportReady = window.createViewReady(activeMount.view);
    const chain = { ran: false };
    const retry = () => {
      retryAfter(chain, () => {
        mountWorkspaceFragment(url, options).catch((error) => {
          if (window.logError) {
            window.logError(error, { context: 'workspace-fragment-retry', url });
          }
        });
      });
    };

    try {
      const request = window.boundedRequest
        ? await window.boundedRequest(url, { credentials: 'same-origin' })
        : await fetch(url, { credentials: 'same-origin' }).then((response) => ({
          response,
          readText: () => response.text(),
          discard: () => {}
        }));
      const { response } = request;
      if (window.isLoopbackRefusal?.(response)) {
        window.recoverFromLoopbackRefusal?.();
      }
      if (response.redirected) {
        request.discard();
        // A fragment route redirects only when the whole page belongs
        // elsewhere (first-run setup, for one). Follow it as a page; never
        // mount the page it lands on as if it were this view's fragment.
        window.location.href = response.url;
        return;
      }
      if (!response.ok) {
        request.discard();
        const error = new Error(`Request failed (HTTP ${response.status})`);
        error.status = response.status;
        error.statusText = response.statusText || '';
        error.url = url;
        error.timestamp = Date.now();
        throw error;
      }
      const html = await request.readText();
      const fragment = parseFragment(html);
      const preloads = preloadScripts(fragment);
      try {
        await loadStylesheets(fragment);
        target.innerHTML = html;
        target.querySelectorAll('link[rel="stylesheet"]').forEach((link) => link.remove());
        performance.mark('workspace:inserted');
        await replayScripts(target, chain);
      } finally {
        preloads.forEach((link) => link.remove());
      }
      performance.mark('view:scripts-loaded');
      document.dispatchEvent(
        new CustomEvent('workspace:mounted', {
          detail: { appName: options.appName || null, url: url }
        })
      );
    } catch (error) {
      renderFailure(target, retry, error);
      reportReady('error');
      if (typeof options.onRetry === 'function') {
        options.onRetry(error);
      }
      if (window.logError) {
        window.logError(error, { context: 'workspace-fragment-mount', url });
      }
      throw error;
    }
  }

  window.mountWorkspaceFragment = mountWorkspaceFragment;
})();
