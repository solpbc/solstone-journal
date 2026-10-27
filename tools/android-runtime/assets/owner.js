// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
(() => {
  'use strict';
  const token = window.__journalOwnerToken;
  const folders = new Map();
  window.__journalFolderSelected = (page, id) => {
    if (page !== token) return false;
    const waiter = folders.get(id);
    if (!waiter) return false;
    try { return waiter.selected?.() === true; } catch (error) { return false; }
  };
  window.__journalFolderReply = (page, id, reply) => {
    if (page !== token) return;
    const waiter = folders.get(id);
    if (!waiter) return;
    folders.delete(id);
    waiter.resolve(reply);
  };
  window.JournalOwnerFolders = {
    cancel() {
      for (const [id, waiter] of folders) {
        try { JournalOwner.cancelFolder(token, id); } catch (error) { /* Delivery is unconfirmed. */ }
        waiter.resolve({ state: waiter.dispatched ? 'outcome_unknown' : 'cancelled', client_item_id: waiter.reference });
        folders.delete(id);
      }
    },
    choose(source, reference, selected) {
      if (folders.size) return Promise.resolve({ state: 'refused', error: 'journal controls busy; check health before retrying' });
      const id = crypto.randomUUID();
      return new Promise((resolve) => {
        const waiter = { resolve, selected, reference, dispatched: false };
        folders.set(id, waiter);
        try {
          waiter.dispatched = true;
          JournalOwner.chooseFolder(token, id, JSON.stringify({ source, client_item_id: reference }));
        }
        catch (error) { folders.delete(id); resolve({ state: 'outcome_unknown', client_item_id: reference }); }
      });
    },
  };
  // Only the native picker can supply private URI material. The shared card
  // supplies a closed source/action and its displayed identity for admission.
  window.JournalOwnerSources = {
    change(source, action, expectedIdentity, reference, selected) {
      if (folders.size) return Promise.resolve({ state: 'refused' });
      const id=crypto.randomUUID();
      return new Promise((resolve) => {
        const waiter={resolve,selected,reference,dispatched:false};
        folders.set(id,waiter);
        try {
          waiter.dispatched=true;
          JournalOwner.chooseSource(token,id,JSON.stringify({source,action,expected_identity:expectedIdentity,client_item_id:reference}));
        } catch (error) {
          folders.delete(id);
          resolve({state:'outcome_unknown',client_item_id:reference});
        }
      });
    },
    cancel() { window.JournalOwnerFolders.cancel(); },
  };
  const pending = new Map();
  let sequence = 0;
  window.__journalOwnerReply = (page, id, reply) => {
    if (page !== token) return;
    const waiter = pending.get(id);
    if (!waiter) return;
    pending.delete(id);
    clearTimeout(waiter.timeout);
    waiter.signal?.removeEventListener('abort', waiter.abort);
    if (reply.error) { waiter.reject(new TypeError(reply.error)); return; }
    const bytes = Uint8Array.from(atob(reply.body), c => c.charCodeAt(0));
    const response = new Response([204, 205, 304].includes(reply.status) ? null : bytes, {
      status: reply.status, headers: reply.headers,
    });
    Object.defineProperties(response, {
      url: {value: reply.url}, redirected: {value: reply.url !== waiter.url},
    });
    waiter.resolve(response);
  };
  const stallMessage = 'the transfer stopped before it finished. check the result before trying again.';
  const lostMessage = 'journal response unavailable; check the result before trying again';
  const unsentMessage = 'this file couldn\'t be sent. choose it again, and check the result before retrying.';
  function latin1(bytes) {
    let text = '';
    for (let index = 0; index < bytes.length; index += 1) text += String.fromCharCode(bytes[index]);
    return text;
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
    const bytes = Uint8Array.from(atob(reply.body || ''), c => c.charCodeAt(0));
    const response = new Response([204, 205, 304].includes(reply.status) ? null : bytes, {
      status: reply.status, headers: reply.headers,
    });
    Object.defineProperties(response, {
      url: {value: reply.url || url.href}, redirected: {value: (reply.url || url.href) !== url.href},
    });
    return response;
  }
  async function streamBody(request, url) {
    if (request.signal.aborted) throw new DOMException('aborted', 'AbortError');
    const id = String(++sequence);
    let opened;
    try {
      opened = JSON.parse(JournalOwner.openStream(JSON.stringify({
        token, id, url: url.href, method: request.method, headers: Array.from(request.headers),
      })));
    } catch (error) {
      throw new TypeError(lostMessage);
    }
    if (!opened.ok) throw new TypeError(opened.error || lostMessage);
    let aborted = false;
    let reader = null;
    const abort = () => {
      if (aborted) return;
      aborted = true;
      if (reader) {
        try { reader.cancel(); } catch (error) { /* The cancel still names this page. */ }
      }
      JournalOwner.cancel(token, id);
    };
    pending.set(id, {abort, timeout: 0, signal: request.signal, url: url.href});
    request.signal.addEventListener('abort', abort, {once: true});
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
            const step = JSON.parse(JournalOwner.writeChunk(token, id, latin1(piece)));
            if (step.accepted) { lastAccepted = Date.now(); break; }
            if (step.busy) { await new Promise(resolve => setTimeout(resolve, 0)); continue; }
            fail(new TypeError(step.error || lostMessage));
          }
        }
      }
      while (true) {
        if (aborted || request.signal.aborted) fail(new DOMException('aborted; check the result before retrying', 'AbortError'));
        const step = JSON.parse(JournalOwner.finishStream(token, id));
        if (step.pending) { await new Promise(resolve => setTimeout(resolve, 0)); continue; }
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
    const url = new URL(request.url, location.href);
    if (url.origin !== location.origin || url.username || url.password) throw new TypeError('private journal address required');
    if (request.signal.aborted) throw new DOMException('aborted', 'AbortError');
    const mediaType = request.headers.get('content-type') || '';
    if (/^multipart\/|^application\/octet-stream/i.test(mediaType)) return streamBody(request, url);
    const body = ['GET', 'HEAD'].includes(request.method) ? '' : await request.text();
    if (request.signal.aborted) throw new DOMException('aborted', 'AbortError');
    if (new TextEncoder().encode(body).length > 256 * 1024) throw new TypeError('journal request too large');
    const id = String(++sequence);
    return new Promise((resolve, reject) => {
      const abort = () => {
        const waiter = pending.get(id);
        if (!waiter) return;
        pending.delete(id); clearTimeout(waiter.timeout);
        request.signal.removeEventListener('abort', abort);
        JournalOwner.cancel(token, id);
        reject(new DOMException('aborted; check the result before retrying', 'AbortError'));
      };
      const timeout = setTimeout(abort, 75000);
      pending.set(id, {resolve, reject, timeout, abort, signal: request.signal, url: url.href});
      request.signal.addEventListener('abort', abort, {once: true});
      if (request.signal.aborted) { abort(); return; }
      JournalOwner.fetch(JSON.stringify({token, id, url: url.href, method: request.method,
        headers: Array.from(request.headers), body}));
    });
  };
  window.addEventListener('pagehide', () => {
    for (const waiter of Array.from(pending.values())) waiter.abort();
  });
})();
