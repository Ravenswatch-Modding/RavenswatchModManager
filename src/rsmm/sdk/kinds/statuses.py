"""``kind = "status"`` — a status of the mod's own: a HUD icon, a buff or debuff
look, and stacking rules.

::

    [[content]]
    kind = "status"
    name = "Frostbite"                 # unique within the mod
    look = "debuff"                    # "buff" | "debuff"
    hidden = false                     # true keeps it off the HUD
    icon = "HUD\\Debuff_Chilled.png"   # a texture the game ships
    value = "float"                    # "float" | "int" | "bool"
    combine = "max"                    # add sub mult max min reduce oldest
    hud_combine = "max"                # same names; the HUD's kind, default = combine
    reapply = "refresh"                # keep extend refresh replace replace_shortest
    max_stacks = 0                     # 0 = unlimited

A status needs two parts that agree on one id, the CRC of ``"<mod id>:<name>"``:
a table row in ``Common_Settings`` (icon, look, HUD flags; `rsmm apply` merges
every mod's rows into the one stream, see ``engine.status_table``) and a value
definition the loader registers at boot from ``statuses.json``.

The emit stages two files under ``assets/_pending_statuses/``, neither installed
as an asset: ``rows.json`` for the table merge and ``statuses.json``, which
apply copies to ``mods/<id>/statuses.json`` for the loader. The block's ``id``
is its ``name``.
"""

from __future__ import annotations

import json
from pathlib import Path

from ...engine import named_id
from ...engine import status_table as ST
from ..content import ContentDef, ContentError, written_this_run

STAGING = "_pending_statuses"
ROWS_FILE = "rows.json"
LOADER_FILE = "statuses.json"

_FIELDS = frozenset({"name", "look", "hidden", "icon", "value", "combine",
                     "hud_combine", "reapply", "max_stacks"})
_NAME_MAX = 64


def status_key(mod_id: str, name: str) -> str:
    """The string the id is the engine hash of."""
    return f"{mod_id}:{name}"


def status_id(mod_id: str, name: str) -> int:
    return named_id.of(status_key(mod_id, name))


def hud_kind(f: dict) -> str:
    """The computer kind of the table row (+0x1c): ``hud_combine``, else ``combine``."""
    return f.get("hud_combine", f.get("combine", "max"))


def validate(block: dict) -> list[str]:
    """What is wrong with one ``status`` block, one message each."""
    f = block
    bad: list[str] = []
    name = f.get("name")
    if not isinstance(name, str) or not name:
        bad.append("name must be a non-empty string")
    elif name != name.strip() or len(name) > _NAME_MAX or not name.isprintable():
        bad.append(f"name must be printable text of at most {_NAME_MAX} characters "
                   "with no space at either end")
    if f.get("look") not in ST.LOOKS:
        bad.append(f"look must be one of {', '.join(ST.LOOKS)}")
    if not isinstance(f.get("hidden", False), bool):
        bad.append("hidden must be true or false")
    icon = f.get("icon", "")
    if not isinstance(icon, str) or "\0" in icon:
        bad.append("icon must be a texture path string")
    for key, choices, default in (("value", ST.VALUE_TYPES, "float"),
                                  ("combine", tuple(ST.COMBINE), "max"),
                                  ("hud_combine", tuple(ST.COMBINE),
                                   f.get("combine", "max")),
                                  ("reapply", tuple(ST.REAPPLY), "refresh")):
        if f.get(key, default) not in choices:
            bad.append(f"{key} must be one of {', '.join(choices)}")
    stacks = f.get("max_stacks", 0)
    if isinstance(stacks, bool) or not isinstance(stacks, int) or stacks < 0:
        bad.append("max_stacks must be an integer of 0 or more")
    return bad


def collisions(mod_id: str, names: list[str]) -> list[str]:
    """Problems with the ids a mod's status names hash to: a name used twice,
    two names that hash alike, or an id the game already uses."""
    out: list[str] = []
    seen: dict[int, str] = {}
    for name in names:
        sid = status_id(mod_id, name)
        if sid in ST.VANILLA_IDS:
            out.append(f"{name!r} hashes to {sid:#010x}, the id of a status the game "
                       "ships")
        elif sid in seen:
            what = "is declared twice" if seen[sid] == name else \
                f"hashes to the same id as {seen[sid]!r}"
            out.append(f"{name!r} {what} ({sid:#010x})")
        seen.setdefault(sid, name)
    return out


def records(mod_id: str, f: dict) -> tuple[dict, dict]:
    """``(statuses.json entry, row entry)`` for a validated block."""
    name = f["name"]
    sid = status_id(mod_id, name)
    combine = ST.COMBINE[f.get("combine", "max")]
    hud = ST.COMBINE[hud_kind(f)]
    loader = {
        "key": status_key(mod_id, name), "name": name, "id": sid,
        "value": f.get("value", "float"), "combine": combine,
        "reapply": ST.REAPPLY[f.get("reapply", "refresh")],
        "max_stacks": f.get("max_stacks", 0),
    }
    row = {"id": sid, "name": name, "look": f["look"],
           "hidden": bool(f.get("hidden", False)), "combine": hud,
           "icon": f.get("icon", "")}
    return loader, row


def _load(path: Path) -> list:
    if not written_this_run(path):
        return []
    try:
        got = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    return got if isinstance(got, list) else []


def emit(mod_id: str, defn: ContentDef, out_dir: Path) -> list[Path]:
    f = defn.fields
    unknown = sorted(set(f) - _FIELDS)
    if unknown:
        raise ContentError(f"status {defn.id}: unsupported field(s) {unknown}; "
                           f"known: {sorted(_FIELDS)}")
    block = {**f, "name": f.get("name", defn.id)}
    bad = validate(block)
    if bad:
        raise ContentError(f"status {defn.id}: " + "; ".join(bad))
    loader, row = records(mod_id, block)
    stage = out_dir / STAGING
    stage.mkdir(parents=True, exist_ok=True)
    out = []
    for fname, entry in ((LOADER_FILE, loader), (ROWS_FILE, row)):
        path = stage / fname
        first = not written_this_run(path)
        have = _load(path)
        problems = collisions(mod_id, [e["name"] for e in have] + [entry["name"]])
        if problems:
            raise ContentError(f"status {defn.id}: " + "; ".join(problems))
        path.write_text(json.dumps([*have, entry], indent=2) + "\n", encoding="utf-8")
        if first:               # the same two files serve every block of the mod
            out.append(path)
    return out
