"""The status table in ``Common_Settings``: read it, append rows, write it back.

``Ot/Common_Settings.level.ot.GameStream.gen`` is a level stream (see
``level_placements`` for the wrapper and the graph around it). One of its
objects is the ``oCDtStatusSettingsGo`` named "Status Go", and it holds a
vector of ``oCDtStatus`` rows, one per status, by value (not in the object
table). Serializer ``FUN_14024f640`` and the vanilla bytes agree on a row::

    MARK_BEGIN
    u32  class index         position of "oCDtStatus" in the stream's class table
    u8   +0x18 look          1 buff, 0 debuff
    u8   +0x19 hidden        1 keeps it off the HUD; present only when the class
                             version in the table is non-zero (see below)
    u32  +0x1c computer kind how the HUD folds the status' modifiers (0..6)
    u32  step count          vector<oCDtStatusIconStep>
      per step:
      MARK_BEGIN
      u32  class index       position of "oCDtStatusIconStep"
      f32  threshold         the step shows once the value reaches it
      lstr root              TResourcePtr: "Ui", or "" for no texture
      lstr path              the texture, e.g. "HUD\\Buff_Strength_Icon.png"
      MARK_END
    u32  +0x20 id
    MARK_END

The ``u32 2`` that follows the threshold is the length prefix of the root
string "Ui", not a field of its own. A step with no texture ends in two empty
strings (Marked, Chill apply dot, Force combat state).

``+0x19`` sits behind ``FUN_1404fcd60(stream, tmp, 0x16eddfd0)``, which looks
up the version the stream recorded for class id ``0x16eddfd0`` (oCDtStatus) and
reads the byte only when its second word is non-zero (or when writing). The
class table row is ``(class id, 1, version, parent)``; oCDtStatus is
``(0x16eddfd0, 1, 1, ...)`` in the shipped file, so the byte is present. Both
words are 1 there, so the file cannot say which one the engine tests; this
codec tests the third column and writes no byte when it is 0.

Rows the codec did not build keep their original bytes, so the vanilla table
round-trips byte for byte. Class indices come from the file's own class table.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field

from . import cooked
from . import level_placements as LP

#: The decoded path of the stream, as `data/asset_map.json` spells it.
STREAM_DECODED = "Ot/Common_Settings.level.ot.GameStream.gen"

#: The 20 statuses the engine registers in code (README, Q2): a row with any of
#: these ids is the game's own, so a mod's status may not take one.
VANILLA_IDS: tuple[int, ...] = (
    0x16EDE056, 0x16EDE058, 0x1A0F9C2B, 0x1A5D3D69, 0x16EDE05A, 0x16EDE05C,
    0x16EDE05E, 0x173FCD75, 0x192E362C, 0x16EDE060, 0x16EDE062, 0xBA4BF279,
    0x16EDE064, 0x16EDE066, 0x1E9E687F, 0x16EDE068, 0x173FCDAA, 0x173FCDAC,
    0x1A40367D, 0x1DDB5600,
)

STATUS_CLASS = "oCDtStatus"
STEP_CLASS = "oCDtStatusIconStep"
SETTINGS_CLASS = "oCDtStatusSettingsGo"

#: Computer kinds, as the engine numbers them (def +0x74, row +0x1c).
COMBINE = {"add": 0, "sub": 1, "mult": 2, "max": 3, "min": 4, "reduce": 5, "oldest": 6}
#: Reapply policies (def +0x70).
REAPPLY = {"keep": 0, "extend": 1, "refresh": 2, "replace": 3, "replace_shortest": 4}
VALUE_TYPES = ("float", "int", "bool")
LOOKS = {"buff": 1, "debuff": 0}


class StatusTableError(ValueError):
    pass


@dataclass
class Step:
    threshold: float = 0.0
    root: bytes = b"Ui"
    path: bytes = b""


@dataclass
class Row:
    id: int
    look: int = 0
    hidden: int = 0
    kind: int = 0
    steps: list[Step] = field(default_factory=lambda: [Step()])
    #: The bytes this row was read from; None for a row we build.
    raw: bytes | None = None


def make_row(*, id: int, look: str, hidden: bool, combine: str, icon: str) -> Row:
    """The row a mod's ``status`` block asks for: one step at threshold 0.0."""
    path = icon.replace("/", "\\").encode("utf-8") if icon else b""
    return Row(id=id, look=LOOKS[look], hidden=int(hidden), kind=COMBINE[combine],
               steps=[Step(0.0, b"Ui" if path else b"", path)])


@dataclass
class Table:
    """Where the rows sit in the stream, and what was read."""

    rows: list[Row]
    has_hidden: bool
    status_cls: int
    step_cls: int
    obj: int                # index into the level graph's objects
    count_at: int           # offset of the row count inside the object payload
    end_at: int             # offset just past the last row inside the payload


