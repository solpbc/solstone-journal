// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

export class CdpClient {
  constructor(ws) {
    this.ws = ws;
    this.nextId = 0;
    this.callbacks = new Map();
    this.eventListeners = new Map();

    ws.addEventListener('message', (event) => {
      let data;
      try {
        data = JSON.parse(event.data);
      } catch (_) {
        return;
      }

      if (data.id !== undefined && this.callbacks.has(data.id)) {
        const { resolve, reject } = this.callbacks.get(data.id);
        this.callbacks.delete(data.id);
        if (data.error) {
          reject(new Error(data.error.message || JSON.stringify(data.error)));
        } else {
          resolve(data.result);
        }
      } else if (data.method) {
        const listeners = this.eventListeners.get(data.method) || [];
        listeners.forEach((fn) => {
          try {
            fn(data.params);
          } catch (_) {}
        });
      }
    });

    ws.addEventListener('error', (err) => {
      this.callbacks.forEach(({ reject }) => reject(err));
      this.callbacks.clear();
    });

    ws.addEventListener('close', () => {
      this.callbacks.forEach(({ reject }) => reject(new Error('CDP WebSocket closed')));
      this.callbacks.clear();
      const listeners = this.eventListeners.get('close') || [];
      listeners.forEach((fn) => {
        try {
          fn();
        } catch (_) {}
      });
    });
  }

  send(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      this.callbacks.set(id, { resolve, reject });
      try {
        this.ws.send(JSON.stringify({ id, method, params }));
      } catch (err) {
        this.callbacks.delete(id);
        reject(err);
      }
    });
  }

  on(event, handler) {
    if (!this.eventListeners.has(event)) {
      this.eventListeners.set(event, []);
    }
    this.eventListeners.get(event).push(handler);
  }

  close() {
    try {
      this.ws.close();
    } catch (_) {}
  }
}

export function connectWebSocket(url) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    const onOpen = () => {
      cleanup();
      resolve(new CdpClient(ws));
    };
    const onError = (err) => {
      cleanup();
      reject(err);
    };
    const cleanup = () => {
      ws.removeEventListener('open', onOpen);
      ws.removeEventListener('error', onError);
    };
    ws.addEventListener('open', onOpen);
    ws.addEventListener('error', onError);
  });
}
