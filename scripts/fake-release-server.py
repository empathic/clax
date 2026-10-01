#!/usr/bin/env python3
"""A stand-in for GitHub release downloads, for scripts/test-install.sh.

Usage: fake-release-server.py <root> <request log> <port file>

Serves <root>/good/<path> at /<mode>/<path> and <root>/wrong/<path> at
/wrong/<path>, and appends every request path to <request log>. Modes:
  ok       the file as it is; /ok/latest redirects to /ok/tag/v$FAKE_LATEST
  none     404 for everything
  badsum   SHA256SUMS with every checksum zeroed
  partial  archives: the full Content-Length, half the body, then a close
  slow     waits 60 s before answering
  wrong    the files under <root>/wrong
Binds 127.0.0.1 on a port the kernel picks and writes it to <port file>.
"""
import http.server
import os
import re
import sys
import threading
import time

ROOT, LOG, PORT_FILE = sys.argv[1:4]
LOG_LOCK = threading.Lock()


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        with LOG_LOCK, open(LOG, "a") as f:
            f.write(self.path + "\n")
        mode, _, rest = self.path.lstrip("/").partition("/")
        if rest == "latest":
            self.send_response(302)
            self.send_header("Location", f"/{mode}/tag/v{os.environ.get('FAKE_LATEST', '0.0.0')}")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if rest.startswith("tag/"):
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"ok")
            return
        tree = "wrong" if mode == "wrong" else "good"
        path = os.path.join(ROOT, tree, rest)
        if mode == "slow":
            time.sleep(60)
        if mode == "none" or ".." in rest or not os.path.isfile(path):
            self.send_error(404)
            return
        with open(path, "rb") as f:
            data = f.read()
        if mode == "badsum" and rest.endswith("SHA256SUMS"):
            data = re.sub(rb"^[0-9a-f]{64}", b"0" * 64, data, flags=re.M)
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        if mode == "partial" and rest.endswith(".tar.gz"):
            self.wfile.write(data[: len(data) // 2])
            self.wfile.flush()
            self.close_connection = True
            return
        self.wfile.write(data)


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
server.daemon_threads = True
with open(PORT_FILE + ".tmp", "w") as f:
    f.write(str(server.server_address[1]))
os.replace(PORT_FILE + ".tmp", PORT_FILE)
server.serve_forever()