def _classes(g: LP._Graph) -> list[tuple[str, tuple[int, int, int, int]]]:
    ct = g.blocks[g.ot - 1][2]
    (n,) = struct.unpack_from("<I", ct, 0)
    o, out = 4, []
    for _ in range(n):
        (ln,) = struct.unpack_from("<I", ct, o)
        name = ct[o + 4:o + 4 + ln].decode("utf-8", errors="replace")
        out.append((name, struct.unpack_from("<4I", ct, o + 4 + ln)))
        o += 4 + ln + 16
    return out


def _lstr(p: bytes, o: int) -> tuple[bytes, int]:
    (n,) = struct.unpack_from("<I", p, o)
    if o + 4 + n > len(p):
        raise StatusTableError("string runs past its block")
    return p[o + 4:o + 4 + n], o + 4 + n


def _read_rows(p: bytes, o: int, count: int, status_cls: int, step_cls: int,
               has_hidden: bool) -> tuple[list[Row], int]:
    rows: list[Row] = []
    for _ in range(count):
        start = o
        if p[o:o + 8] != cooked.MARK_BEGIN + struct.pack("<I", status_cls):
            raise StatusTableError(f"status row {len(rows)} does not begin as expected")
        o += 8
        look = p[o]
        o += 1
        hidden = 0
        if has_hidden:
            hidden = p[o]
            o += 1
        kind, nsteps = struct.unpack_from("<II", p, o)
        o += 8
        steps: list[Step] = []
        for _s in range(nsteps):
            if p[o:o + 8] != cooked.MARK_BEGIN + struct.pack("<I", step_cls):
                raise StatusTableError("icon step does not begin as expected")
            (thr,) = struct.unpack_from("<f", p, o + 8)
            root, o2 = _lstr(p, o + 12)
            path, o2 = _lstr(p, o2)
            if p[o2:o2 + 4] != cooked.MARK_END:
                raise StatusTableError("icon step has fields this codec does not know")
            steps.append(Step(thr, root, path))
            o = o2 + 4
        (sid,) = struct.unpack_from("<I", p, o)
        o += 4
        if p[o:o + 4] != cooked.MARK_END:
            raise StatusTableError("status row has fields this codec does not know")
        o += 4
        rows.append(Row(sid, look, hidden, kind, steps, raw=p[start:o]))
    return rows, o


def _encode_row(r: Row, status_cls: int, step_cls: int, has_hidden: bool) -> bytes:
    if r.raw is not None:
        return r.raw
    if r.hidden and not has_hidden:
        raise StatusTableError("this stream's oCDtStatus has no hidden flag")
    out = bytearray(cooked.MARK_BEGIN + struct.pack("<I", status_cls) + bytes([r.look]))
    if has_hidden:
        out.append(r.hidden)
    out += struct.pack("<II", r.kind, len(r.steps))
    for s in r.steps:
        out += cooked.MARK_BEGIN + struct.pack("<If", step_cls, s.threshold)
        out += struct.pack("<I", len(s.root)) + s.root
        out += struct.pack("<I", len(s.path)) + s.path + cooked.MARK_END
    out += struct.pack("<I", r.id) + cooked.MARK_END
    return bytes(out)


def _locate(data: bytes):
    try:
        cf, sec, inner = LP._inner(data)
        g = LP._graph(inner)
    except LP.LevelPlacementError as e:
        raise StatusTableError(str(e)) from None
    classes = _classes(g)
    names = [n for n, _r in classes]
    try:
        status_cls, step_cls = names.index(STATUS_CLASS), names.index(STEP_CLASS)
        settings_cls = names.index(SETTINGS_CLASS)
    except ValueError as e:
        raise StatusTableError(f"the stream has no class table entry for {e}") from None
    held = [i for i, c in enumerate(g.class_idx) if c == settings_cls]
    if len(held) != 1:
        raise StatusTableError(f"expected one {SETTINGS_CLASS}, found {len(held)}")
    has_hidden = classes[status_cls][1][2] != 0
    return cf, sec, inner, g, status_cls, step_cls, held[0], has_hidden


def decode(data: bytes) -> Table:
    """The status table of a ``Common_Settings`` stream."""
    try:
        _cf, _sec, _inner, g, scls, tcls, obj, has_hidden = _locate(data)
        p = g.objects[obj][2]
        first = p.find(cooked.MARK_BEGIN + struct.pack("<I", scls))
        if first < 4:
            raise StatusTableError("no status rows in the settings object")
        (count,) = struct.unpack_from("<I", p, first - 4)
        rows, end = _read_rows(p, first, count, scls, tcls, has_hidden)
    except (struct.error, IndexError) as e:
        raise StatusTableError(f"the status table does not read cleanly: {e}") from None
    return Table(rows, has_hidden, scls, tcls, obj, first - 4, end)


