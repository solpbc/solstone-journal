#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Serve the import host fixtures on loopback and optionally open the Linux viewer."""
import argparse
import contextlib
import functools
import http.server
from pathlib import Path
import subprocess
import threading
from urllib.parse import urlsplit


class Fixtures(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if urlsplit(self.path).path == "/_bridge/bootstrap":
            self.send_response(303)
            self.send_header("Location", "/conformance/index.html")
            self.end_headers()
            return
        super().do_GET()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--viewer", type=Path, help="installed Linux journal viewer launcher")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    handler = functools.partial(Fixtures, directory=str(root))
    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
        origin = f"http://127.0.0.1:{server.server_port}"
        print(f"{origin}/conformance/index.html", flush=True)
        if args.viewer is None:
            server.serve_forever()
            return
        with contextlib.ExitStack() as cleanup:
            threading.Thread(target=server.serve_forever, daemon=True).start()
            cleanup.callback(server.shutdown)
            viewer = cleanup.enter_context(subprocess.Popen([str(args.viewer.resolve())], stdin=subprocess.PIPE,
                                                           stdout=subprocess.PIPE))
            cleanup.callback(viewer.terminate)
            viewer.stdin.write(f"{origin}/_bridge/bootstrap?token=fixture\n".encode())
            viewer.stdin.flush()
            if viewer.stdout.read(1) != b"+":
                raise SystemExit("Viewer did not open the test journal.")
            raise SystemExit(viewer.wait())


if __name__ == "__main__":
    main()
