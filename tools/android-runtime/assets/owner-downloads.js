// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
(() => {
  'use strict';
  const token = window.__journalOwnerToken;
  // Keep the actual Blob until its ordinary revocation. A click retains that
  // object synchronously, even when its caller revokes the URL immediately.
  const blobs = new Map();
  const create = URL.createObjectURL.bind(URL);
  const revoke = URL.revokeObjectURL.bind(URL);
  URL.createObjectURL = object => {
    const url = create(object);
    if (object instanceof Blob) blobs.set(url, object);
    return url;
  };
  URL.revokeObjectURL = url => { blobs.delete(String(url));revoke(url); };
  let retired = false;
  async function save(blob, name) {
    const id = JournalOwner.beginDownload(token, name, blob.type, blob.size);
    if (!id) return;
    let reader;
    let complete = false;
    try {
      reader = blob.stream().getReader();
      while (!retired) {
        const next = await reader.read();
        if (next.done) { complete = true;break; }
        for (let offset = 0; offset < next.value.length; offset += 32768) {
          const piece = next.value.subarray(offset, offset + 32768);
          let binary = '';
          for (const byte of piece) binary += String.fromCharCode(byte);
          // Acknowledgement follows the disk write, bounding outstanding data.
          if (!JournalOwner.downloadChunk(token, id, btoa(binary))) return;
        }
      }
    } finally {
      try { await reader?.cancel(); }
      finally { JournalOwner.endDownload(token, id, complete); }
    }
  }
  document.addEventListener('click', event => {
    const link = event.target.closest?.('a[download]');
    if (!link || !link.href.startsWith('blob:')) return;
    const blob = blobs.get(link.href);
    event.preventDefault();
    if (!blob) {
      window.alert("couldn't save this file. try downloading it again.");
      return;
    }
    save(blob, link.download || 'journal-export').catch(() => {
      if (!retired) window.alert("couldn't save this file. try downloading it again.");
    });
  }, true);
  window.addEventListener('pagehide', () => { retired = true;blobs.clear(); });
})();
