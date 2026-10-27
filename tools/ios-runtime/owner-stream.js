// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
(() => {
  'use strict';
  const stallMessage = 'the transfer stopped before it finished. check the result before trying again.';
  const lostMessage = 'journal response unavailable; check the result before trying again';
  const unsentMessage = 'this file couldn\'t be sent. choose it again, and check the result before retrying.';
  const original = window.fetch.bind(window);
  const pending = new Map();
  let sequence = 0;
  function latin1(bytes) {
    let text = '';
    for (let index = 0; index < bytes.length; index += 1) text += String.fromCharCode(bytes[index]);
    return text;
  }
  function sameJournal(url) {
    if (url.username || url.password) return false;
    return url.protocol === 'journal-owner:' && url.hostname === 'local';
  }
  function call(message) {
    const handler = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.journalOwner;
    if (!handler) return Promise.reject(new TypeError(lostMessage));
    return Promise.resolve(handler.postMessage(message)).then((reply) => {
      if (typeof reply === 'string') return JSON.parse(reply);
      return reply || {};
    });
  }
  const authorityReady = call({ op: 'bind' }).then((reply) => {
    const issued = Number(reply && reply.authority);
    if (!reply || reply.ok !== true || !Number.isSafeInteger(issued) || issued <= 0) {
      throw new TypeError(lostMessage);
    }
    return issued;
  });
  function bindAuthority() {
    return authorityReady;
  }
  function readWithin(reader, signal, budgetMs) {
    return new Promise((resolve, reject) => {
      let settled = false;
      const stop = (settle, value) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        signal.removeEventListener('abort', onAbort);
        settle(value);
      };
      const timer = setTimeout(() => {
        try { reader.cancel(); } catch (error) { /* abandoned with the stall */ }
        stop(reject, new TypeError(stallMessage));
      }, budgetMs);
      const onAbort = () => {
        try { reader.cancel(); } catch (error) { /* caller already cancelled */ }
        stop(reject, new DOMException('aborted; check the result before retrying', 'AbortError'));
      };
      if (signal.aborted) { onAbort(); return; }
      signal.addEventListener('abort', onAbort);
      try {
        reader.read().then((chunk) => stop(resolve, chunk), (error) => stop(reject, error));
      } catch (error) {
        stop(reject, error);
      }
    });
  }
  function responseFrom(reply, url) {
    if (reply.error) throw new TypeError(reply.error);
    const bytes = Uint8Array.from(atob(reply.body || ''), (c) => c.charCodeAt(0));
    const response = new Response([204, 205, 304].includes(reply.status) ? null : bytes, {
      status: reply.status, headers: reply.headers,
    });
    Object.defineProperties(response, {
      url: { value: reply.url || url.href }, redirected: { value: (reply.url || url.href) !== url.href },
    });
    return response;
  }
  async function streamBody(request, url) {
    if (request.signal.aborted) throw new DOMException('aborted', 'AbortError');
    const documentAuthority = await bindAuthority();
    const id = String(++sequence);
    const opened = await call({
      op: 'open', authority: documentAuthority, id, url: url.href, method: request.method,
      headers: Array.from(request.headers),
    });
    if (!opened.ok) throw new TypeError(opened.error || lostMessage);
    let aborted = false;
    let reader = null;
    const abort = () => {
      if (aborted) return;
      aborted = true;
      if (reader) {
        try { reader.cancel(); } catch (error) { /* The cancel still names this document. */ }
      }
      call({ op: 'cancel', authority: documentAuthority, id }).catch(() => {});
    };
    pending.set(id, abort);
    request.signal.addEventListener('abort', abort, { once: true });
    const fail = (error) => { pending.delete(id); throw error; };
    try {
      const body = request.body;
      reader = body && typeof body.getReader === 'function' ? body.getReader() : null;
      if (body && !reader) fail(new TypeError(unsentMessage));
      let lastAccepted = Date.now();
      while (reader) {
        if (aborted || request.signal.aborted) fail(new DOMException('aborted; check the result before retrying', 'AbortError'));
        const budget = 60000 - (Date.now() - lastAccepted);
        if (budget <= 0) fail(new TypeError(stallMessage));
        const chunk = await readWithin(reader, request.signal, budget);
        if (chunk.done) break;
        const slice = chunk.value;
        for (let start = 0; start < slice.length; start += 8192) {
          const piece = slice.subarray(start, start + 8192);
          while (true) {
            if (aborted || request.signal.aborted) fail(new DOMException('aborted; check the result before retrying', 'AbortError'));
            if (Date.now() - lastAccepted > 60000) fail(new TypeError(stallMessage));
            const step = await call({ op: 'chunk', authority: documentAuthority, id, chunk: latin1(piece) });
            if (step.accepted) { lastAccepted = Date.now(); break; }
            if (step.busy) { await new Promise((resolve) => setTimeout(resolve, 0)); continue; }
            fail(new TypeError(step.error || lostMessage));
          }
        }
      }
      while (true) {
        if (aborted || request.signal.aborted) fail(new DOMException('aborted; check the result before retrying', 'AbortError'));
        const step = await call({ op: 'finish', authority: documentAuthority, id });
        if (step.pending) { await new Promise((resolve) => setTimeout(resolve, 0)); continue; }
        pending.delete(id);
        return responseFrom(step, url);
      }
    } catch (error) {
      if (!aborted) abort();
      throw error;
    } finally {
      pending.delete(id);
      request.signal.removeEventListener('abort', abort);
    }
  }
  window.fetch = async (input, options) => {
    const request = new Request(input, options);
    const mediaType = request.headers.get('content-type') || '';
    if (!/^multipart\/|^application\/octet-stream/i.test(mediaType)) return original(input, options);
    const url = new URL(request.url, location.href);
    if (!sameJournal(url)) throw new TypeError('private journal address required');
    return streamBody(request, url);
  };
  const folders = new Map();
  window.__journalFolderSelected = (authority, id) => {
    const pending = folders.get(id);
    if (!pending || pending.authority !== authority) return false;
    try { if (pending.selected?.() !== true) return false; } catch (error) { return false; }
    pending.chosen = true;
    return true;
  };
  function folderChoice(op, source, reference, selected, fields = {}) {
      if (folders.size) return Promise.resolve({ state: 'refused', error: lostMessage });
      const id = crypto.randomUUID();
      return new Promise((resolve, reject) => {
        const pending = { resolve, reject, selected, reference, authority: 0, chosen: false, dispatched: false };
        folders.set(id, pending);
        bindAuthority().then((authority) => {
          if (folders.get(id) !== pending) return;
          pending.authority = authority;
          pending.dispatched = true;
          return call({ op, authority, id, source, ...fields, client_item_id: reference }).then((reply) => {
            if (folders.get(id) !== pending) return;
            folders.delete(id);
            resolve(reply);
          });
        }).catch((error) => {
          if (folders.get(id) !== pending) return;
          folders.delete(id);
          if (pending.dispatched) resolve({ state: 'outcome_unknown', client_item_id: reference });
          else reject(error);
        });
      });
  }
  window.JournalOwnerFolders = {
    cancel() {
      for (const [id, pending] of folders) {
        folders.delete(id);
        if (pending.authority) call({ op: 'cancelFolder', authority: pending.authority, id }).catch(() => {});
        pending.resolve({ state: pending.dispatched ? 'outcome_unknown' : 'cancelled', client_item_id: pending.reference });
      }
    },
    choose(source, reference, selected) {
      return folderChoice('chooseFolder', source, reference, selected);
    },
  };
  window.JournalOwnerSources = {
    change(source, action, expectedIdentity, reference, selected) {
      return folderChoice('chooseSource', source, reference, selected, { action, expected_identity: expectedIdentity });
    },
    cancel() { window.JournalOwnerFolders.cancel(); },
  };
  window.addEventListener('pagehide', () => {
    window.JournalOwnerFolders.cancel();
    for (const abort of Array.from(pending.values())) abort();
  });
})();
