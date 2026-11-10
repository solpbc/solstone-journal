// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The view contract.
//
//     SolViews.register(name, { mount(ctx) })
//
// A view's script only registers; nothing of the view runs when the script
// loads. `mount(ctx)` draws the view into the page and returns its dispose
// function, or `{ dispose, update }` when the view can take a change of
// parameters in place (a day step) without being mounted again.
//
// Everything a view adds to the page outside its own markup goes through
// `ctx`, which takes it all back when the view goes: listeners on the
// document or the window, timers, observers, live-stream subscriptions,
// globals, body classes and the document title. Its reads go through
// `ctx.read` and `ctx.fetch`, which end with it. A mount that throws part-way
// takes back what it had added before the throw, so trying again starts
// clean.
(function () {
  const views = new Map();
  let current = null;
  let serial = 0;

  function logError(error, context) {
    if (typeof window.logError === 'function') window.logError(error, context);
    else console.error(error);
  }

  function routeOf(loc) {
    loc = loc || {};
    const parts = String(loc.pathname || '').split('/');
    const view = parts[1] === 'app' && parts[2] ? decodeURIComponent(parts[2]) : null;
    const segment = view && parts[3] ? decodeURIComponent(parts[3]) : null;
    return {
      view,
      pathname: loc.pathname || '',
      search: loc.search || '',
      hash: loc.hash || '',
      day: segment && /^\d{8}$/.test(segment) ? segment : null
    };
  }

  // Listeners are kept per target, type and phase, so adding the same
  // function twice is one listener, as the browser counts it.
  function listenerKey(type, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    return `${type}\u0000${capture ? 1 : 0}`;
  }

  function createContext(name, route) {
    const abort = window.SolStore.createAbort();
    const reads = window.SolStore.scope(abort.signal);
    const listeners = new Map();
    const timeouts = new Set();
    const intervals = new Set();
    const observers = new Set();
    const undo = [];
    const bodyClasses = new Set();
    const titleBefore = typeof document !== 'undefined' ? document.title : '';
    let titleSet = false;
    let disposed = false;

    const ctx = {
      view: name,
      route,
      signal: abort.signal,
      get disposed() { return disposed; },

      on(target, type, fn, options) {
        if (disposed || !target || typeof target.addEventListener !== 'function') return () => {};
        target.addEventListener(type, fn, options);
        let byTarget = listeners.get(target);
        if (!byTarget) listeners.set(target, (byTarget = new Map()));
        const key = listenerKey(type, options);
        let fns = byTarget.get(key);
        if (!fns) byTarget.set(key, (fns = new Map()));
        fns.set(fn, options);
        return () => {
          target.removeEventListener?.(type, fn, options);
          fns.delete(fn);
        };
      },

      setTimeout(fn, ms, ...args) {
        if (disposed) return 0;
        const id = window.setTimeout(() => {
          timeouts.delete(id);
          fn(...args);
        }, ms);
        timeouts.add(id);
        return id;
      },
      clearTimeout(id) {
        timeouts.delete(id);
        window.clearTimeout(id);
      },
      setInterval(fn, ms, ...args) {
        if (disposed) return 0;
        const id = window.setInterval(fn, ms, ...args);
        intervals.add(id);
        return id;
      },
      clearInterval(id) {
        intervals.delete(id);
        window.clearInterval(id);
      },
      // Runs while the page is in view (AppServices.visibleInterval).
      visibleInterval(fn, ms) {
        if (disposed) return () => {};
        const stop = window.AppServices?.visibleInterval?.(fn, ms);
        if (typeof stop === 'function') undo.push(stop);
        return stop || (() => {});
      },

      // An observer the view created; it is disconnected with the view.
      observe(observer) {
        if (!observer) return observer;
        if (disposed) {
          observer.disconnect?.();
          return observer;
        }
        observers.add(observer);
        const disconnect = observer.disconnect;
        if (typeof disconnect === 'function') {
          observer.disconnect = function () {
            observers.delete(observer);
            return disconnect.apply(this, arguments);
          };
        }
        return observer;
      },

      // The live stream, as `appEvents` offers it.
      listen(tract, optionsOrFn, fn) {
        const events = window.appEvents;
        if (disposed || !events?.listen) return () => {};
        const stop = fn === undefined ? events.listen(tract, optionsOrFn) : events.listen(tract, optionsOrFn, fn);
        if (typeof stop === 'function') undo.push(stop);
        return stop || (() => {});
      },
      onConnectionState(fn) {
        const events = window.appEvents;
        if (disposed || !events?.onConnectionState) return () => {};
        const stop = events.onConnectionState(fn);
        if (typeof stop === 'function') undo.push(stop);
        return stop || (() => {});
      },

      // A name on `window` that the view's markup or other code calls. The
      // value before the mount comes back with dispose.
      global(globalName, value) {
        if (disposed) return;
        const had = Object.prototype.hasOwnProperty.call(window, globalName);
        const before = window[globalName];
        window[globalName] = value;
        undo.push(() => {
          if (window[globalName] !== value) return;
          if (had) window[globalName] = before;
          else delete window[globalName];
        });
      },

      title(text) {
        if (disposed) return;
        titleSet = true;
        document.title = text;
      },

      bodyClass(className, on = true) {
        if (disposed && on) return;
        document.body?.classList?.toggle(className, !!on);
        if (on) bodyClasses.add(className);
        else bodyClasses.delete(className);
      },

      // The view's own history entries (a segment, a tab). They keep the
      // entry's place in the page's history, so going back and forward and
      // the scroll restore still know whose entry it is.
      history: {
        push(url) {
          if (window.solRouter?.viewEntry) window.solRouter.viewEntry('push', url);
          else window.history.pushState(null, '', url);
        },
        replace(url) {
          if (window.solRouter?.viewEntry) window.solRouter.viewEntry('replace', url);
          else window.history.replaceState(window.history.state, '', url);
        }
      },

      read: reads.read,
      fetch: reads.fetch,
      ready(state) { ctx.reportReady?.(state); },
      onDispose(fn) {
        if (disposed) {
          try { fn(); } catch (error) { logError(error, { context: 'view-dispose', view: name }); }
          return;
        }
        undo.push(fn);
      },

      // What the mount still holds; for tests and the leak check.
      held() {
        let listenerCount = 0;
        listeners.forEach((byTarget) => byTarget.forEach((fns) => { listenerCount += fns.size; }));
        return {
          listeners: listenerCount,
          timeouts: timeouts.size,
          intervals: intervals.size,
          observers: observers.size,
          undo: undo.length,
          bodyClasses: bodyClasses.size,
          reads: reads.inflightCount()
        };
      },

      teardown() {
        if (disposed) return;
        disposed = true;
        abort.abort();
        while (undo.length) {
          const fn = undo.pop();
          try { fn(); } catch (error) { logError(error, { context: 'view-dispose', view: name }); }
        }
        listeners.forEach((byTarget, target) => {
          byTarget.forEach((fns, key) => {
            const type = key.split('\u0000')[0];
            fns.forEach((options, fn) => target.removeEventListener?.(type, fn, options));
          });
        });
        listeners.clear();
        timeouts.forEach((id) => window.clearTimeout(id));
        timeouts.clear();
        intervals.forEach((id) => window.clearInterval(id));
        intervals.clear();
        observers.forEach((observer) => {
          try { observer.disconnect(); } catch (_) {}
        });
        observers.clear();
        bodyClasses.forEach((className) => document.body?.classList?.remove(className));
        bodyClasses.clear();
        if (titleSet && typeof document !== 'undefined') document.title = titleBefore;
      }
    };
    return ctx;
  }

  function register(name, definition) {
    if (!definition || typeof definition.mount !== 'function') {
      throw new Error(`View ${name} needs a mount function`);
    }
    views.set(name, definition);
  }

  function teardown(instance) {
    try {
      instance.handle?.dispose?.();
    } catch (error) {
      logError(error, { context: 'view-dispose', view: instance.name });
    }
    instance.ctx.teardown();
  }

  // Ends the view on the page, if any.
  function dispose() {
    const instance = current;
    current = null;
    if (instance) teardown(instance);
  }

  // Mounts a registered view into the page as it is now: its markup is in
  // place and the address is the view's. `reportReady` is the mount's
  // readiness report. Resolves with the mounted view, or rejects after taking
  // back whatever the mount added.
  async function mount(name, options = {}) {
    const definition = views.get(name);
    if (!definition) throw new Error(`View ${name} isn't registered`);
    dispose();
    const ctx = createContext(name, routeOf(options.location || window.location));
    const instance = {
      id: ++serial,
      name,
      ctx,
      handle: null,
      state: null,
      options,
      // A view that reads again on the stream's reconnect by itself.
      rereadsItself: !!definition.rereadsOnReconnect,
      readyWaiters: []
    };
    const report = options.reportReady || (() => {});
    ctx.reportReady = (state) => {
      if (instance.state === null && ['content', 'empty', 'error'].includes(state)) {
        instance.state = state;
        instance.readyWaiters.splice(0).forEach((resolve) => resolve(state));
      }
      report(state);
    };
    current = instance;
    try {
      const result = await definition.mount(ctx);
      instance.handle = typeof result === 'function' ? { dispose: result } : (result || {});
    } catch (error) {
      if (current === instance) current = null;
      teardown(instance);
      throw error;
    }
    if (ctx.disposed && current === instance) current = null;
    return instance;
  }

  // A change of parameters the mounted view takes in place. False when the
  // view can't, and the caller mounts it again instead.
  function update(params, options = {}) {
    const instance = current;
    if (!instance || typeof instance.handle?.update !== 'function') return false;
    instance.ctx.route = routeOf(options.location || window.location);
    instance.state = null;
    if (options.reportReady) {
      const report = options.reportReady;
      instance.ctx.reportReady = (state) => {
        if (instance.state === null && ['content', 'empty', 'error'].includes(state)) {
          instance.state = state;
          instance.readyWaiters.splice(0).forEach((resolve) => resolve(state));
        }
        report(state);
      };
    }
    instance.handle.update(params);
    return true;
  }

  // Resolves with the mounted view's readiness, or null after `ms`.
  function whenReady(instance, ms) {
    if (!instance) return Promise.resolve(null);
    if (instance.state !== null) return Promise.resolve(instance.state);
    return new Promise((resolve) => {
      instance.readyWaiters.push(resolve);
      if (ms !== undefined) window.setTimeout(() => resolve(null), ms);
    });
  }

  window.SolViews = {
    register,
    registered: (name) => views.has(name),
    mount,
    update,
    dispose,
    whenReady,
    current: () => current,
    routeOf
  };
})();
