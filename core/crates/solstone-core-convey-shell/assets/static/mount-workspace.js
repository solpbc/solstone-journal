// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  let activeMount = null;

  // Capture this mount before starting asynchronous reads. A late completion
  // from an earlier mount cannot mark the replacement view ready.
  window.createViewReady = function (view) {
    const mount = activeMount;
    return readyReporter(mount, view);
  };

  // A view that takes a change in place (a day step) reports it ready again,
  // as a new readiness of the same mount.
  window.beginViewReady = function (view, detail) {
    activeMount = { view, ready: false, detail: detail || null };
    return readyReporter(activeMount, view);
  };

  function readyReporter(mount, view) {
    return function (state) {
      if (!mount || mount !== activeMount || mount.ready || mount.view !== view) return;
      if (!['content', 'empty', 'error'].includes(state)) {
        console.warn('Unknown view readiness state');
        return;
      }
      mount.ready = true;
      performance.mark('view:ready', { detail: Object.assign({ view, state }, mount.detail || {}) });
      // The rest of the shell waits for this mark, so its bytes are not part
      // of the wait for the first view.
      window.loadShellRest?.();
      window.settleEarly?.();
      if (state !== 'error') prefetchDateViewCode();
    };
  }

  // The views with a date picker name this code themselves. Bringing it into
  // the browser's cache once the page is quiet means the first of those views
  // has nothing extra to wait for. Where it's cached already, this costs no
  // request. A failure here is ignored: the view makes its own request and
  // reports its own error.
  const PREFETCH_WAIT_MS = 2000;
  let prefetchScheduled = false;

  function prefetchDateViewCode() {
    if (prefetchScheduled) return;
    prefetchScheduled = true;
    const start = () => {
      for (const path of window.__SOL_ENTRY_TABLE__?.dateViewCode || []) {
        fetch(path, { credentials: 'same-origin', priority: 'low' })
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

  // `place` is where the script runs, when that isn't where it stands.
  function runScript(script, chain, place) {
    const at = place || script;
    return new Promise((resolve, reject) => {
      const clone = cloneScript(script);
      if (place) clone.setAttribute('data-view-code', '');
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
        at.replaceWith(clone);
        return;
      }
      chain.ran = true;
      at.replaceWith(clone);
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
  const loadedStylesheets = new Map();

  // Only the mounted view's own stylesheets apply. A sheet another view
  // loaded stays in the page, switched off, so coming back to that view asks
  // for nothing.
  function applyStylesheets(fragment) {
    const wanted = new Set(
      Array.from(fragment.querySelectorAll('link[rel="stylesheet"]'), (link) => link.getAttribute('href'))
    );
    loadedStylesheets.forEach((link, href) => {
      link.disabled = !wanted.has(href);
    });
  }

  // A view's stylesheets are in place before its markup is shown, so it never
  // paints unstyled. A sheet that fails or hangs doesn't hold the view back;
  // a stale one is recovered by the listener in api.js.
  function loadStylesheets(fragment) {
    const pending = [];
    for (const source of fragment.querySelectorAll('link[rel="stylesheet"]')) {
      const href = source.getAttribute('href');
      if (loadedStylesheets.has(href)) continue;
      const link = document.createElement('link');
      link.setAttribute('rel', 'stylesheet');
      link.setAttribute('href', href);
      loadedStylesheets.set(href, link);
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

  // A view on the view contract only registers when its script runs, so its
  // scripts run once in the life of the page, before its markup goes in, and
  // stay in the head. The view itself starts when it is mounted.
  const ranScripts = new Set();

  async function runContractScripts(fragment, chain) {
    for (const script of Array.from(fragment.querySelectorAll('script'))) {
      if (!scriptRunsHere(script)) continue;
      const key = script.getAttribute('src') || script.textContent;
      if (ranScripts.has(key)) continue;
      const place = document.createElement('template');
      document.head.appendChild(place);
      try {
        await runScript(script, chain, place);
      } catch (error) {
        place.remove();
        throw error;
      }
      ranScripts.add(key);
    }
  }

  // A fragment that names hashed code this page's entry table doesn't list
  // comes from another build of the journal: its code mustn't run in this
  // page.
  const HASHED_CODE = /\/[0-9a-f]{16}\//;
  function fromAnotherBuild(appName, fragment) {
    const listed = new Set((window.__SOL_ENTRY_TABLE__?.views?.[appName]?.scripts || []).map((script) => script.src));
    return Array.from(fragment.querySelectorAll('script[src]')).some((script) => {
      const src = script.getAttribute('src') || '';
      return HASHED_CODE.test(src) && !listed.has(src);
    });
  }

  function isContractView(appName) {
    return !!(appName && window.SolViews && window.__SOL_ENTRY_TABLE__?.views?.[appName]?.contract);
  }

  // Once any of a view's scripts has run, its listeners and timers are on the
  // page, and running the fragment again would stack a second set on them.
  // So "try again" after that point loads the whole page; before it, nothing
  // was attached and the fragment is simply asked for again.
  // Once any of a view's scripts has run, a view off the contract has its
  // listeners and timers on the page, and running the fragment again would
  // stack a second set on them. So "try again" after that point loads the
  // whole page; before it, nothing was attached and the fragment is simply
  // asked for again. A view on the contract took back what it added, so it is
  // always simply mounted again.
  function retryAfter(chain, again) {
    if (chain.ran && !chain.contract) {
      window.location.reload();
      return;
    }
    again();
  }

  const BUILD_RELOAD_STAMP_KEY = 'solstone.serverBuildReloadAt';

  // The markup that goes into the page for a view on the contract: its
  // scripts already ran in the head, and its stylesheets are in the head.
  function contractMarkup(fragment) {
    const copy = fragment.cloneNode(true);
    copy.querySelectorAll('script, link[rel="stylesheet"]').forEach((node) => node.remove());
    return copy;
  }

  // Answers with the fragment's text. A caller that already holds it (the
  // router keeps each pilot view's fragment for the life of the page) passes
  // it in, and nothing is asked.
  async function fetchFragment(url, options) {
    if (typeof options.fragmentText === 'string') return { html: options.fragmentText, startedEarly: false };
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
      return null;
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
    return { html: await request.readText(), response };
  }

  // Options:
  // - `appName`: the view.
  // - `fragmentText`: the fragment, when the caller holds it.
  // - `signal`: a newer navigation took over; stop without drawing.
  // - `swap(fn)`: runs the step that replaces the page's view (the router
  //   wraps it in a view transition); resolves when it has run.
  // - `onContractMount(instance)`: the mounted contract view.
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

    const contract = isContractView(options.appName);
    window.beginViewGeneration?.();
    const startedEarly = window.claimEarlyView?.(options.appName || null) === true;
    activeMount = { view: options.appName || null, ready: false };
    const reportReady = window.createViewReady(activeMount.view);
    const chain = { ran: false, contract };
    const superseded = () => !!options.signal?.aborted;
    const retry = () => {
      retryAfter(chain, () => {
        const again = Object.assign({}, options, { swap: null, signal: null });
        mountWorkspaceFragment(url, again).catch((error) => {
          if (window.logError) {
            window.logError(error, { context: 'workspace-fragment-retry', url });
          }
        });
      });
    };

    try {
      const fetched = await fetchFragment(url, options);
      if (!fetched || superseded()) return;
      const { html } = fetched;
      const fragment = parseFragment(html);
      if (contract) {
        if (fromAnotherBuild(options.appName, fragment)) {
          if (window.reloadOnce?.(BUILD_RELOAD_STAMP_KEY)) return;
          throw new Error('The view is from another build of the journal');
        }
        // The scripts of the view only register it, so they run first; the
        // view draws once it is mounted into its markup.
        await loadStylesheets(fragment);
        await runContractScripts(fragment, chain);
        if (superseded()) return;
        if (!window.SolViews.registered(options.appName)) {
          throw new Error(`View ${options.appName} didn't register`);
        }
        let mounted = null;
        const place = () => {
          window.SolViews.dispose();
          applyStylesheets(fragment);
          target.replaceChildren(contractMarkup(fragment));
          performance.mark('workspace:inserted');
          window.solRouter?.placed?.(target);
          window.solRouter?.watchLoading?.(target);
          mounted = window.SolViews.mount(options.appName, { reportReady });
          // The swap waits for the view's first synchronous drawing only.
          return mounted.then(() => {}, () => {});
        };
        if (typeof options.swap === 'function') await options.swap(place);
        else await place();
        const instance = await mounted;
        window.solRouter?.keepFragment?.(options.appName, html);
        options.onContractMount?.(instance);
      } else {
        // The head of the page already asked for these on a first mount.
        const preloads = startedEarly ? [] : preloadScripts(fragment);
        try {
          await loadStylesheets(fragment);
          window.SolViews?.dispose();
          target.innerHTML = html;
          target.querySelectorAll('link[rel="stylesheet"]').forEach((link) => link.remove());
          performance.mark('workspace:inserted');
          await replayScripts(target, chain);
        } finally {
          preloads.forEach((link) => link.remove());
        }
      }
      performance.mark('view:scripts-loaded');
      // A contract view reports ready later, from its own first read. Loading
      // the rest of the shell here would put that request ahead of the mark.
      if (!contract) window.loadShellRest?.();
      document.dispatchEvent(
        new CustomEvent('workspace:mounted', {
          detail: { appName: options.appName || null, url: url }
        })
      );
    } catch (error) {
      if (superseded()) return;
      window.settleEarly?.();
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
