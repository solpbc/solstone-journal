"""Loopback-only REST v2 adapter over a synthetic local restic repository,
with injected delete faults for qualification.

Counts bytes actually returned/accepted, every request, and lock-object lifetime.
This is a measurement adapter, not a cloud-provider emulator.
"""
import json, os, re, shutil, socket, threading, time, urllib.parse
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from pathlib import Path

class Server(ThreadingHTTPServer):
    daemon_threads = True
    def __init__(self, repo, log):
        self.repo = Path(repo).resolve()
        assert str(self.repo).startswith('/var/tmp/')
        self.fault = None  # 'refuse-delete' | 'fake-delete' for data files
        self.events = []
        self.guard = threading.Lock()
        self.idle = threading.Condition(self.guard)
        self.active = 0
        self.log = open(log, 'a', buffering=1)
        super().__init__(('127.0.0.1', 0), Handler)
    def event(self, e):
        with self.guard:
            self.events.append(e)
            self.log.write(json.dumps(e) + '\n')
    def wait_idle(self):
        deadline=time.monotonic()+30
        with self.idle:
            while self.active:
                remaining=deadline-time.monotonic()
                if remaining<=0:raise TimeoutError('REST handlers did not finish logging')
                self.idle.wait(remaining)

class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    def log_message(self, *args): pass
    def route(self):
        url = urllib.parse.urlsplit(self.path)
        parts = [urllib.parse.unquote(x) for x in url.path.split('/') if x]
        if not parts or parts[0] != 'repo': raise ValueError('outside repo')
        parts = parts[1:]
        if any('/' in x or x in ('.', '..') for x in parts): raise ValueError('unsafe')
        typ = parts[0] if parts else ''
        if typ not in ('', 'config', 'keys', 'locks', 'snapshots', 'index', 'data'):
            raise ValueError('type')
        p = self.server.repo.joinpath(*parts)
        if typ == 'data' and len(parts) == 2:
            if not re.fullmatch('[0-9a-f]{64}', parts[1]): raise ValueError('id')
            p = self.server.repo / 'data' / parts[1][:2] / parts[1]
        return p, typ, len(parts), urllib.parse.parse_qs(url.query)
    def handle_one(self):
        with self.server.idle:self.server.active+=1
        start = time.monotonic(); up = down = 0; status = 500
        typ = ''; name = ''; listing = False
        try:
            p, typ, depth, query = self.route(); name = p.name
            if self.command == 'POST' and depth == 0 and 'create' in query:
                self.reply(200, b''); status = 200
            elif self.command == 'GET' and depth == 1 and typ != 'config':
                listing = True
                files = sorted(p.rglob('*')) if p.exists() else []
                body = json.dumps([{'name': x.name, 'size': x.stat().st_size}
                                   for x in files if x.is_file()]).encode()
                self.reply(200, body, 'application/vnd.x.restic.rest.v2'); down = len(body); status = 200
            elif self.command in ('GET', 'HEAD'):
                if not p.is_file(): self.reply(404, b''); status = 404
                else:
                    size = p.stat().st_size; first = 0; last = size - 1; status = 200
                    h = self.headers.get('Range')
                    if h:
                        m = re.fullmatch(r'bytes=(\d+)-(\d*)', h)
                        if not m: raise ValueError('range')
                        first = int(m[1]); last = min(int(m[2]) if m[2] else last, last)
                        if first > last: self.reply(416, b''); status = 416
                        else: status = 206
                    if status != 416:
                        self.send_response(status)
                        self.send_header('Content-Length', str(last-first+1))
                        if status == 206: self.send_header('Content-Range', f'bytes {first}-{last}/{size}')
                        self.end_headers()
                        if self.command == 'GET':
                            with p.open('rb') as f:
                                f.seek(first); left = last-first+1
                                while left:
                                    b = f.read(min(left, 1024*1024))
                                    if not b: raise OSError('short file')
                                    self.wfile.write(b); down += len(b); left -= len(b)
            elif self.command in ('POST', 'PUT'):
                n = int(self.headers.get('Content-Length', '0'))
                if self.headers.get('Transfer-Encoding'): raise ValueError('chunked unsupported')
                p.parent.mkdir(parents=True, exist_ok=True)
                tmp = p.with_name(p.name + '.measure-tmp')
                with tmp.open('xb') as f:
                    left = n
                    while left:
                        b = self.rfile.read(min(left, 1024*1024))
                        if not b: raise OSError('short upload')
                        f.write(b); up += len(b); left -= len(b)
                os.replace(tmp, p)
                self.reply(200, b''); status = 200
            elif self.command == 'DELETE' and typ == 'data' and self.server.fault == 'refuse-delete':
                self.reply(403, b''); status = 403
            elif self.command == 'DELETE' and typ == 'data' and self.server.fault == 'fake-delete':
                self.reply(200, b''); status = 200
            elif self.command == 'DELETE':
                if p.is_file(): p.unlink(); self.reply(200, b''); status = 200
                else: self.reply(404, b''); status = 404
            else: self.reply(405, b''); status = 405
        except (OSError, ValueError) as ex:
            self.close_connection = True
            try: self.reply(500, str(ex).encode())
            except OSError: pass
            status = 500
        finally:
            self.server.event(dict(start=start, end=time.monotonic(), method=self.command,
                type=typ, name=name, list=listing, status=status, uploaded=up, downloaded=down))
            with self.server.idle:
                self.server.active-=1
                self.server.idle.notify_all()
    do_GET = do_HEAD = do_POST = do_PUT = do_DELETE = handle_one
    def reply(self, status, body, content_type='application/octet-stream'):
        self.send_response(status); self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(body))); self.end_headers()
        if self.command != 'HEAD': self.wfile.write(body)

def start(repo, log):
    s = Server(repo, log)
    t = threading.Thread(target=s.serve_forever, daemon=True); t.start()
    return s, f'rest:http://127.0.0.1:{s.server_port}/repo'

def summarize(events):
    counts = {}; alive = {}; intervals = []
    for e in events:
        key = e['method'] + ('-LIST' if e['list'] else '')
        counts[key] = counts.get(key, 0) + 1
        if e['type'] == 'locks' and e['status'] == 200:
            if e['method'] in ('POST', 'PUT'): alive[e['name']] = e['end']
            elif e['method'] == 'DELETE' and e['name'] in alive:
                intervals.append((alive.pop(e['name']), e['start']))
    if alive: raise RuntimeError('measurement left lock objects')
    merged = []
    for a, b in sorted(intervals):
        if merged and a <= merged[-1][1]: merged[-1] = (merged[-1][0], max(b, merged[-1][1]))
        else: merged.append((a,b))
    return dict(downloaded=sum(e['downloaded'] for e in events), uploaded=sum(e['uploaded'] for e in events),
        requests=len(events), request_methods=counts, lock_seconds=sum(b-a for a,b in merged),
        lock_intervals=len(intervals), errors=[e for e in events if e['status'] >= 400])
