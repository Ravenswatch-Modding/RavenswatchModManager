"""`rsmm editor --stdio`: the native map editor's transport.

The pipe must hand every request to the same ``app.handle`` as the HTTP server,
fill in the write token itself (only the parent holds the pipe), keep stray
prints out of the frame stream, and answer malformed lines instead of dying.
"""

from __future__ import annotations

import io
import json

from rsmm.cli.editor import stdio


def _frames(raw: bytes) -> list[tuple[dict, bytes]]:
    out, i = [], 0
    while i < len(raw):
        j = raw.index(b"\n", i)
        head = json.loads(raw[i:j])
        body = raw[j + 1:j + 1 + head["len"]]
        assert len(body) == head["len"], "frame body shorter than its len"
        out.append((head, body))
        i = j + 1 + head["len"]
    return out


def _run(tmp_path, *lines: object) -> dict[int, tuple[dict, bytes]]:
    inp = io.BytesIO(b"".join(
        (ln if isinstance(ln, bytes) else json.dumps(ln).encode()) + b"\n" for ln in lines))
    out = io.BytesIO()
    assert stdio.run(inp, out, mods=tmp_path) == 0
    frames = _frames(out.getvalue())
    return {h["id"]: (h, b) for h, b in frames}


def test_hello_comes_first(tmp_path):
    inp, out = io.BytesIO(b""), io.BytesIO()
    stdio.run(inp, out, mods=tmp_path)
    (head, body), = _frames(out.getvalue())
    assert head["id"] == 0 and head["status"] == 200
    assert json.loads(body)["mods_dir"] == str(tmp_path)


def test_routes_answer_like_http(tmp_path):
    got = _run(tmp_path,
               {"id": 1, "method": "GET", "target": "/map/api/mod?id=NOT%20VALID"},
               {"id": 2, "method": "GET", "target": "/map/api/nope"},
               {"id": 3, "method": "GET", "target": "/nowhere/"})
    assert got[1][0]["status"] == 400 and "mod id" in json.loads(got[1][1])["error"]
    assert got[2][0]["status"] == 404
    assert got[3][0]["status"] == 404


def test_post_gets_the_token_from_the_pipe(tmp_path):
    # A POST over HTTP without X-RSMM-Token is a 403. Over the pipe it reaches
    # the route, which then refuses the body on its own terms.
    got = _run(tmp_path, {"id": 5, "method": "POST", "target": "/map/api/save",
                          "body": {"chapter": "Nowhere", "mod_id": "x"}})
    head, body = got[5]
    assert head["status"] != 403, json.loads(body)


def test_bad_lines_are_answered_not_fatal(tmp_path):
    got = _run(tmp_path, b"{not json", {"id": "x", "method": "GET", "target": "/"},
               {"cancel": 99},
               {"id": 4, "method": "GET", "target": "/map/api/nope"})
    assert got[-1][0]["status"] == 400
    assert got[4][0]["status"] == 404, "a later good request still runs"


def test_prints_do_not_corrupt_frames(tmp_path, monkeypatch, capsys):
    from rsmm.cli.editor import app

    real = app.handle

    def noisy(*a, **kw):
        print("engine chatter that must not reach stdout")
        return real(*a, **kw)

    monkeypatch.setattr(app, "handle", noisy)
    got = _run(tmp_path, {"id": 1, "method": "GET", "target": "/map/api/nope"})
    assert got[1][0]["status"] == 404
