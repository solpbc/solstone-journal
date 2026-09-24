// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.svg': 'image/svg+xml',
  '.wasm': 'application/wasm',
  '.txt': 'text/plain; charset=utf-8',
};

export function startStaticServer({ roots = [], port = 0, host = '127.0.0.1' } = {}) {
  return new Promise((resolve, reject) => {
    const server = http.createServer((req, res) => {
      const url = new URL(req.url, `http://${host}`);
      let pathname = decodeURIComponent(url.pathname);
      // Strip leading slashes and normalize
      pathname = path.normalize(pathname).replace(/^(\.\.[\/\\])+/, '');
      if (pathname.startsWith('/') || pathname.startsWith('\\')) {
        pathname = pathname.slice(1);
      }

      let filePath = null;
      for (const root of roots) {
        const candidate = path.join(root, pathname);
        try {
          if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
            filePath = candidate;
            break;
          }
        } catch (_) {}
      }

      if (!filePath) {
        res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' });
        res.end(`Not Found: ${url.pathname}`);
        return;
      }

      const ext = path.extname(filePath).toLowerCase();
      const contentType = MIME_TYPES[ext] || 'application/octet-stream';

      try {
        const content = fs.readFileSync(filePath);
        res.writeHead(200, {
          'Content-Type': contentType,
          'Content-Length': content.length,
          'Cache-Control': 'no-store',
        });
        res.end(content);
      } catch (readErr) {
        res.writeHead(500, { 'Content-Type': 'text/plain; charset=utf-8' });
        res.end(`Internal Server Error: ${readErr.message}`);
      }
    });

    server.once('error', (err) => {
      reject(new Error(`Static server bind failure: ${err.message}`));
    });

    server.listen(port, host, () => {
      const addr = server.address();
      const boundPort = addr.port;
      resolve({
        server,
        port: boundPort,
        origin: `http://${host}:${boundPort}`,
        close: () => new Promise((res) => server.close(res)),
      });
    });
  });
}
