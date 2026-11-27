---
title: Map editor
description: The native map editor for building new chapters and placing your own models.
---

The map editor is a desktop app (`apps/map-editor`, Rust + wgpu) for making
**new chapters** and editing the map of an existing one. It reads the game
through the `rsmm` CLI (`rsmm editor --stdio`) and saves ordinary mods: a
`map` content block, plus `scenery` edits and your art in `art/`.

:::caution[Test builds]
The editor is new. Test builds come from the **Map editor** workflow on
GitHub Actions (artifact `rsmm-map-editor-Windows` or `-Linux`). There is no
release download yet.
:::

## Running it

- **Test build:** unzip and run `rsmm-map-editor`. The `rsmm` next to it is
  the CLI it reads the game through. Ravenswatch must be installed, or set
  `RSMM_GAME_DIR`.
- **From a clone:** `cd apps/map-editor && cargo run --release`. It finds the
  repository's `./rsmm` on its own. `--rsmm <path>` or `RSMM_BIN` picks
  another CLI.

## Making a chapter

1. **File › New chapter…** gives you flat ground with no scenery and one
   start and one boss spot, using the sky, light and enemies of the world
   you pick. **Keep the run's tiles** leaves the shipped chapter's camps,
   crystals, shrines and so on rolling on its spots instead. **Edge walls**
   (on by default) puts invisible walls round the ground, since the
   chapter's own walls are removed. **File › New map from a chapter…**
   (Ctrl+N) copies a shipped chapter instead.
2. Shape the ground with the **Terrain** tab's brushes. The Layer brush
   makes the 4 m terraces the shipped chapters use.
3. Drag props from **Project › Game** into the scene, or **Import…** your own
   `.glb` (and a `.png` with the same name for its texture). Imported models
   are solid by default. **Inspector › Collision** sets the box.
4. **Create › Enemy camp** places one of the chapter's camps (the
   `Wandering` ones roam on their own; the game has no route points for
   them). In the Inspector, **Own spawn settings** sets the camp's difficulty
   (Very easy … Very hard) and power (min / max: how big a group it rolls).
   Which monsters come is the map's tribe.
5. **Create › Invisible wall** and **Walkable floor** place boxes that block
   or carry heroes, with nothing drawn in game. A new chapter has no walls of
   its own, so wall off its edges. Tilt a floor for a ramp. These only work on
   a map of your own (New chapter / New map from chapter).
6. The **Terrain** tab's **Block** brush paints ground nobody can enter
   (Ctrl clears it). It saves as invisible walls. **View › Walkability** shows
   blocked ground in red and every wall and walkable box.
7. **Inspector › Collision** lists a model's or a collider's boxes: Wall or
   Ground, size, centre and rotation, in the object's own space. Walls draw
   orange, ground green.
8. Place spots in the **Spots** tab: what the run can put where.
9. **Check** builds the mod without saving, and **Save** (Ctrl+S) writes it
   to your mods folder. Then apply it in RSMM as usual.
10. **Play** (F5, also in File and the Save panel) saves, enables the mod
    and starts Ravenswatch through `rsmm run`. That applies every enabled
    mod, and puts the game files back when the game closes.

## Moving things

| Do | How |
|---|---|
| Look around / fly | Hold right mouse, move; add WASD, Q / E |
| Orbit, pan, zoom | Alt + left drag, middle drag, wheel |
| Move / rotate / scale tool | W / E / T, then drag an arrow, ring or handle |
| Move along a plane | Drag a coloured square between two arrows |
| Uniform scale | Drag the centre cube |
| Blender-style keys | G / R / S, then X / Y / Z to lock an axis (Shift = a plane), type a number, Enter |
| Snap | Ctrl while dragging (grid, 15°, 0.1×) |
| Frame selection / whole map | F / Home |
| Every command | F3 or Ctrl+P. F1 lists the shortcuts |

An arrow that points straight at you fades and can't be grabbed. Turn the
camera to use it. Objects you add rotate and scale on every axis. Objects in a
tile tab only turn about Y and scale evenly, because a tile's `places` store
only that.

## What gets saved

- Your objects go in the `map` kind's `objects`, each with `pos`, `yaw` or
  `quat`, and `scale` (one number or `[x, y, z]`). A collider is
  `collider = true` with its `collision` boxes.
- Moves and removals of the base chapter's own objects go to the clone's
  copy of its levels. The shipped chapter is never edited.
- Art you imported is copied into `mods/<id>/art/`.

The editor autosaves to `~/.config/rsmm-map-editor` and offers to recover
after a crash.
