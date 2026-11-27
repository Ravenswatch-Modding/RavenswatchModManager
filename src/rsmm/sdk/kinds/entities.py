"""Edits to shipped entity files, in place — SDK entry point.

``[[content]] kind = "entity"`` applies the same graph steps a hero's abilities
use (``set``, ``link``, ``add_link``, ``remove_link``; see
``engine/ability_edit.py``) to any shipped ``.entity.ot`` -- a boss spawner, a
camp, a UI -- and writes the patched copy into the mod's ``assets/`` tree, so
the applier installs it as a plain asset override and ``rsmm restore`` puts the
original back. Part names are the ones ``rsmm entity-graph`` shows.

Fields:
    ``entity``  (str or list of str, required)  the entity's path under
                                 EntitySettings, e.g.
                                 ``Objects/Map_Boss_Spawner/Map_Boss_Spawner_Graph_Model``
                                 (``.entity.ot`` optional, ``\\`` or ``/``).
                                 A list applies the same steps to each file, and
                                 each starts from its own shipped copy, so ``scale``
                                 reads each file's own number. An empty list, a
                                 non-string item or a repeated path is refused.
    ``steps``   (list, required) ``[[content.steps]]`` tables, applied in order.

A ``set`` step may read its value from the mod's config instead of ``value``:
``config = "<field>"``, plus ``times = <n>`` to multiply a number field by
``n`` (the shipped value, so a 1.0 multiplier leaves the game as it was).
``scale = "<field>"`` replaces ``value`` and ``times``: it reads the shipped
number at the step's ``set`` path and writes that number times the config
field, so a game patch that rebalances the value carries into the mod. It
reads the shipped file, never an earlier block's output, so two blocks on one
file do not compound. The target must be a number; ``scale`` is refused beside
``value``, ``config`` or ``times``.
Any step may carry ``when = "<bool field>"``: it applies only while that
switch is on (e.g. a ``link`` that forces a branch).

``clone`` steps are refused: a copied part adds resources only a custom-hero
build preloads. Several blocks may edit one file; each starts from what an
earlier block of this mod already wrote.
"""

from __future__ import annotations

import logging
from dataclasses import replace
from pathlib import Path

from ...engine import ability_edit as AE
from ...engine import corpus
from ...engine.entity_graph_edit import EntityEditError, EntityFile
from ..content import ContentDef, ContentError, written_this_run
from . import _common as C

_log = logging.getLogger(__name__)

_GEN_SUFFIX = ".entity.ot.EntitySettingsResource.gen"
_ROOT = "EntitySettings"


def _rel(entity: str) -> str:
    """``Objects\\X\\Y.entity.ot`` -> ``EntitySettings/Objects/X/Y`` + gen suffix."""
    p = entity.replace("\\", "/").strip("/")
    for cut in (_GEN_SUFFIX, ".entity.ot", ".entity"):
        p = p.removesuffix(cut)
    if p.startswith(_ROOT + "/"):
        p = p[len(_ROOT) + 1:]
    return f"{_ROOT}/{p}{_GEN_SUFFIX}"


def _shipped_number(defn: ContentDef, step: dict, ef: EntityFile) -> int | float:
    """The shipped number at ``step["set"]`` (``Part.field``)."""
    spec = str(step["set"])
    if ".cinebloc[" in spec:
        part, _, fld = spec.partition(".cinebloc[")
        fld = "cinebloc[" + fld
    else:
        part, _, fld = spec.rpartition(".")
    if not part:
        raise ContentError(f"entity {defn.id}: set = {spec!r}: write it as 'Part.field'")
    try:
        shipped = ef.get_value(part, fld)
    except EntityEditError as e:
        raise ContentError(f"entity {defn.id}: scale on {spec!r}: {e}") from e
    if isinstance(shipped, bool) or not isinstance(shipped, int | float):
        raise ContentError(f"entity {defn.id}: scale on {spec!r} needs a number field, "
                           f"the shipped value is {shipped!r}")
    # A float read from f32 bytes carries decode noise (2.2 reads 2.2000000477);
    # seven significant digits is what an f32 holds.
    return float(f"{shipped:.7g}") if isinstance(shipped, float) else shipped


