// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Moving between views without loading a new page.
//
// Between two views on the view contract, the page stays: the rail, the
// status mark, the sun arc and the live stream are kept, and only the view in
// `<main>` changes. Every other move is a whole page load, as it always was:
// to or from any other view, out of the journal, or onto a page whose code
// this one doesn't have.
//
// Each history entry has one owner. The router's own entries are moves to
// another address path. An entry that keeps the path and changes only the
// query or the hash belongs to the view (a transcript segment and its tab),
// and the router leaves it alone.
(function () {
  const BUILD_RELOAD_STAMP_KEY = 'solstone.serverBuildReloadAt';
  const SHELL_BUILD_RELOAD_STAMP_KEY = 'solstone.shellBuildReloadAt';
  // A view transition waits for the next view's first reads this long at
  // most, so it never animates into an empty frame or waits on a slow read.
  const TRANSITION_WAIT_MS = 300;
  // How long a view's first load runs before it is announced as still
  // working, as the visual line says it.
  const STILL_WORKING_MS = 10000;
  const SCROLL_ENTRIES = 50;
  const HOVER_INTENT_MS = 100;
  const MOVE_MARKS = ['view:ready', 'view:update', 'router:start', 'router:reread', 'workspace:inserted', 'view:scripts-loaded'];

  let active = false;
  let mountedPath = null;
  let entryId = null;
  let entrySerial = 0;
  let navigation = null;
  let transition = null;
  let loadingTimer = null;
  let connectSerial = 0;
  let wasConnected = false;
  // The connect that last read a failed view again; at most once per connect.
  let rereadConnect = 0;
  const scrolls = new Map();
  const fragments = new Map();
  const prefetched = new Set();

  function entryTable() {
    return window.__SOL_ENTRY_TABLE__?.views || {};
  }

  function viewOf(pathname) {
    const match = /^\/app\/([^/]+)(\/|$)/.exec(pathname || '');
    if (!match) return null;
    const name = decodeURIComponent(match[1]);
    const rest = pathname.slice(match[0].length);
    if (rest.startsWith('api/') || rest.startsWith('static/')) return null;
    return name;
  }

  function isPilot(name) {
    return !!(name && entryTable()[name]?.contract && window.SolViews);
  }

  function knownApp(name) {
    return (window.solShellData?.apps || []).find((app) => app.name === name) || null;
  }

  function newEntryId() {
    entrySerial += 1;
    return `${Date.now().toString(36)}.${entrySerial}`;
  }

  function stateWith(id, owner) {
    const base = window.history.state && typeof window.history.state === 'object' ? window.history.state : {};
    return Object.assign({}, base, { solRoute: { id, owner } });
  }

  function rememberScroll() {
    if (!entryId) return;
    scrolls.delete(entryId);
    scrolls.set(entryId, window.scrollY || 0);
    while (scrolls.size > SCROLL_ENTRIES) scrolls.delete(scrolls.keys().next().value);
  }

  // A view's own history entry (`ctx.history`). A new entry gets an id of its
  // own; a replaced one keeps the entry's id.
  function viewEntry(kind, url) {
    if (kind === 'push') {
      rememberScroll();
      entryId = newEntryId();
      window.history.pushState(stateWith(entryId, 'view'), '', url);
    } else {
      window.history.replaceState(stateWith(entryId || newEntryId(), window.history.state?.solRoute?.owner || 'view'), '', url);
    }
  }

  function fullLoad(url, stampKey, mode) {
    window.beginViewGeneration?.();
    if (stampKey) {
      try {
        window.sessionStorage.setItem(stampKey, String(Date.now()));
      } catch (_) {}
    }
    if (mode === 'pop') window.location.reload();
    else window.location.assign(url.href);
    return true;
  }

  // Where a link or a day step should go: the router, or the browser.
  function decide(url) {
    if (!active || url.origin !== window.location.origin) return false;
    const here = window.SolViews?.current()?.name || viewOf(window.location.pathname);
    const there = viewOf(url.pathname);
    if (!isPilot(here) || !isPilot(there)) return false;
    if (url.pathname === window.location.pathname) {
      // The same page again is a fresh mount; the same path with another
      // query or hash is the view's own address, left to the browser.
      return url.search === window.location.search && url.hash === window.location.hash;
    }
    return true;
  }

  function onClick(event) {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target?.closest?.('a[href]');
    if (!link) return;
    if ((link.target && link.target !== '_self') || link.hasAttribute('download')) return;
    let url;
    try {
      url = new URL(link.href, window.location.href);
    } catch (_) {
      return;
    }
    if (!decide(url)) {
      // A full load from this page. Abort view reads now: the browser can cut
      // the fetch before pagehide runs. A same-path link stays here and keeps
      // its reads.
      if (url.origin !== window.location.origin || url.pathname !== window.location.pathname) {
        window.beginViewGeneration?.();
      }
      return;
    }
    event.preventDefault();
    go(url, url.pathname === window.location.pathname ? 'replace' : 'push');
  }

  function onPopState() {
    if (!active) return;
    const path = window.location.pathname;
    const id = window.history.state?.solRoute?.id || null;
    if (path === mountedPath) {
      // The view's own entry: it restores itself.
      entryId = id;
      return;
    }
    const url = new URL(window.location.href);
    if (!isPilot(viewOf(path))) {
      fullLoad(url, null, 'pop');
      return;
    }
    rememberScroll();
    entryId = id;
    go(url, 'pop');
  }

  // Waits for the next view's first reads to answer, or the wait's end.
  function readsAnswered(early, ms) {
    const responses = Object.values(early?.reads || {}).map((entry) => entry.response.catch(() => null));
    if (!responses.length) return Promise.resolve();
    return Promise.race([
      Promise.all(responses),
      new Promise((resolve) => window.setTimeout(resolve, ms))
    ]);
  }

  function canAnimate() {
    return typeof document.startViewTransition === 'function'
      && document.visibilityState === 'visible'
      && !window.matchMedia?.('(prefers-reduced-motion: reduce)')?.matches;
  }

  // Runs `place` inside a view transition where one can run, and settles once
  // it has run. A rejected transition is expected when a newer move skips it.
  function swapWith(place) {
    if (transition) {
      try { transition.skipTransition(); } catch (_) {}
      transition = null;
    }
    if (!canAnimate()) return Promise.resolve(place());
    // The engine can keep a finished transition, and its callback, reachable
    // after it ends. The callback lets go of the swap once it has run, so a
    // finished transition never keeps a view that has since gone.
    let job = place;
    const run = () => {
      const fn = job;
      job = null;
      return fn ? Promise.resolve(fn()) : Promise.resolve();
    };
    let ran = null;
    const started = document.startViewTransition(() => {
      ran = run();
      return ran;
    });
    transition = started;
    ['ready', 'finished'].forEach((name) => started[name]?.catch?.(() => {}));
    return started.updateCallbackDone.then(
      () => ran,
      () => ran || run()
    ).finally(() => {
      ran = null;
      if (transition === started) transition = null;
    });
  }

  function focusQuietly(node) {
    try {
      node.focus({ preventScroll: true });
    } catch (_) {
      node.focus();
    }
  }

  // A heading the view's markup ships with stays where it is. One the view
  // draws can be drawn again as its reads come in (home's are), and focus on
  // a heading that is taken out falls to the page. So a move focuses the
  // shipped heading, or else the workspace.
  let placedHeading = null;
  function placed(target) {
    placedHeading = target.querySelector('h1, h2');
  }

  function focusView() {
    const main = document.getElementById('main-content');
    const heading = placedHeading;
    placedHeading = null;
    if (!main) return;
    if (!main.hasAttribute('tabindex')) main.setAttribute('tabindex', '-1');
    const into = heading && heading.isConnected && main.contains(heading) && !heading.closest('[hidden]') ? heading : main;
    if (into !== main && !into.hasAttribute('tabindex')) into.setAttribute('tabindex', '-1');
    focusQuietly(into);
  }

  function announce(text) {
    const region = document.getElementById('view-announcer');
    if (!region) return;
    region.textContent = '';
    // A new text node in the live region is what a screen reader announces.
    window.setTimeout(() => {
      region.textContent = text;
    }, 50);
  }

  function stillWorkingWords() {
    return window.CONVEY_COPY?.STILL_WORKING || 'still working…';
  }

  // A view's first load says it is still working once it runs past ten
  // seconds. This is the one place that says it: the visual line in a pilot
  // view's loading state is hidden from assistive technology.
  function watchLoading() {
    window.clearTimeout(loadingTimer);
    const instance = () => window.SolViews?.current();
    loadingTimer = window.setTimeout(() => {
      const view = instance();
      if (view && view.state === null) announce(stillWorkingWords());
    }, STILL_WORKING_MS);
    const settle = () => {
      const view = instance();
      if (!view) return;
      window.SolViews.whenReady(view).then(() => window.clearTimeout(loadingTimer));
    };
    window.setTimeout(settle, 0);
  }

  function restoreScroll(mode) {
    if (mode === 'pop' && entryId && scrolls.has(entryId)) {
      window.scrollTo(0, scrolls.get(entryId));
    } else if (mode !== 'pop') {
      window.scrollTo(0, 0);
    }
  }

  // A new view takes focus at its heading and is announced by name. A day
  // step keeps focus where the owner left it, on the step they pressed.
  function settleAfterMove(mode, label) {
    const view = window.SolViews?.current();
    window.SolViews?.whenReady(view, TRANSITION_WAIT_MS).then(() => {
      restoreScroll(mode);
      if (!label) return;
      focusView();
      announce(label);
    });
  }

  // The view's day in the address, for a view that takes a day in place.
  function dayParams(url, name) {
    const route = window.SolViews.routeOf(url);
    if (route.view !== name || !route.day) return null;
    if (route.pathname !== `/app/${encodeURIComponent(name)}/${route.day}`) return null;
    return { day: route.day };
  }

  async function go(url, mode) {
    const name = viewOf(url.pathname);
    // A page that has learned the journal is another build, or that doesn't
    // know the view's app, has the old code: it loads the new page instead.
    if (window.appEvents?.buildChanged?.()) return fullLoad(url, BUILD_RELOAD_STAMP_KEY, mode);
    const app = knownApp(name);
    if (!app || !app.workspace_url) return fullLoad(url, SHELL_BUILD_RELOAD_STAMP_KEY, mode);

    if (navigation) navigation.abort();
    const controller = window.SolStore.createAbort();
    navigation = controller;
    // A page that stays would keep every move's marks for its whole life:
    // only the latest move's are kept.
    MOVE_MARKS.forEach((mark) => performance.clearMarks?.(mark));
    try {
      return await move(url, mode, name, app, controller);
    } finally {
      if (navigation === controller) navigation = null;
    }
  }

  async function move(url, mode, name, app, controller) {
    if (mode === 'push') {
      rememberScroll();
      entryId = newEntryId();
      window.history.pushState(stateWith(entryId, 'router'), '', url.pathname + url.search + url.hash);
    }

    // The same view, another day: the view takes it in place.
    const current = window.SolViews.current();
    const params = current && current.name === name ? dayParams(url, name) : null;
    if (params && typeof current.handle?.update === 'function') {
      mountedPath = url.pathname;
      window.settleEarly?.();
      const reportReady = window.beginViewReady(name, { update: true });
      window.SolViews.update(params, { reportReady });
      // A day still loading at ten seconds says so, as a new view's does.
      watchLoading();
      performance.mark('view:update', { detail: { view: name } });
      settleAfterMove(mode, null);
      return true;
    }

    // Unused code fetched ahead goes; a fragment that was used stays, one
    // per view, for the life of the page.
    prefetched.forEach((view) => {
      if (view !== name) fragments.delete(view);
    });
    prefetched.clear();

    // The next view's first reads start now, at the move, in a table of
    // their own; whatever the last move started and nothing took ends here.
    // An address with no day first learns its day, so the reads are the
    // day's.
    window.settleEarly?.();
    const held = fragments.get(name);
    const started = Date.now();
    performance.mark('router:start', { detail: { view: name, mode } });
    let early = null;
    try {
      await openDay(app);
      if (controller.signal.aborted) return true;
      early = window.__solEarlyRoute?.(window.location.href, { fragment: !held }) || null;
      mountedPath = window.location.pathname;
      await window.mountWorkspaceFragment(app.workspace_url, {
        appName: name,
        fragmentText: held ? held.text : undefined,
        signal: controller.signal,
        swap: async (place) => {
          await readsAnswered(early, Math.max(0, TRANSITION_WAIT_MS - (Date.now() - started)));
          if (controller.signal.aborted) throw new Error('superseded');
          return swapWith(place);
        },
        onContractMount: () => {}
      });
    } catch (error) {
      if (controller.signal.aborted) return true;
      window.logError?.(error, { context: 'router-navigate', view: name });
    }
    if (controller.signal.aborted) return true;
    window.solShellChrome?.markCurrent?.(name);
    document.title = `${app.label} - journal`;
    settleAfterMove(mode, app.label);
    return true;
  }

  // A view can open on an address with no day, and name a read that picks one
  // on the journal's clock. The page takes that address before the view runs,
  // so the view only ever sees a day in its path. The page's first view and
  // every move to such an address go through here.
  async function openDay(app) {
    const open = entryTable()[app.name]?.openDay;
    if (!open || window.location.pathname !== open.route) return;
    const { day } = await window.apiJson(open.url);
    if (typeof day !== 'string' || !/^\d{8}$/.test(day)) {
      throw new Error('No day to open');
    }
    window.history.replaceState(
      window.history.state,
      '',
      `${open.route}${day}${window.location.search}${window.location.hash}`
    );
  }

  // A day step or another in-page move asked for by code. False when the
  // browser should load it instead.
  function navigate(href) {
    let url;
    try {
      url = new URL(href, window.location.href);
    } catch (_) {
      return false;
    }
    if (!decide(url)) return false;
    go(url, 'push');
    return true;
  }

  // Code ahead of a move, never data: a scroll over the rail on a phone
  // presses on links it never opens, and a read on a large journal is seconds
  // of the journal's work. The fragment is kept in memory, one per view; its
  // scripts and styles go to the browser's cache under their hashed names.
  function prefetchView(name) {
    if (!active || !isPilot(name) || fragments.has(name)) return;
    const entry = entryTable()[name];
    const app = knownApp(name);
    if (!entry || !app) return;
    const record = { text: null };
    fragments.set(name, record);
    prefetched.add(name);
    fetch(entry.fragment, { credentials: 'same-origin', priority: 'low' })
      .then((response) => (response.ok && !response.redirected ? response.text() : null))
      .then((text) => {
        if (text === null) fragments.delete(name);
        else record.text = text;
      })
      .catch(() => fragments.delete(name));
    const code = [...entry.scripts.filter((script) => !script.routePrefix).map((script) => script.src), ...entry.styles];
    code.forEach((href) => {
      fetch(href, { credentials: 'same-origin', priority: 'low' })
        .then((response) => response.arrayBuffer())
        .catch(() => {});
    });
  }

  function intentTarget(event) {
    const link = event.target?.closest?.('#app-rail a[href], #app-dock a[href]');
    if (!link) return null;
    try {
      const url = new URL(link.href, window.location.href);
      return decide(url) ? viewOf(url.pathname) : null;
    } catch (_) {
      return null;
    }
  }

  let hoverTimer = null;
  function onPointerDown(event) {
    const name = intentTarget(event);
    if (name) prefetchView(name);
  }
  function onPointerOver(event) {
    window.clearTimeout(hoverTimer);
    const name = intentTarget(event);
    if (name) hoverTimer = window.setTimeout(() => prefetchView(name), HOVER_INTENT_MS);
  }

  // A view whose first load ended in its error state reads again when the
  // live stream next connects, once per connect: a mount that fails again
  // waits for the connect after.
  function onConnection(state) {
    const connected = !!state?.connected;
    if (connected && !wasConnected) connectSerial += 1;
    wasConnected = connected;
    if (!connected || !active || navigation) return;
    const view = window.SolViews?.current();
    // The view on the page, or the one whose mount itself failed (its
    // fragment or code never arrived).
    const name = view ? view.name : viewOf(window.location.pathname);
    const failed = view ? view.state === 'error' && !view.rereadsItself : isPilot(name);
    if (!failed || rereadConnect === connectSerial) return;
    const app = knownApp(name);
    if (!app) return;
    rereadConnect = connectSerial;
    performance.mark('router:reread', { detail: { view: name } });
    window.mountWorkspaceFragment(app.workspace_url, {
      appName: name,
      fragmentText: fragments.get(name)?.text
    }).catch(() => {});
  }

  // Turned on once the page's first view is a pilot view and mounted.
  function start(name, fragmentText) {
    if (active || !isPilot(name)) return;
    active = true;
    mountedPath = window.location.pathname;
    entryId = window.history.state?.solRoute?.id || newEntryId();
    window.history.replaceState(stateWith(entryId, window.history.state?.solRoute?.owner || 'router'), '');
    if ('scrollRestoration' in window.history) window.history.scrollRestoration = 'manual';
    if (typeof fragmentText === 'string') fragments.set(name, { text: fragmentText });
    document.addEventListener('click', onClick);
    window.addEventListener('popstate', onPopState);
    document.addEventListener('pointerdown', onPointerDown, { passive: true });
    document.addEventListener('pointerover', onPointerOver, { passive: true });
    window.appEvents?.onConnectionState?.(onConnection);
  }

  // The fragment a mount used, kept for the next move to that view: it is
  // this build's code, and a page on another build loads a new page anyway.
  function keepFragment(name, text) {
    if (isPilot(name) && typeof text === 'string') {
      fragments.set(name, { text });
      prefetched.delete(name);
    }
  }

  window.solRouter = {
    start,
    navigate,
    openDay,
    viewEntry,
    watchLoading,
    placed,
    keepFragment,
    isActive: () => active,
    // For tests and the leak check.
    held: () => ({ scrolls: scrolls.size, fragments: fragments.size, active, mountedPath, entryId })
  };
})();
