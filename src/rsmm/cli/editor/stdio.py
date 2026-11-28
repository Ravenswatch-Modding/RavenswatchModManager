"""The editors over a pipe, for the native map editor (``apps/map-editor``).

``rsmm editor --stdio`` reads one request per line on stdin and answers each
with a frame on stdout. It hands every request to the same ``app.handle`` the
local server and the web editor use: same routes, same validation, same saves.
There is no socket, so there is no Host check, and the token is filled in here:
only the process that started this one holds the pipe.

Request (one JSON object per line)::

    {"id": 7, "method": "GET", "target": "/map/api/scene?chapter=DarkHills"}
    {"id": 8, "method": "POST", "target": "/map/api/save", "body": {...}}
    {"id": 9, "cancel": 7}

Response frame: one JSON header line, then exactly ``len`` raw body bytes::

    {"id": 7, "status": 200, "ctype": "application/json", "len": 1234, "headers": {}}\\n<body>

Frame ``id`` 0 is sent once at start: its body is ``{"game": ..., "mods_dir": ...}``.
Requests run on a small thread pool, so frames can arrive out of order; match
them by ``id``. EOF on stdin ends the process (the app went away).
"""

from __future__ import annotations

import json
import os
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import BinaryIO

from rsmm.cli.editor import app

#: Parallel requests. Mesh and texture conversion are the slow reads; a few at
#: once keeps the viewport filling without starving the save path.
WORKERS = 4
#: Longest request line accepted: room for the largest upload the backend
#: takes (base64 grows it by 4/3) plus its JSON wrapper. A save carries at most
#: MAX_STROKES brush dabs, far below this.
MAX_LINE = app.MAX_UPLOAD * 4 // 3 + (1 << 20)


class Pipe:
    def __init__(self, out: BinaryIO, *, mods: Path | None = None):
        # Off for the test suite (conftest), as for the HTTP server.
        self.ctx = app.Context(mods=mods, start="map",
                               disk_cache=os.environ.get("RSMM_EDITOR_DISK_CACHE", "1") != "0")
        self._out = out
        self._lock = threading.Lock()
        self._cancelled: set[int] = set()
        self._clock = threading.Lock()

    def send(self, rid: int, status: int, ctype: str, data: bytes,
             headers: dict[str, str] | None = None) -> None:
        head = json.dumps({"id": rid, "status": status, "ctype": ctype,
                           "len": len(data), "headers": headers or {}})
        with self._lock:
            self._out.write(head.encode("utf-8") + b"\n")
            self._out.write(data)
            self._out.flush()

    def hello(self) -> None:
        body = json.dumps({"game": self.ctx.game, "mods_dir": str(self.ctx.mods_dir)})
        self.send(0, 200, app.JSON, body.encode("utf-8"))

    def _alive(self, rid: int):
        def alive() -> bool:
            with self._clock:
                return rid not in self._cancelled
        return alive

    def handle_line(self, line: bytes) -> tuple[int, str, str, bytes] | None:
        """Parse one request line. Returns ``(id, method, target, body)``, or
        None for a cancel. A malformed line is answered here with id -1."""
        try:
            req = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            self.send(-1, 400, app.JSON, b'{"error": "request is not JSON"}')
            return None
        if not isinstance(req, dict):
            self.send(-1, 400, app.JSON, b'{"error": "request must be an object"}')
            return None
        if "cancel" in req:
            if isinstance(req["cancel"], int):
                with self._clock:
                    self._cancelled.add(req["cancel"])
            return None
        rid, method, target = req.get("id"), req.get("method"), req.get("target")
        if not isinstance(rid, int) or rid <= 0 or not isinstance(method, str) \
                or not isinstance(target, str) or not target.startswith("/"):
            self.send(rid if isinstance(rid, int) else -1, 400, app.JSON,
                      b'{"error": "need a positive id, a method and a /target"}')
            return None
        body = b""
        if "body" in req:
            body = json.dumps(req["body"]).encode("utf-8")
        return rid, method, target, body

    def run_one(self, rid: int, method: str, target: str, body: bytes) -> None:
        headers = {"X-RSMM-Token": self.ctx.token}
        if method.upper() == "POST":
            headers["Content-Type"] = app.JSON
        try:
            r = app.handle(self.ctx, method, target, headers, body, alive=self._alive(rid))
        except Exception as e:  # noqa: BLE001 -- answer the request; never kill the pipe
            self.send(rid, 500, app.JSON, json.dumps({"error": str(e)}).encode("utf-8"))
            return
        finally:
            with self._clock:
                self._cancelled.discard(rid)
        if r.status == 499:
            self.send(rid, 499, app.JSON, b"")      # cancelled: tell the app it's done
            return
        self.send(rid, r.status, r.ctype, r.data, dict(r.headers))


def run(inp: BinaryIO | None = None, out: BinaryIO | None = None,
        *, mods: Path | None = None) -> int:
    real_out = out or sys.stdout.buffer
    # Anything the engine prints must not land inside a frame.
    saved, sys.stdout = sys.stdout, sys.stderr
    try:
        return _serve(Pipe(real_out, mods=mods), inp or sys.stdin.buffer)
    finally:
        sys.stdout = saved


def _serve(pipe: Pipe, src: BinaryIO) -> int:
    pipe.hello()
    with ThreadPoolExecutor(max_workers=WORKERS) as pool:
        while True:
            line = src.readline(MAX_LINE + 1)
            if not line:
                break
            if len(line) > MAX_LINE:
                pipe.send(-1, 413, app.JSON, b'{"error": "request line too long"}')
                # Skip the rest of the oversized line.
                while line and not line.endswith(b"\n"):
                    line = src.readline(MAX_LINE)
                continue
            if not line.strip():
                continue
            parsed = pipe.handle_line(line)
            if parsed is not None:
                pool.submit(pipe.run_one, *parsed)
    return 0
