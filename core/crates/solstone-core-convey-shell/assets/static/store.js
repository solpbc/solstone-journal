// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The reads a view makes, owned by the view that made them.
//
// A view on the view contract reads through its mount's scope. Every read is
// keyed by its absolute address. While a read of a key is in flight, a second
// read of the same key from the same mount shares it rather than asking again.
// A read asked because something changed (`fresh: true`) wants an answer newer
// than the one on its way, so it is asked of its own and becomes the key's
// read in flight; the older one still answers whoever asked for it.
// A read belongs to its mount: when the mount ends, the request is aborted and
// its answer, or its failure, is never handed back, so a view that has gone
// can't draw. Everything is kept in memory only; nothing goes to web storage.
//
// Reads still go out through `apiJson` and `fetch`, so the read bounds, the
// transport retries and the adoption of a read the page started early all
// apply exactly as they do to any other read.
(function () {
  const NEVER = new Promise(() => {});

  // A signal the page can abort, even in a host without AbortController.
  function createAbort() {
    if (typeof AbortController === 'function') return new AbortController();
    const listeners = new Set();
    const signal = {
      aborted: false,
      addEventListener(type, fn) { if (type === 'abort') listeners.add(fn); },
      removeEventListener(type, fn) { listeners.delete(fn); }
    };
    return {
      signal,
      abort() {
        if (signal.aborted) return;
        signal.aborted = true;
        listeners.forEach((fn) => { try { fn(); } catch (_) {} });
        listeners.clear();
      }
    };
  }

  // One signal that aborts when either does.
  function eitherSignal(first, second) {
    if (!first) return second;
    if (!second || first === second) return first;
    if (typeof AbortSignal === 'function' && typeof AbortSignal.any === 'function') {
      try { return AbortSignal.any([first, second]); } catch (_) {}
    }
    const joined = createAbort();
    const abort = () => joined.abort();
    if (first.aborted || second.aborted) abort();
    first.addEventListener('abort', abort, { once: true });
    second.addEventListener('abort', abort, { once: true });
    return joined.signal;
  }

  function keyFor(url) {
    const text = String(url);
    try {
      return new URL(text, window.location.href).href;
    } catch (_) {
      const origin = window.location?.origin || '';
      return text.startsWith('/') ? origin + text : text;
    }
  }

  function isRead(init) {
    return String(init?.method || 'GET').toUpperCase() === 'GET' && !init?.body;
  }

  // The last answer and failure per key, for diagnostics and tests. Bounded:
  // the oldest key goes first.
  const LAST_LIMIT = 64;
  const last = new Map();

  function remember(key, fields) {
    last.delete(key);
    last.set(key, Object.assign({ at: Date.now() }, fields));
    while (last.size > LAST_LIMIT) last.delete(last.keys().next().value);
  }

  // Options that change what is asked mean a read of its own.
  function shareable(opts) {
    if (!opts) return true;
    return Object.keys(opts).every((name) => name === 'signal' || name === 'noAuthRedirect');
  }

  function withoutFresh(opts) {
    if (!opts || !('fresh' in opts)) return opts;
    const rest = Object.assign({}, opts);
    delete rest.fresh;
    return rest;
  }

  // One mount's reads. `signal` ends them all.
  function scope(signal) {
    const inflight = new Map();
    let closed = false;
    const ended = () => closed || !!signal?.aborted;
    signal?.addEventListener?.('abort', () => {
      closed = true;
      inflight.clear();
    }, { once: true });

    function read(url, opts) {
      // A save is never cut short by a view going away: its outcome would be
      // unknown. It goes out exactly as it would without a scope.
      if (!isRead(opts)) return window.apiJson(url, withoutFresh(opts));
      if (ended()) return NEVER;
      const key = keyFor(url);
      const fresh = !!opts?.fresh;
      opts = withoutFresh(opts);
      const share = shareable(opts);
      if (share && !fresh && inflight.has(key)) return inflight.get(key);
      const request = window.apiJson(url, Object.assign({}, opts, {
        signal: eitherSignal(opts?.signal || null, signal)
      }));
      const settled = request.then(
        (value) => {
          if (inflight.get(key) === settled) inflight.delete(key);
          remember(key, { result: value, error: null });
          return ended() ? NEVER : value;
        },
        (error) => {
          if (inflight.get(key) === settled) inflight.delete(key);
          remember(key, { result: null, error });
          return ended() ? NEVER : Promise.reject(error);
        }
      );
      if (share) inflight.set(key, settled);
      return settled;
    }

    // A plain fetch. A body can be read only once, so these are not shared,
    // but they belong to the mount like any read.
    function fetchFor(input, init) {
      const target = typeof window.fetch === 'function' ? window.fetch : fetch;
      if (!isRead(init) || (typeof Request === 'function' && input instanceof Request)) {
        return target(input, init);
      }
      if (ended()) return NEVER;
      const key = keyFor(input);
      const request = target(input, Object.assign({}, init, {
        signal: eitherSignal(init?.signal || null, signal)
      }));
      return request.then(
        (response) => {
          remember(key, { result: { status: response?.status }, error: null });
          return ended() ? NEVER : response;
        },
        (error) => {
          remember(key, { result: null, error });
          return ended() ? NEVER : Promise.reject(error);
        }
      );
    }

    return {
      read,
      fetch: fetchFor,
      inflightCount: () => inflight.size
    };
  }

  window.SolStore = {
    NEVER,
    scope,
    createAbort,
    eitherSignal,
    keyFor,
    last(url) {
      const entry = last.get(keyFor(url));
      return entry ? Object.assign({}, entry) : null;
    },
    size: () => last.size
  };
})();
