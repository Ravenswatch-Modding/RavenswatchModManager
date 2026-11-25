---
title: Instructions for AI agents building mods
description: A file to give an AI agent building a Ravenswatch mod. What to read, how producers, consumers and the R.* submodules fit together, and why RSMM changes go through an issue.
---

For AI agents helping someone build a mod with Ravenswatch Mod Manager (RSMM). Your job is the mod: a folder of data and, where needed, a Lua script that uses RSMM as it stands.

Read [Agent quick start](https://docs.rsmm.me/guides/agent-quick-start/) first, unless it sent you here.

## The one rule: RSMM changes go through an issue

**Never push to RSMM and never open a pull request against it.** RSMM is `Ravenswatch-Modding/RavenswatchModManager`. The rule covers direct pushes, branches, pull requests from a fork, and draft pull requests, however small the fix and even if the account you are using has write access.

If the mod needs something RSMM doesn't have, or RSMM has a bug, the way forward is an issue: see [When RSMM is missing something](#when-rsmm-is-missing-something).

Why: the SDK is shared by every mod. A change made for one mod lands in everyone's game, so the maintainers decide where each capability belongs and build it themselves. A clear issue gets you there faster than a patch they have to take apart.

### What this means on disk

The modding tools are a clone of the RSMM repository, and mods are usually written in its `mods/` folder, so you may be working inside the RSMM checkout. Git ignores `mods/`.

| Path | You may |
|---|---|
| `mods/<your mod id>/` | Create and edit freely. It can be its own git repository. |
| Rest of the RSMM clone (`src/`, `data/`, `apps/`, `tests/`, `docs/`, ...) | Read. Update with `git pull --ff-only`. Make no edits, commits or branches. |
| `<game>/rsmm/lib/` (the installed Lua SDK) | Read. A local patch here works only on this machine and is overwritten by the next loader update. |
| Everything else in `<game>/` | Change only through `rsmm` commands. Leave `_Save/` and `DarkTalesResources/_Cooking/` alone. |
| Other people's mods | Read. Copy only what their licence allows. |

Before any git command that writes, run `git rev-parse --show-toplevel` and `git remote -v`. The top level must be the mod's own folder (or the author's own mods repository), and no remote may be RSMM or a fork of it. Otherwise stop. The one exception is `git pull --ff-only` to update the clone.

### Which instructions win

The clone has its own `CLAUDE.md`, `AGENTS.md`, `CONTRIBUTING.md` and `.claude/skills/`. Your tool may load them automatically. Apart from the pointer to the quick start, they are for people developing RSMM itself. Their facts about the engine and the linter are useful to you; their workflow (editing `src/loader/`, adding symbols, regenerating files, opening pull requests, publishing releases) is not yours. Where they disagree with this file, follow this file.

The same goes for wording like "move its logic into `rsmm.sdk`" or "add a symbol to `data/symbols.json`" in the docs and in `rsmm lint` output. For a mod author that means: open an issue.

## Read the docs first

Ravenswatch runs on its own C++ engine. It is not Unity: there is no `Ravenswatch_Data/`, `Assembly-CSharp.dll` or mono runtime, and BepInEx, Harmony, MelonLoader and DLL plugins do not load. RSMM's loader is a `winhttp.dll` proxy with an embedded Lua 5.4. Knowledge from modding other games will mislead you, so read before you write. Don't invent a manifest key, content kind, event or `R.*` call; look it up.