def _resolve_config(defn: ContentDef, steps: list[dict], mod_root: Path,
                    shipped: bytes | None = None) -> list[dict]:
    """A ``set`` step may take its value from the mod's config instead of a
    literal: ``config = "<field>"`` reads the player's value (or the schema
    default), and ``times = <n>`` multiplies a number by it — so one
    "health multiplier" field can scale every shipped value it names.
    ``scale = "<field>"`` multiplies the SHIPPED number at the ``set`` path by
    the player's value instead, so the mod holds no copy of the shipped value;
    ``shipped`` is the pristine file it is read from."""
    if not any("config" in s or "when" in s or "scale" in s for s in steps):
        return steps
    from rsmm.sdk.config import ConfigError, ConfigStore, config_value
    try:
        store = ConfigStore(mod_root)
    except (OSError, ConfigError, ValueError) as e:
        raise ContentError(f"entity {defn.id}: config unreadable: {e}") from e
    graph: list[EntityFile] = []
    out = []
    for s in steps:
        if "when" in s:
            try:
                on = config_value(store, s["when"])
            except ConfigError as e:
                raise ContentError(f"entity {defn.id}: when: {e}") from e
            if not isinstance(on, bool):
                raise ContentError(f"entity {defn.id}: when = {s['when']!r} must name an "
                                   f"on/off (bool) field")
            if not on:
                continue
            s = {k: v for k, v in s.items() if k != "when"}
        if "scale" in s:
            if "set" not in s or {"value", "config", "times"} & s.keys():
                raise ContentError(f"entity {defn.id}: 'scale' goes on a set step, "
                                   f"in place of 'value', 'config' and 'times'")
            try:
                mult = config_value(store, s["scale"])
            except ConfigError as e:
                raise ContentError(f"entity {defn.id}: {e}") from e
            if isinstance(mult, bool) or not isinstance(mult, int | float):
                raise ContentError(f"entity {defn.id}: scale = {s['scale']!r} needs a number "
                                   f"field, it is {mult!r}")
            if shipped is None:
                raise ContentError(f"entity {defn.id}: scale needs the shipped file")
            if not graph:
                try:
                    graph.append(EntityFile(shipped))
                except EntityEditError as e:
                    raise ContentError(f"entity {defn.id}: {e}") from e
            base = _shipped_number(defn, s, graph[0])
            value = round(base * mult, 6)
            if isinstance(base, int):
                value = int(round(value))
            out.append({k: v for k, v in s.items() if k != "scale"} | {"value": value})
            continue
        if "config" not in s:
            out.append(s)
            continue
        if "set" not in s or "value" in s:
            raise ContentError(f"entity {defn.id}: 'config' goes on a set step, "
                               f"in place of 'value'")
        try:
            value = config_value(store, s["config"], s.get("times"))
        except ConfigError as e:
            raise ContentError(f"entity {defn.id}: {e}") from e
        out.append({k: v for k, v in s.items() if k not in ("config", "times")}
                   | {"value": value})
    return out


def emit(mod_id: str, defn: ContentDef, out_dir: Path) -> list[Path]:
    """Write the edited entity files into the mod's ``assets/`` tree."""
    C.validate_id("entity", defn.id)
    entity = defn.fields.get("entity")
    many = isinstance(entity, list)
    names = entity if many else [entity]
    if not entity or not all(isinstance(n, str) and n for n in names):
        raise ContentError(f"entity {defn.id}: needs 'entity', the file's path under "
                           f"EntitySettings (e.g. Objects/Map_Boss_Spawner/Map_Boss_Spawner_Model)"
                           f", or a list of such paths")
    steps = defn.fields.get("steps")
    if not isinstance(steps, list) or not steps or not all(isinstance(s, dict) for s in steps):
        raise ContentError(f"entity {defn.id}: needs 'steps', a list of [[content.steps]] tables")
    if any("clone" in s and s.get("from") for s in steps):
        raise ContentError(f"entity {defn.id}: cross-file clone steps are not supported here; "
                           f"use same-file clone / set / link / add_link / remove_link / percent")
    rels = [_rel(n) for n in names]
    for n, rel in zip(names, rels, strict=True):
        if rels.count(rel) > 1:
            raise ContentError(f"entity {defn.id}: {n!r} is listed twice ({rel})")
    written: list[Path] = []
    for n, rel in zip(names, rels, strict=True):
        # A list block's errors name the entity too, so the bad one is findable.
        shown = replace(defn, id=f"{defn.id} ({n})") if many else defn
        written += _emit_file(mod_id, defn, shown, n, rel, steps, out_dir)
    return written


def _emit_file(mod_id: str, defn: ContentDef, shown: ContentDef, entity: str, rel: str,
               steps: list[dict], out_dir: Path) -> list[Path]:
    """Apply ``steps`` to one shipped entity file. ``shown`` is ``defn`` as
    errors should name it; the seed and log keep the block's own id."""
    shipped = corpus.read(rel)
    if shipped is None:
        raise ContentError(f"entity {shown.id}: no shipped entity {entity!r} ({rel}) — "
                           f"is the game install readable?")
    steps = _resolve_config(shown, steps, out_dir.parent, shipped)
    stem = Path(rel).name.removesuffix(_GEN_SUFFIX)
    dest = out_dir / Path(*rel.split("/"))
    before = dest.read_bytes() if written_this_run(dest) else shipped
    try:
        res = AE.apply({stem: before}, steps, main=stem, seed=f"{mod_id}:{defn.id}")
    except AE.AbilityEditError as e:
        raise ContentError(f"entity {shown.id}: {e}") from e
    for w in res.warnings:
        _log.warning("entity %s: %s", shown.id, w)
    blob = res.files.get(stem, before)
    if blob == before:
        _log.info("entity %s/%s: no change to %s", mod_id, shown.id, stem)
        return []
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(blob)
    _log.info("entity %s/%s: patched %s", mod_id, shown.id, rel)
    return [dest]
