"""Tiny server for the traffic fixture (tests/fixtures/sites/net.html).

    python tests/fixtures/sites/net_server.py [port]      (default 47822)

Serves the files of this folder, a small JSON API and an echo WebSocket, all on one origin:

    GET  /api/items?page=N   {"items":[{"id","name"}…],"page":N,"next":N+1|null}
    GET  /api/items/<id>     {"id","name","tags":[…]}
    POST /api/items          201 {"id":99, …posted JSON}
    GET  /api/price          {"price":10}
    GET  /ws                 WebSocket: echoes "echo: <text>" and sends "tick 1"

Standard library only.
"""

import base64
import hashlib
import json
import os
import re
import struct
import sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=HERE, **kwargs)

    def log_message(self, fmt, *args):
        sys.stderr.write("net_server: " + (fmt % args) + "\n")

    def send_json(self, status, data):
        body = json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/ws":
            return self.websocket()
        if url.path == "/api/items":
            page = int(parse_qs(url.query).get("page", ["1"])[0])
            items = [{"id": page * 10 + i, "name": f"Item {page * 10 + i}"} for i in range(3)]
            return self.send_json(200, {"items": items, "page": page, "next": page + 1 if page < 3 else None})
        m = re.fullmatch(r"/api/items/(\d+)", url.path)
        if m:
            item_id = int(m.group(1))
            return self.send_json(200, {"id": item_id, "name": f"Item {item_id}", "tags": ["new"]})
        if url.path == "/api/price":
            return self.send_json(200, {"price": 10})
        return super().do_GET()

    def do_POST(self):
        if urlparse(self.path).path == "/api/items":
            length = int(self.headers.get("Content-Length") or 0)
            data = json.loads(self.rfile.read(length) or b"{}")
            return self.send_json(201, {"id": 99, **data})
        self.send_json(404, {"error": "not found"})

    # -- WebSocket (RFC 6455, text frames only) ---------------------------------------

    def websocket(self):
        key = self.headers.get("Sec-WebSocket-Key", "")
        accept = base64.b64encode(hashlib.sha1((key + WS_GUID).encode()).digest()).decode()
        self.send_response(101, "Switching Protocols")
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.end_headers()
        self.wfile.flush()
        self.close_connection = True
        self.send_frame(1, b"tick 1")
        while True:
            frame = self.read_frame()
            if frame is None:
                return
            opcode, data = frame
            if opcode == 8:  # close
                self.send_frame(8, data[:2])
                return
            if opcode == 1:
                self.send_frame(1, b"echo: " + data)

    def read_frame(self):
        head = self.rfile.read(2)
        if len(head) < 2:
            return None
        opcode, size = head[0] & 0x0F, head[1] & 0x7F
        if size == 126:
            size = struct.unpack(">H", self.rfile.read(2))[0]
        elif size == 127:
            size = struct.unpack(">Q", self.rfile.read(8))[0]
        mask = self.rfile.read(4) if head[1] & 0x80 else b"\0\0\0\0"
        data = bytes(b ^ mask[i % 4] for i, b in enumerate(self.rfile.read(size)))
        return opcode, data

    def send_frame(self, opcode, data):
        size = len(data)
        if size < 126:
            head = bytes([0x80 | opcode, size])
        elif size < 65536:
            head = bytes([0x80 | opcode, 126]) + struct.pack(">H", size)
        else:
            head = bytes([0x80 | opcode, 127]) + struct.pack(">Q", size)
        self.wfile.write(head + data)
        self.wfile.flush()


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 47822
    print(f"net_server on http://127.0.0.1:{port}/net.html", flush=True)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