All pages are on `https://docs.rsmm.me`. In a clone, the same pages are Markdown files under `apps/docs/src/content/docs/` (pull first, so they're current). Two ways to get them as text:

- [`/llms.txt`](https://docs.rsmm.me/llms.txt): an index of every page, small. Open the pages you need from it.
- [`/llms-mods.txt`](https://docs.rsmm.me/llms-mods.txt): every authoring page in one file. It is over 500 KB, so download it and search it unless your context can hold it all.

`/llms-full.txt` adds the reverse-engineering notes. You don't need it to build a mod.

### Reading order

Always, before the first line of a mod:

1. [Mods ship data, not code](https://docs.rsmm.me/concepts/data-not-code/): the rule everything else follows from.
2. [The mod lifecycle](https://docs.rsmm.me/concepts/mod-lifecycle/): how a mod gets into the game and back out.
3. [Authoring mods](https://docs.rsmm.me/guides/modding/): the main reference. Read up to "Recipes", then the sections your mod touches.
4. [Content kinds & confidence](https://docs.rsmm.me/reference/sdk-api/kinds/): what can be added or changed, and how proven each kind is.

Then by what the mod does:

| The mod | Read |
|---|---|
| Is the author's first | [Make your first mod](https://docs.rsmm.me/getting-started/first-mod/), [First mod, by kind](https://docs.rsmm.me/guides/first-mod-by-kind/), [Example mods](https://docs.rsmm.me/guides/examples/) |
| Adds or edits items | [Custom items](https://docs.rsmm.me/guides/custom-items/) |
| Adds or edits enemies, bosses | [Custom enemies](https://docs.rsmm.me/guides/custom-enemies/) |
| Changes talents | [Talent modding, step by step](https://docs.rsmm.me/guides/talent-tutorial/), [Advanced talent modding](https://docs.rsmm.me/guides/talent-tutorial-advanced/), [Talent name lookup](https://docs.rsmm.me/reference/talent-names/) |
| Changes numbers | "Numeric balance" in [Authoring mods](https://docs.rsmm.me/guides/modding/) |
| Runs Lua in game | "Lua-scripted mod" and every `R.*` section in [Authoring mods](https://docs.rsmm.me/guides/modding/), then the [Lua gameplay API](https://docs.rsmm.me/reference/lua-api/) |
| Reacts to events | "Events" and "Event payloads" in [Authoring mods](https://docs.rsmm.me/guides/modding/), [Event systems](https://docs.rsmm.me/reverse-engineering/event-systems/) |
| Will be published | "Don't ship vanilla bytes" and "Online play" in [Authoring mods](https://docs.rsmm.me/guides/modding/), `rsmm pack` and `rsmm publish` in the [CLI reference](https://docs.rsmm.me/reference/cli/) |

Not set up yet: [Set up the modding tools](https://docs.rsmm.me/getting-started/modding-tools/). Something broken: [Troubleshooting](https://docs.rsmm.me/getting-started/troubleshooting/), then `rsmm doctor`.

### Ask the tools

| Question | Command |
|---|---|
| Which vanilla ids can I clone? | `rsmm schema <kind> --grep <text>` |
| Which items, enemies, talents exist? | `rsmm items`, `rsmm enemies`, `rsmm talents` |
| Where is this asset? | `rsmm assets search <text>` |
| Which events can I listen to? | `rsmm symbols events [filter]` |
| Is my mod valid? | `rsmm lint <id>` (the id, not a path) |
| What will it change, and what does it overlap? | `rsmm diff <id>` |
| What happened in game? | `rsmm log`, `rsmm log --grep <text>` |
| Is the install healthy? | `rsmm doctor` |

### When sources disagree

Trust `rsmm lint`, then the Lua SDK source (the module's header comment), then Authoring mods and the Lua API page, then everything else. Known out-of-date spot:

- The reverse-engineering pages show raw engine calls, hooks and memory access (`rsmm.call`, `rsmm.hook`, `rsmm.write_*`) from a mod's `init.lua`. Follow [Off limits](#off-limits) instead.

## How a mod is built

A mod is data: a `manifest.toml` and the assets it uses, plus Lua where needed. A Python script is never the deliverable. `rsmm lint` fails any `*.py` in a mod except the `on_disable.py` lifecycle hook, and an SDK generator stays outside the mod folder.

Use the first of these that can do the job:

1. **A `[[patch]]`** in `manifest.toml` for a number, a texture swap or a plaintext field. Patches from different mods merge.
2. **A content definition**, in its own folder (`items/ember_charm/item.toml`) or as `[[content]]`, for something new or changed. Each content kind is rated `confirmed`, `experimental` or `guess`, and one rated below `confirmed` needs `experimental = true` under `[mod]` (`sdk.Mod(..., experimental=True)` in an SDK generator).
3. **An asset override** under `assets/`. One mod owns the whole file, so two mods overriding the same file conflict.
4. **`init.lua`** at the mod root (not `main.lua` or `scripts/main.lua`), for behaviour that depends on what happens during a run.
5. **An issue**, when none of the above can do it.

Aim for mods that feel built into the game: its own menus, key bindings and events before anything drawn on top or forced every frame. For settings, a `config_schema.toml` gives the mod rows in Settings > Mods. For keys, `R.input.keybinds` adds rows to the game's Controls menu. `R.overlay` is for data the game has nowhere to show.

Ship only files the author made or changed. `rsmm pack` refuses unmodified game files, and mods that unlock paid content (DLC heroes, outfits) are not accepted.

## Producers and consumers

This is the idea behind RSMM's design. Use it to decide where each part of a feature goes.

- A **producer** makes something available: it emits an event, exposes state, offers one action, or adds native data. It doesn't know or care who uses it.
- A **consumer** decides what to do with it. It uses only the producer's public surface and never reaches behind it.

The same split runs at every level:

| Producer | Consumer | Example |
|---|---|---|
| The SDK (`R.*`, content kinds) | Your mod | The SDK offers `R.camera.zoom`. The mod decides when to zoom and how far. |
| One `R.*` submodule | Another submodule or a mod | `R.input` reports a wheel notch and knows nothing about cameras. |
| A content kind (build time) | Lua (run time) | The kind adds a marker to a nameplate. Lua decides whose marker shows. |
| One mod | Another mod | A loot mod exposes `roll()` through `R.api`. Another mod calls it. |

**Your mod is always a consumer of the SDK.** It holds the rules, the numbers, the conditions and the taste. The SDK holds the general ability and knows no mod by name.

### Splitting a feature

Say what the feature does in one sentence, then cut it where the concern changes. "Scroll the mouse wheel to zoom the camera, but not in menus":

| Part | Concern | Who owns it |
|---|---|---|
| The player turned the wheel | Input | `R.input` (producer) |
| A menu is open | What the UI is showing | `R.ui` (producer) |
| The view gets closer | Camera | `R.camera` (producer) |
| One notch is 10%, skip it in menus | This mod's rules | Your `init.lua` (consumer) |

Three general producers and one small consumer:

```lua
local R = require "rsmm"

if not (R.input and R.input.on_wheel and R.ui and R.ui.is_open and R.camera and R.camera.zoom) then
    R.log("[wheel-zoom] needs a newer loader (no R.input.on_wheel, R.ui or R.camera.zoom)")
    return
end

R.input.on_wheel(function(steps)                -- R.input: what the player did
    -- R.ui: what the UI shows. true, false, or nil for "can't tell"; zoom only on a sure false.
    if R.ui.is_open("menu") ~= false then return end
    R.camera.zoom(0.9 ^ steps)                  -- R.camera: what the view does
end)
```

with, in its `manifest.toml`:

```toml
[mod]
loader_flags = ["RSMM_ENABLE_HERO_CAPTURE"]   # R.camera has to find the hero
```

If a part has no owner yet, that part is the issue. Build the rest against what exists.

### Is the missing part a producer or a consumer?

- Would another, unrelated mod want it, in the same form? Producer. It belongs in RSMM, so open an issue.
- Does it contain this mod's numbers, text, hero, art or rule? Consumer. It stays in the mod.
- Is it both? Cut it again. "Zoom out when a boss wakes" is a boss event (producer), a zoom (producer) and one line joining them (consumer).

## How the `R.*` submodules work

`local R = require "rsmm"` is the only entry point (there is no `R.call()` or `rsmm.call()`), and every Lua file in the mod that uses `R` needs it in scope. `R` is the Lua SDK. Each `R.<name>` namespace owns one concern. Many live in their own file and the rest in the core file; the SDK attaches them all to `R`, so use `R.camera`, not `require "rsmm.camera"`.

A namespace can be missing. If a submodule fails to load, or the player's loader is older than it, that namespace is nil while the rest of `R` keeps working. Check for what you use and log a plain reason when it isn't there. Many calls return `nil, reason` or `false`; log the reason when there is one, and look in `rsmm log` when there isn't.

### Which namespace owns what

| Concern | Namespaces |
|---|---|
| Events between the game, the SDK and mods | `R.on`, `R.once`, `R.on_match`, `R.off`, `R.emit`, `R.events` |
| Timing and threads | `R.schedule` |
| Logging, saved values, settings, translations | `R.log`, `R.kv`, `R.config`, `R.i18n` |
| Calls between mods, and the installed list | `R.api`, `R.mods` |
| What the player pressed | `R.input`, `R.input.keybinds` |
| What the view does | `R.camera` |
| What the UI is showing, and menu pages | `R.ui`, `R.ui.menus` |
| Data shown in the desktop overlay | `R.overlay` |
| Sound | `R.audio` |
| The run: state, chapters, day and night, map | `R.game`, `R.run`, `R.daynight`, `R.map` |
| The hero: identity, health, stats, level, status effects | `R.hero`, `R.entity`, `R.hp`, `R.stat`, `R.xp`, `R.status` |
| Abilities, talents, melodies, hero input lock | `R.ability`, `R.talent`, `R.melody`, `R.control` |
| Items and currencies | `R.give`, `R.item`, `R.shards`, `R.ingredient`, `R.reroll`, `R.revive` |
| Game modifiers and game options | `R.modifier`, `R.options` |
| Damage totals and ranking | `R.damage` |
| Spawning, interactions, projectiles | `R.spawn`, `R.interact`, `R.projectile` |
| Players and network state | `R.player`, `R.lobby`, `R.net` |
| Recording what a playtest showed | `R.exp` |

A name in this table is not proof that it works. Some calls are experimental, some are kept only for older mods, and a few are placeholders that log "NOT IMPLEMENTED". Before using one, read its section in [Authoring mods](https://docs.rsmm.me/guides/modding/) or the [Lua gameplay API](https://docs.rsmm.me/reference/lua-api/), then the source comment for anything the docs leave open.

In a clone, submodules are `src/loader/lua/rsmm/<name>.lua` (the file name can differ: `R.game` is `gamevalues.lua`) and the core is `src/loader/lib/rsmm.lua`. The comment above a namespace says what it owns, what was seen working in game, and its limits. `data/contracts/r_surface.txt` lists the names that exist.

### Off limits

Lint rejects these:

- `require "rsmm.<anything>"`, and writing into the SDK (`R.x = ...`, `function R.x()`). Keep your own table.
- `R.hook`, `R.unhook`, `R.ptr`, `_internal`, `R.engine.call_raw`, `R.engine.resolve`, `peek`, `poke`, `read_*`, `write_*`, and literal game addresses.

Lint doesn't catch all of these, but leave them alone too:

- Any name starting with an underscore, such as `R.stat._set`.
- `R.debug` and `R.watch`, unless the mod is a debugging tool. `R.debug` needs the mod tagged `debug` or `test`.
- `R.engine.call` and `R.engine.fn`. They reach a few hundred named functions. They pass lint, but needing one usually means a producer is missing, so write the issue. If the author decides to use one anyway, never pass it a pointer: that has been behind the loader's in-game crashes.

### Runtime rules

- **Change the game only from the main thread.** Wrap such calls in `R.schedule.next_main(function() ... end)`, or use `after_main` / `every_main`, including inside event handlers. Plain `after`, `every`, `next_frame`, the `tick` event and `R.config.on_change` watchers run on a background thread, where changing the game can crash it.
- **Declare hero capture.** `R.entity`, `R.stat`, `R.xp`, `R.combat`, `R.camera` and some `R.talent` calls need `loader_flags = ["RSMM_ENABLE_HERO_CAPTURE"]` under `[mod]`. Without it they never find the hero, and the mod does nothing. Lint warns about this.
- **Wait for the hero.** Calls that act on the hero refuse until the hero's first attack or ability. `R.give.ready()` says when.
- **Each mod has its own Lua state.** Globals aren't shared. Signal with `R.emit` and `R.on`, and call with `R.api.expose` and `R.api.require`. Only data crosses: no functions as arguments or results.
- **There is no `io` and no `debug`.** Save with `R.kv`; read settings with `R.config`.
- **Keep handlers short.** Many run inside the game's own code.
- **Undo what you hold.** Pair `R.control.lock()` with `R.control.release()`. List pinned game options under `restore_options` in the manifest.
- **Online, mods are sorted by what they do.** `rsmm modpack` says whether a mod counts as client-only or gameplay, and why. Set `multiplayer_scope` to match.

## Check the work

```sh
rsmm lint <id>       # fix every error; explain every warning you leave
rsmm diff <id>       # what it changes, and what it overlaps
rsmm apply           # install every enabled mod
rsmm install-loader  # Lua mods only; needed again after a restore
rsmm run             # start the game
rsmm log -n 80       # look for "init failed" and "[lua] <id>"
rsmm restore --all   # undo everything
```

- `enabled = true` must be in the manifest, or `rsmm apply` skips the mod. Examples ship with it off.
- No line about the mod in the log usually means the loader isn't installed or the mod is off. `rsmm doctor` checks the loader.
- Lint passing means the mod is well formed. It doesn't mean it works. Only the game shows that, and the author is usually the one playing, so tell them exactly what to do in game and what they should see.
- Report honestly: say what was linted, what was seen in game and by whom, and what is untested.
- Before explaining a bug, settle what the author actually saw. Ask one question if the report could mean two things.
- `rsmm watch` re-applies on save while the game runs, which suits Lua changes.
- Publishing (`rsmm publish <id>`) is public. Do it only when the author asks for that release. The author enters their own API token.

### Before packaging for release

Ask the author this question unless they have already answered it:

> When the mod is enabled, when should its effect start, stop, and apply again? What should happen when the mod is disabled?

For a simple on/off mod, propose the usual behavior: the effect works whenever
it applies while enabled, including after returning to the menu and starting
another run, and is absent while disabled. An effect that only works once per
game launch needs to be an explicit design choice.

Review the mod against that answer before handing over a release package. Check
the transitions that matter for the feature, such as using it again, changing
heroes, starting a new run, or disabling and re-enabling it. If nobody has
observed the expected behavior in game yet, package a playtest build and say
which checks are still needed before calling it ready for release.

The loader normally runs `init.lua` once per game launch. Lua locals can
therefore outlive a run. If a flag or cached value belongs to one run, clear it
at the run boundary using `R.on("run:end", ...)`. For example, if a movement
bonus should apply once per run, clear the flag that records whether it was
applied and discard any cached starting speed when the run ends.

## When RSMM is missing something

Stop at the gap. Don't patch RSMM to get past it, and don't work around lint. Then:

1. **Make sure it's missing.** Search `data/contracts/r_surface.txt`, the docs and `rsmm symbols events`. Update first (`git pull --ff-only`, `rsmm update-loader`), since it may already have landed.
2. **Look for an existing issue:**

   ```sh
   gh issue list -R Ravenswatch-Modding/RavenswatchModManager --state all --search "<words>"
   ```

   If there is one, show it to the author. A short comment with their use case beats a duplicate.
3. **Split it.** One issue per producer. A feature that needs an input piece and a camera piece is two issues that link to each other, with the one the other depends on first.
4. **Write the issue and show it to the author.** Open it, or comment on an existing one, only once they say yes, because it is public and under their name:

   ```sh
   gh issue create -R Ravenswatch-Modding/RavenswatchModManager --title "<title>" --body-file <file>
   ```

   Leave labels to the maintainers. Without `gh`, give the author the text and <https://github.com/Ravenswatch-Modding/RavenswatchModManager/issues/new/choose>.
5. **Carry on with the rest of the mod.** Leave the missing part out, or fall back behind a check like the one in the example above. When the feature ships, set the oldest version that has it (these numbers are only examples):

   ```toml
   [requires]
   loader = 66   # Lua SDK and loader features
   cli    = 31   # manifest and content features
   ```

### Request for something new

```markdown
Title: <R.namespace or content kind>: <the capability, in a few words>

**What's missing**
One capability, in one or two sentences. What a mod can't do today.

**Where it belongs**
The R.* namespace or content kind that owns this concern, and why.

**Suggested shape**
Producer only: the event and its payload, the state to read, or the one call.
Nothing specific to this mod.

**Who uses it**
The mod's name (and a link, if public), and what it will do with it.

**What I tried**
Existing calls you checked and why each falls short, with file or page names.
Any fallback the mod uses meanwhile.
```

Describe the need. Addresses, patches and SDK code aren't required. If you found something out about the engine, include it and say whether it was seen in game or only worked out from reading.

### Bug in RSMM

Check that the mod isn't the cause: `rsmm lint <id>`, then try again with only this mod enabled. Then include what happened, what was expected, steps to reproduce, operating system, RSMM version, the whole `rsmm doctor` output, and the relevant `rsmm log` lines. Attach the smallest mod that shows it. Remove user names and tokens from pasted output.

Security problems never go in a public issue. Follow [SECURITY.md](https://github.com/Ravenswatch-Modding/RavenswatchModManager/blob/main/SECURITY.md). Problems with someone else's mod go to that mod's page.

## Before you say it's done

- [ ] Every file you changed is the mod's own, and `git status` in the RSMM clone shows nothing from you.
- [ ] Nothing was pushed to RSMM and no pull request was opened.
- [ ] `rsmm lint <id>` has no errors.
- [ ] Every `R.*` call, event, manifest key and content kind was looked up, not guessed.
- [ ] Rules and numbers are in the mod; nothing general was rebuilt inside it.
- [ ] Anything RSMM is missing is in an issue, or a draft waiting for the author.
- [ ] For a release package, you confirmed the intended behavior with the author and checked the relevant transitions in game.
- [ ] You told the author what was tested in game and what wasn't.
