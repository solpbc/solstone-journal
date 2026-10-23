// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
(() => {
  'use strict';
  const token = window.__journalOwnerToken;
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
  window.fetch = async (input, options) => {
    const request = new Request(input, options);
    const url = new URL(request.url, location.href);
    if (url.origin !== location.origin || url.username || url.password) throw new TypeError('private journal address required');
    if (request.signal.aborted) throw new DOMException('aborted', 'AbortError');
    const mediaType = request.headers.get('content-type') || '';
    if (/^multipart\/|^application\/octet-stream/i.test(mediaType)) throw new TypeError('file uploads are unavailable in this build');
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