def encode(data: bytes, rows: list[Row]) -> bytes:
    """``data`` with its status table replaced by ``rows``.

    Everything outside the table is kept as it was; the stream's own length
    fields follow the new size.
    """
    cf, sec, inner, g, scls, tcls, obj, has_hidden = _locate(data)
    t = decode(data)
    begin, end, p = g.objects[obj]
    body = b"".join(_encode_row(r, scls, tcls, has_hidden) for r in rows)
    new_p = p[:t.count_at] + struct.pack("<I", len(rows)) + body + p[t.end_at:]
    new_inner = (inner[:begin] + cooked.MARK_BEGIN + new_p + cooked.MARK_END
                 + inner[end:])
    new_sec = bytearray(sec[:LP._INNER_OFF]) + new_inner
    struct.pack_into("<II", new_sec, LP._SELF_SIZE_OFF, len(new_inner), len(new_inner))
    cf.sections[-1] = cooked.Section(payload=bytes(new_sec))
    return cooked.emit(cf)


def append_rows(data: bytes, new: list[Row]) -> bytes:
    """``data`` with ``new`` rows added after the existing ones."""
    if not new:
        return data
    t = decode(data)
    have = {r.id for r in t.rows}
    for r in new:
        if r.id in have:
            raise StatusTableError(f"status id {r.id:#010x} is already in the table")
        have.add(r.id)
    return encode(data, [*t.rows, *new])


# --- the apply step ----------------------------------------------------------

#: Where the ``status`` kind stages each mod's rows, under its ``assets/``.
STAGING = "_pending_statuses"
ROWS_FILE = "rows.json"


def collect_rows(mods) -> list[tuple[str, Row]]:
    """``(mod id, row)`` for every enabled mod's staged status, in mod load
    order. Unreadable staging is skipped with a warning."""
    import json
    import sys

    out: list[tuple[str, Row]] = []
    for m in mods:
        path = m.assets_dir / STAGING / ROWS_FILE
        if not m.enabled or not path.is_file():
            continue
        try:
            for e in json.loads(path.read_text(encoding="utf-8")):
                out.append((m.id, make_row(
                    id=int(e["id"]), look=e["look"], hidden=bool(e["hidden"]),
                    combine=_COMBINE_NAME[int(e["combine"])], icon=str(e["icon"]))))
        except (OSError, ValueError, KeyError, TypeError) as ex:
            print(f"  [warn] {m.id}: unreadable status rows: {ex}", file=sys.stderr)
    return out


_COMBINE_NAME = {v: k for k, v in COMBINE.items()}


def sync_status_table(stream, dry_run: bool, mods, owner: str | None = None) -> int:
    """Put every enabled mod's status rows into ``stream``, the installed
    ``Common_Settings`` file; with none, put the vanilla file back. Rebuilt from
    the pristine ``.rsmm.bak`` each apply, like ``sync_versiondef``. Returns the
    number of files changed (0 or 1).

    ``owner`` names a mod that replaces the whole stream; its file is left
    alone and the rows are not added to it."""
    import shutil
    import sys

    from .paths import BACKUP_SUFFIX

    rows = collect_rows(mods)
    if stream is None or not stream.exists():
        return 0
    if owner:
        if rows:
            print(f"  [warn] {owner} replaces Common_Settings; the status rows of "
                  f"{len({m for m, _r in rows})} mod(s) are not added to it",
                  file=sys.stderr)
        return 0
    bak = stream.with_name(stream.name + BACKUP_SUFFIX)
    if not rows:
        if bak.exists():
            if not dry_run:
                shutil.copy2(bak, stream)
                bak.unlink()
            print("  [status] restored the vanilla status table")
            return 1
        return 0
    pristine = (bak if bak.exists() else stream).read_bytes()
    try:
        have = {r.id for r in decode(pristine).rows}
        add: list[Row] = []
        for mod_id, r in rows:
            if r.id in have:
                print(f"  [warn] {mod_id}: status id {r.id:#010x} is already taken; "
                      "its row is not added", file=sys.stderr)
                continue
            have.add(r.id)
            add.append(r)
        patched = append_rows(pristine, add)
    except StatusTableError as e:
        print(f"  [warn] status rows not added: {e}", file=sys.stderr)
        return 0
    if patched == stream.read_bytes():
        return 0
    print(f"  [status] {'would add' if dry_run else 'adding'} {len(add)} status row(s) "
          f"from {len({m for m, _r in rows})} mod(s) to Common_Settings")
    if not dry_run:
        if not bak.exists():
            shutil.copy2(stream, bak)
        stream.write_bytes(patched)
    return 1
