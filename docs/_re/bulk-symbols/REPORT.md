# Bulk Symbol Recovery Report

Target build: `e04db5c457dd` (Steam build `26864515`, v1.6.2.3, `Ravenswatch.exe` SHA-256 `e04db5c457dd301016e2f22d18bd780b368ae9e3a34653fa363457b42dd17a68`).

---

## Phase 1: Bring corpus up to build e04db5c457dd (1.6.2.3)

**`FUN_` count after Phase 1:** **`59,619`** `FUN_*` out of `60,368` internal non-thunk functions in `docs/_re/out/symbols.json` (**`749`** named functions; baseline `symbols_b9c3.json` had `65,155` `FUN_*` / `0` named). Note: Phase 1.6 vtable typing ran after the Phase 1.5 `symbols.json` export and typed `10,986` functions without renaming them, so `FUN_` counts reflect Phase 1.5.

### Steps & Checks

1. **Vanilla prep & preflight**
   - Moved `46` leftover mod/RSMM files (`20` mod-added `_Cooking` files, `19` `*.rsmm.bak`, `.rsmm_state.json`, `.rsmm_state.json.pre-clear`, `.rsmm_game_version.json`, `winhttp.dll`, `rsmm/`, `mods/`, `rsmm_loader_flags.json`) from `E:\SteamLibrary\steamapps\common\Ravenswatch` to `E:\Documents\Projects\Ravenswatch\RSMM\evidence\2026-11-26-bulk-symbols\game-mod-stash\` (nothing deleted).
   - `python tools/update_symbols.py preflight` exited `0` (`OK`, `ghidra` unlocked, `exe e04db5c457dd -> no patch`).
2. **Phase 1.1 — Snapshot `e04db5c457dd`**
   - Output: `E:\Documents\Projects\Ravenswatch\RSMM\evidence\game-snapshots\e04db5c457dd\`
   - `Ravenswatch-e04db5c457dd.exe`: `22,531,144` bytes, SHA-256 `e04db5c457dd301016e2f22d18bd780b368ae9e3a34653fa363457b42dd17a68`
   - `appmanifest_2071280.acf`: `buildid` `26864515`
   - `listing.tsv` / `sha256sums.txt`: `24,059` files listed and hashed
   - `class_versions.json`: `583` cooked classes
3. **Phase 1.2 — Ghidra import & `RsmmNoReturnApply.java`**
   - Pre-import backup saved to `ghidra-backup\`; post-import backup saved to `ghidra-backup-post-import\`.
   - Auto-analysis completed in `856` seconds.
   - `noreturn_fix.log`: `functions_before 61150`, `noreturn_before 33`, target `FUN_140cbabe8@140cbabe8` (`_CxxThrowException@140cbd808` kept), `call_sites 5`, `parent_functions 4`, `tail_functions_folded 0`, `parents_fixed 4`, `body_growth_bytes 4`, `fallthroughs_inside_parent 5`, `functions_after 61150`, `noreturn_after 32`.
   - Project `E:\Documents\Ghidra\Ravenswatch` now holds both `/Ravenswatch.exe` (`b9c3`) and `/Ravenswatch-e04db5c457dd.exe` (`e04d`).
4. **Phase 1.3 — Apply RSMM knowledge (`k.json`)**
   - `tools/ghidra_apply_knowledge.py`: `398` symbol-map entries.
   - `ApplyRsmmKnowledge.java` (dry + real on `/Ravenswatch-e04db5c457dd.exe`): `9` missing `.pdata` function starts defined (`0` failed), `326` functions named, `72` data labels applied (`0` kept, `0` not a function start).
5. **Phase 1.4 — Port `b9c3` renames with BinDiff**
   - `BinExportHeadless.java`: `Ravenswatch-b9c3642884a8.BinExport` (`124,310,814` bytes), `Ravenswatch-e04db5c457dd.BinExport` (`124,460,741` bytes).
   - `bindiff.exe`: `Ravenswatch-b9c3642884a8_vs_Ravenswatch-e04db5c457dd.BinDiff` (`similarity=0.9759`, `confidence=0.9883`, `67,269` matched functions).
   - `k_with_bindiff.json`: `398` symbol-map entries + `10` ported names (`CustomFlagFilter_Serialize` at `0x140182160` and `9` `UndefinedFunction_*` placeholders from `b9c3`). Applied: `336` functions named (`0` failed), `72` data labels.
6. **Phase 1.5 — Corpus export**
   - Old `b9c3` files archived as `decompiled_b9c3.jsonl` and `symbols_b9c3.json`. Exported against read-only project copy `ghidra-ro\`.
   - `docs/_re/out_new/decompiled_new.jsonl`: `60,367` lines (`90,652,465` bytes; `ExportDecompiled.java`: `total=60581`, `written=60367`, `skipped_thunk=213`, `skipped_empty=0`, `errs=1`). Note on count vs `65,000`: `decompiled_b9c3.jsonl` (`65,010` lines, `20.5 MB`) was a synthetic `.pdata` stub dump, whereas `decompiled_new.jsonl` is full Ghidra C decompilation (`65,155` `.pdata` entries exist in `e04d`, and `13,277` of the `13,285` `.pdata`-only addresses lie inside existing Ghidra function bodies as chained unwind / cold EH blocks).
   - `docs/_re/out_new/vftables.jsonl`: `6,026` vftables.
   - `docs/_re/out_new/corpus.meta.json`: stamped with `e04db5c457dd` SHA-256 and verified via `assert_corpus_matches_exe()`.
   - `docs/_re/out/symbols.json`: `60,368` functions (`59,619` `FUN_`, `749` non-`FUN_`).
   - `docs/_re/out/strings.json`: `18,153` strings (`errs=0`) exported via `dump_strings.py` using `StringDataInstance` + `d.hasStringValue()` (`docs/_re/scripts/dump_symbols_strings.py` emitted `0` strings under PyGhidra and was left untouched in the repo).
7. **Phase 1.6 — Vtable typing (`rsmm_vtables.py`)**
   - `tools/gen_ghidra_vtables.py --game-only --corpus docs/_re/out_new`: `3,337` vtable structs (`50,222` slots, `1,933` semantically-named), `2,840` object structs (`1,997` sized > 8 B), `12,792` `this`-retypes, `2,267` ctors, `2,718` `ret_self`.
   - Applied via `pyghidraRun.bat --headless`: `2,840` vtable structs created / `3,337` stamped, `2,840` object structs created, `10,986` `this`-typed (`4,049` also return-typed), `0` `no-func`, `3,136` `no-params`, `1` `non-ptr-param0`, `0` `decompile-fail`, `0` `failed`.
   - Verification (`check_vcalls.py`): `HeroController_AddSkill` (`0x14039fec0`) resolves via pattern DB (its virtual call is on a local variable, not `this`); on typed methods (`0x1401cca80` `oCDtGame *`, `0x1401ccce0` `oCDtGame *`, `0x1401dc0c0` `oCDtGameScene *`), calls now render as `param_1->vftable->FUN_1401cca70()`, `param_1->vftable->FUN_1401ccc40(...)`, `param_1->vftable[2].FUN_14047a3c0(...)`, `param_1->vftable[3].FUN_14047a450(param_1)`.

### Operational & Handoff Notes

- **PowerShell headless invocation:** Passing `""` as `<vmarg-list>` directly to `support\launch.bat` in PowerShell drops the empty arg and shifts subsequent arguments (`ClassNotFoundException: ghidra.GhidraClassLoader`); setting `$env:GHIDRA_HEADLESS_MAXMEM = "6G"` and invoking `support\analyzeHeadless.bat` (with `"" | & ...` so `pause` never blocks on error) works cleanly. `RsmmNoReturnApply.java` resides in `tools\ghidra_scripts`, not `scripts\ghidra_scripts`.
- **MCP target program:** `tools/ghidra_mcp/serve.sh` serves `E:\Documents\Ghidra\Ravenswatch`, which contains both `/Ravenswatch.exe` (`b9c3`) and `/Ravenswatch-e04db5c457dd.exe` (`e04d`). Later sessions must specify **`Ravenswatch-e04db5c457dd.exe`** by name.

---

## Phase 2: Name class methods from RTTI

**`FUN_` count:** **`59,619`** -> **`36,189`** (`-23,430` `FUN_*`; `24,179` named out of `60,368` internal non-thunk functions in `docs/_re/out/symbols.json`).

### Method & Sanitization Rule

- **PE RTTI parsing:** Parsed MSVC x64 `CompleteObjectLocator` (`*(u64*)(vt - 8)`) and `ClassHierarchyDescriptor` directly from `Ravenswatch-e04db5c457dd.exe` for all `6,026` vtables (`100%` valid).
- **Name sanitization rule:** Strip trailing `::vftable`, replace whitespace (`\s+`) with `_`, replace `` ` ``, `'`, `"`, `;`, `/`, and control chars with `_`, keep C++ scope/template punctuation (`::`, `<`, `>`, `,`, `(`, `)`, `*`, `&`, `~`, `.`, `:`, `?`, `@`), cap class prefixes at `160` characters (`prefix[:150] + "_" + sha1[:8]`), and suffix global name collisions (`_2`, `_3`, ...). Secondary multiple-inheritance vtables (`this_off > 0`) prefix `sub0x<off>_`.
- **Slot 0 rule:** Phase 1 recon showed `3,857` of `6,026` vtables (primarily `oEngine` descriptor/type getters) place a static getter at slot 0 rather than a deleting destructor. Slot 0 is named `dtor_deleting` only when the decompiled body is `< 40` lines and matches scalar-deleting-destructor logic (`param_2 & 1` / `operator_delete` / `free(`); otherwise it is named `vf0`.
- **10-entry validity test (`rtti_test10.json`):** Applied to backup copy `ghidra-test-rtti\` (`10` named, `0` failed) and verified via `check_names10.py`: Ghidra stores `::` literally in the `Global` namespace and renders `::` as `__` in decompiled C (`oe__dt__RainRenderSettings__vf0`, `oCTGlobalResourceRef<oCEntitySettingsResource,0>__vf4`, `Stormancer__Steam__details__SteamImpl__sub0xa0_vf0`), keeping templates readable.

### Breakdown (`rtti_knowledge.json` — `23,430` entries)

| Category | Count | Notes |
|---|---:|---|
| Unique-vtable slots (`1` vtable) | `18,628` | Direct `<Class>::vf<i>` / `dtor_deleting` |
| Shared single-base slots (`2..50` vtables) | `1,550` | Base present in every sharer's `ClassHierarchyDescriptor` |
| Constructors / destructors (`ctor_or_dtor`) | `3,205` | Stores into `param_1`, unique most-derived `stored_tds` |
| Interface methods added (`mine_vtable_interfaces`) | `47` | Enriched `765` existing slot notes across `83` interface slots |
| **Total applied** | **`23,430`** | `0` failed, `0` skipped during `ApplyRsmmKnowledge.java` |
| Slot 0 split | `1,548` `dtor_deleting` / `3,480` `vf0` | Prevents mislabeling `oEngine` getters |
| Skipped | `50` stubs (`>50` vts), `1,066` ambiguous shared, `861` ambiguous ctors, `239` already named | Conservative no-guess policy |

Re-exported corpus (`docs/_re/out_new/`): `60,367` decompiled functions, `6,026` vftables, `corpus.meta.json` verified, `18,078` strings (`0` errors).

### 10 Random Newly-Named Functions (`seed=20261126`)

1. `0x140835a70` — `oIEntityGpnTesterSettings::vf3`: `oIEntityGpnTesterSettings::vftable slot 3`
2. `0x1407cf7c0` — `oCEntityCpntGpnProximity::vf28`: `oCEntityCpntGpnProximity::vftable slot 28`
3. `0x140127430` — `std::_Ref_count_obj2<Stormancer::Users::RenewCredentialsParameters>::vf0`: `...::vftable slot 0`
4. `0x140ae04a0` — `RakNet::RakPeer::vf23`: shared by `oCSLNetPeer::vftable slot 23` and `RakNet::RakPeer::vftable slot 23` (unique base `RakNet::RakPeer`)
5. `0x140bed420` — `std::_Func_impl_no_alloc<<lambda_abf9c921ca92760965cc7cdb92203635>,pplx::task<std::shared_ptr<Stormancer::IP2PScenePeer>_>,Stormancer::AuthorizeP2PResult>::vf0`: `...::vftable slot 0`
6. `0x1401d94c0` — `oe::dt::RefugeeScenarioEntityCpntDataSettings::vf1`: `oe::dt::RefugeeScenarioEntityCpntDataSettings::vftable slot 1`
7. `0x1405ddb70` — `oCShaderGraphNodeMultiplyColor::vf0`: `oCShaderGraphNodeMultiplyColor::vftable slot 0` (`undefined8 oCShaderGraphNodeMultiplyColor__vf0(void)`)
8. `0x1406454f0` — `oC3dParticleEffectorMoveToOrigin::ctor_or_dtor`: `*param_1 = &oIRsRuntime::vftable; *param_1 = &oI3dParticleEffector::vftable; ...` (3 vtables, most-derived `oC3dParticleEffectorMoveToOrigin`)
9. `0x140b6cbd0` — `std::_Func_impl_no_alloc<<lambda_ab1f5b6f5b152edb897fb327aa1e8c4a>,void,Stormancer::obytestream&___ptr64>::vf4`: `...::vftable slot 4`
10. `0x140491dc0` — `oCGameLevelIdentifier::vf3`: `oCGameLevelIdentifier::vftable slot 3`

---

## Phase 3: Identify statically linked libraries

**`FUN_` count:** **`36,189`** before -> **`36,189`** after (`delta = 0`).

### MSVC CRT/STL Function ID (`apply_msvc_fid.py`)

- **Why delta is `0`:** Ghidra `12.1.4` auto-analysis during Phase 1.2 already enabled and ran all 5 shipped x64 Function ID databases (`vsOlder_x64.fidbf`, `vs2012_x64.fidbf`, `vs2015_x64.fidbf`, `vs2017_x64.fidbf`, `vs2019_x64.fidbf`), accounting for **`382`** `SourceType.ANALYSIS` CRT/STL functions (`34` `IMPORTED`, `23,763` `USER_DEFINED`, `36,189` `DEFAULT`/`FUN_`). Re-running `FidAnalyzer` across the executable section found `0` additional `FUN_` matches.
- **Compiler / Toolset (PE Optional & Rich Headers):** Linker `14.29` (VS 2019 16.10/16.11); Rich Header object counts show mixed static-lib/main builds: `33145` (`354` objs — VS 2022 17.8), `30159` (`336` objs — VS 2019 16.11), `30034` (`128` objs — VS 2019 16.10), `29111` (`44` objs — VS 2019 16.7). Ghidra `12.1.4` does not ship a VS 2022 `.fidbf`.

### Confirmed Statically Linked Libraries (`audit_libraries.py`)

| Library | Proving String(s) / Banner | Vtables | FID DB Build Effort |
|---|---|---:|---|
| **MSVC CRT / STL** | `'Microsoft Visual C++ Runtime Library'`, `'minkernel\crts\ucrt\inc\corecrt_internal_strtox.h'` | `1,383` (`std::`) | **Low (1–2 hr):** Build VS 2022 17.8 (`19.38` / build `33145`) + VS 2019 (`14.29`) static CRT/STL `.lib` FID DBs; Ghidra already ships `vs2019_x64`. |
| **cpprestsdk / pplx** | `'cpprestsdk/2.8.01'` | `959` (`pplx::`) | **Medium (3–5 hr):** Exact version `2.8.01` known; CMake/VS build against VS 2019/2022 (or vcpkg `cpprestsdk` `2.8`). |
| **Stormancer + RxCpp + ctti** | `'D:\Project\oEngine\DarkTales\oeMain\ThirdParty-Ext\Stormancer\stormancer\stormancer-sources\src\P2P\RakNet\P2PTunnels.cpp'`, `'Stormancer_ConnectFail_BadGameVersion'`, `'ctti::pretty_function::type<class Stormancer::Client>'` | `135` (`Stormancer::`) + `42` (`rxcpp::`) | **High (1–2 days):** Requires Passtech's ~2018 Stormancer fork (header/ABI skew noted in `data/netcode_interfaces.json`) + `rxcpp`/`ctti`/`cpprestsdk` dependency wiring. |
| **RakNet** | `'C:\Workspace\oengine2\ThirdParty-Ext\RakNet\src\RakPeer.cpp'`, `'RAKNET_STARTED'`, `'BR_REQUIRES_RAKNET_SUPPORT_IPV6_DEFINE'` | `19` (`RakNet::`) | **Low–Medium (2–4 hr):** Stormancer's `RakNet` fork builds standalone with VS; note `Replica3` has 6 extra slots vs upstream 2018 header. |
| **Bullet Physics** | `'*_SHAPE_PROXYTYPE'` (`BOX_SHAPE_PROXYTYPE`, `CAPSULE_SHAPE_PROXYTYPE`, ...), `'.?AVbtKinematicCharacterController@@'` | `11+` (`bt*`) | **Low–Medium (2–4 hr):** Standard CMake VS static build (`BulletCollision`/`BulletDynamics`/`LinearMath`). |
| **Recast / Detour** | `'C:\Perforce\Ext\Detour\src\Source\DetourNavMeshQuery.cpp'`, `'Recast.cpp'`, `'DetourNode.cpp'` | `—` (non-virtual C++) | **Low (1–2 hr):** Small self-contained C++ source set (`Recast` + `Detour`). |
| **Crashpad / mini_chromium** | `'../../client/crashpad_client_win.cc'`, `'../../third_party/mini_chromium/mini_chromium/base/rand_util.cc'` | `18` (`crashpad::`) | **Medium–High (4–8 hr):** Requires Chromium `gn`/`ninja` Windows build matching the shipped snapshot. |
| **D3D12 Memory Allocator** | `D3D12MA::*` RTTI vtables | `8` (`D3D12MA::`) | **Low (1 hr):** Single-header/single-TU library (`D3D12MemAlloc.cpp`). |
| **msgpack-c (`v1`) & RapidJSON** | `'application/msgpack'`, `msgpack::v1::parse_error`, `rapidjson::GenericDocument` (RTTI strings) | `12` (`msgpack::`) | **N/A (header-only):** Templates instantiated into callers; FID adds little beyond RTTI slot names. |
| **Dear ImGui** | `'Custom ImGui Window'`, `'.?AVImguiLayoutWindow@oe@@'` | `—` | **Low (1–2 hr):** Small C++ source set if debug UI paths aren't stripped. |

*(Excluded — dynamically linked DLLs in the game root: `EOSSDK-Win64-Shipping.dll`, `fmod.dll`, `fmodstudio.dll`, `steam_api64.dll`, `libScePad.dll`, `xaudio2_9redist.dll`.)*

---

## Phase 4: String and table anchors

**`FUN_` count:** **`36,189`** -> **`30,616`** (`-5,573` existing `FUN_*`; `29,829` named out of `60,445` internal non-thunk functions in `docs/_re/out/symbols.json`; `946` newly-defined trampoline/callback functions in Ghidra, of which `77` were non-thunks exported to `symbols.json` and `869` were `isThunk()` `E9` jumps).

### Breakdown (`phase4_knowledge.json` — `6,520` functions + `1,146` data labels = `7,666` entries)

| Sub-step | Count | Notes |
|---|---:|---|
| 4.3 Registrars (`<Class>::Register`) | `1,602` | Mapped chained `.pdata` offsets from `mine_class_ids.py` (`1,604` classes) to containing Ghidra functions |
| 4.3 Callbacks (`meta_cb0..2`, `_impl`, shared) | `1,538` | `861` `meta_cb0`, `215` `meta_cb1`, `32` `meta_cb2`, `428` `meta_cb*_impl` (via `E9` target), `2` shared (`oCClassDescriptor::shared_meta_cb2_dtor` @ `0x1401a1e60` across `1,469` classes) |
| 4.1 Event initializers (`InitEvent_<EVENT>`) | `1,316` | Matched all `150` event string VAs from `mine_event_names.py` (`NamedEvent_Id_FromCrc` callers) |
| 4.1 Event globals (`g_EventId_<EVENT>`) | `1,146` | Data labels for unique event-ID globals initialized by `InitEvent_*` |
| 4.1 Event emitters (`Emit_<EVENT>`) | `16` | Single-event `NamedEvent_Dispatch` callers (`8` more already named in `symbols.json`, `6` multi-event) |
| 4.2 Unique string anchors (`str_<text>`) | `2,014` | `207` priority-1 (`Foo::Bar`, `[Subsys]`, source/asset paths) + `1,807` priority-2 (`3,884` total unique-ref `FUN_` functions scanned) |
| 4.4 Wrappers (`<callee>_thunk`) | `34` (`16` non-thunk) | Fixed-point pass (1 iteration); excludes `atexit`/allocator/guard helpers |
| **Apply (`ApplyRsmmKnowledge.java`)** | **`6,520` fns + `1,146` data** | `0` failed, `0` skipped, `946` newly defined |

Re-exported corpus (`docs/_re/out_new/`): `60,444` decompiled functions (`total=61,528`, `skipped_thunk=1,083`, `errs=1`), `6,026` vftables, `corpus.meta.json` verified, `18,078` strings (`0` errors).

### Sample Anchors Verified in `symbols.json`

- **Registrars:** `0x140171530` (`oCResourceRef::Register`), `0x140171740` (`oISerializable::Register`), `0x1401718f0` (`oIAccess::Register`)
- **Meta callbacks:** `0x1400bb8c0` (`oCResourceRef::meta_cb2_impl`), `0x1400c0040` (`oCEntityCpntValuePicker::meta_cb0_impl`)
- **Events:** `0x140001060` (`InitEvent_NETWORK_DAMAGE`), `0x1400013a0` (`InitEvent_GAIN_HEALTH`), `Emit_*` (`16` emitters)
- **Strings:** `0x140002020` (`str_DarkTalesResources`), `0x14000f840` (`str_Dt_timers`), `0x14000f8d0` (`str_Dt_enemy_generation`)
- **Wrappers:** `0x1400c1e00` (`oe::ValueSignalBase::ctor_or_dtor_thunk`), `0x1400d56a0` (`oCString_Dtor_thunk`), `0x1400fea30` (`_Mtx_unlock_thunk`)

---

## Phase 5: Reflection probe

**`FUN_` count:** **`30,616`** (unchanged; read-only probe).

### Verdict

**Fields are declared by name at `0x14050a060` (`MetaClass_AddMember`) for a limited subset of `144` runtime/VFX/UI-controller/math/settings classes — NOT for asset-definition (`oCDt*` / `oe::dt::*Definition`) classes.**

- **One class's full list — `oC3dTrailParticleRendererSettings` (`"Trail Fx"`, version `0x20001`):**
  - Registrar: `0x14046ad40` (`oC3dTrailParticleRendererSettings::Register`) -> passes metaclass init callback `0x14045fd70` (`str_m_fSegmentLengthMax`) as arg 3 to `0x14050b970`
  - Calls `MetaClass_AddMember` (`0x14050a060`) on `DAT_141479040` (`oC3dTrailParticleRendererSettings` metaclass) with:
    1. `m_fSegmentLengthMax` (id `0x7edaac3`, `oCTMetaAttribute<float>` via `0x1404660b0`)
    2. `m_fPersistence` (id `0x7edaf8a`, `oCTMetaAttribute<float>`)
    3. `m_fSizeFactor` (id `0x7edaf8b`, `oCTMetaAttribute<float>`)
    4. `m_oTrailSettings` (id `0x7ed8ab8`, member via `0x140466110`)

### Places Checked

1. **Registrar `+0x88` callbacks (`meta_cb0..2`) vs 3rd-arg metaclass callback:**
   - Checked `oC2dAnimPictureDesc` (`0x14057e750`): `meta_cb0_impl` (`0x140573c00`) is placement default-constructor, `meta_cb1_impl` (`0x14057fae0`) is copy-constructor, `shared_meta_cb2_dtor` (`0x1401a1e60`) calls `vftable[2](this, 0)` (in-place destructor). None of `meta_cb0..2` declare fields.
   - Instead, the 3rd argument passed to `0x14050b970` (`0x140573bc0` `str_2D_anim_picture` for `oC2dAnimPictureDesc`) sets the display name (`"2D anim picture"`) and version (`0x30001`), and for `144` classes also calls `MetaClass_AddMember` (`0x14050a060`) to register `oIMetaAttribute` fields. Confirming the existing RE note on `MetaClass_AddMember`, retail strips field registration from asset definitions.
2. **Versioned serialize paths:**
   - `oCDtHeroProfileData::vf2` (`0x1400c2ff0`) is a deleting destructor; `0x140182160` (ported from `b9c3` as `CustomFlagFilter_Serialize`) is actually `oCCustomFlagFilter`'s registrar (`id=0x15a9d9bf`, `size=0x38`).
   - `oe::dt::RefugeeArchetypeDesc::vf3` (`0x1401fabe0`) and `FUN_1402116f0` (`0x1402116f0`) pass field strings (`"m_sName"`, `"m_u32Val1".."m_u32Val4"`) through `oCArchive` vtable slots `+0x60`/`+0x90`, accounting for the remaining non-`MetaClass_AddMember` `m_*` refs.
3. **`strings.json` `m_*` and camelCase runs:**
   - Found `284` `m_*` strings across `.rdata`, referenced by `145` functions total (`144` are `MetaClass_AddMember` init callbacks; `m_fVelocityScale` at `0x140f48e30` sits in one of those steering/particle attribute init functions).
   - Found `39` camelCase string runs (`>=6` within `0x80` bytes), which are `oC*` / `oI*` / `oe*` class-name pools (`0x140ed7d98..`, `0x140f025d8..`, etc.) rather than field-name tables.

## Phase 6: Proposed names (Hero / Skills / XP / Status / Modifiers)

**`FUN_` count:** **`30,550`** (down from `30,616`, `-66`; `29,895` named out of `60,445` total internal non-thunk functions in `docs\_re\out\symbols.json`).

- **Method:** Started from the `67` Hero / Skill / Status / Talent / Modifier / XP / Ability function seeds in `data\symbols.json`, inspected decompiled C bodies in `docs\_re\out_new\decompiled_new.jsonl`, and proposed `p_*`-prefixed names strictly where callers, callees, vtables, strings, or struct offsets provided concrete justification.
- **Artifact:** `E:\Documents\Projects\Ravenswatch\RSMM\evidence\2026-11-26-bulk-symbols\proposed-hero-skills.json` (`66` entries, `kind: "function"`, no `"force"` flag). **`data\symbols.json` was not modified.**
- **Subsystem breakdown (`66` named):**
  - HeroController / SkillController / AbilityController / GroupLevel (`14`): e.g. `p_HeroController_CountHeldSkills`, `p_HeroController_RefreshSkillSlotStates`, `p_HeroController_RecountSkillTiers`, `p_HeroController_RecountSkillCategories`, `p_HeroController_TickAll`, `p_SkillController_ApplyTierSignals`, `p_GuidVector_InsertUnique`, `p_GuidVector_Remove`.
  - XpComponent (`5`): `p_XpComponent_GetLevel`, `p_XpComponent_GetXp`, `p_XpComponent_GetXpForCurrentLevel`, `p_XpComponent_AddLevels`, `p_XpComponent_SyncStatReportLevel`.
  - EntityValueStore / Modifiers (`17`): `p_ModifierValueDesc_Alloc`, all 7 `p_EntityValueModifier*Computer_Alloc` factories (`Add`/`Sub`/`Mult`/`Max`/`Min`/`Reduce`/`Oldest`), `p_EntityValueStore_GetModifierHolderPersistentData`, `p_EntityValueModifierList_ForEach`, `p_EntityValueModifierList_InsertAndNotify`, `p_EntityValueStore_HasOverrideKey`, `p_EntityValueStore_RemoveModifierById`, `p_EntityValueStore_EraseModifierFromTable`, `p_CustomizeChallengePage_UpdateModifierSelection`, `p_ModifierSignal_UnsubscribeHandler`, `p_SignalChannel_DispatchPtrU32`, `p_EntityValueUnion_ResolveResource`.
  - Entity / Combat / Signals / Engine utilities (`30`): `p_EntityComponent_Deactivate`, `p_EntityComponentSettings_ResolveDisplayName`, `p_AttackHit_ResolveSourceComponent`, `p_Entity_GetOwnerPeerId`, `p_RemoteDamageTarget_GetOrLogModifierSlot`, `p_CombatEvent_FinalizeAndDestroy`, signal channels/closures/nodes (`7`), meta-class registration (`2`), `p_TlsRng_NextFloat01`, `oCString` helpers (`5`), `CustomFlagList` helpers (`2`), `Vector*` helpers (`5`), `p_RefCountedPtr_Assign`.
- **Verification:**
  - `ApplyRsmmKnowledge.java` dry-run & apply on `/Ravenswatch-e04db5c457dd.exe`: `66 named (0 newly defined), 0 kept an existing user name, 0 not a function start, 0 data label(s)`, save succeeded.
  - `dump_symbols_strings.py` + `dump_strings.py` refreshed `docs\_re\out\symbols.json` (`60,445` total, `30,550` `FUN_`, `29,895` named, `66` `p_*`) and preserved `docs\_re\out\strings.json` (`18,078` strings).

## Phase 7: Coverage diff experiment (DynamoRIO `drcov`)

**`FUN_` count:** **`30,550`** (experiment only; no symbols added in Phase 7).
**Status:** **Completed cleanly** (vanilla game restored, both runs captured & diffed, `steam_appid.txt` cleaned up).

1. **Step 7.1 — Anti-cheat check:** Clear in both the prior log (`logs_2026-11-22_21h-03m-15s.txt`) and the instrumented boot logs (`logs_2026-11-27_01h-05m-05s.txt`, `logs_2026-11-28_05h-53m-42s.txt`):
   - No `EasyAntiCheat` or `BattlEye` directory exists under `E:\SteamLibrary\steamapps\common\Ravenswatch`.
   - `EOS (LogEOSAntiCheat): [AntiCheatClient] Anti-cheat client not available. Verify that the game was started using the anti-cheat bootstrapper if you intend to use it.`
2. **Step 7.2 — DynamoRIO `drcov` gameplay runs & frame rate:**
   - **Does `Ravenswatch.exe` run under DynamoRIO?** **Yes**, cleanly and smoothly. Once past initial startup JIT warmup, the user reported **~144 FPS with no noticeable lag** (*"probably full 144, can't be sure but didn't see any lag"*).
   - **Run 1 — Baseline (hub idle ~30s):** `drcov-baseline\drcov.Ravenswatch.exe.06300.0000.proc.log` (`9,975,129` bytes, `drcov` v3): `1,243,681` total BBs across all modules, `268,050` unique `Ravenswatch.exe` BBs mapped to **`30,115`** unique Ghidra functions (`5` unmapped BBs). Prior 2026-11-26 boot-only capture archived in `drcov-boot-test-2026-11-26\`.
   - **Run 2 — Action (same hero, hub idle ~30s + 1 dash):** `drcov-action\drcov.Ravenswatch.exe.03384.0000.proc.log` (`9,915,313` bytes, `drcov` v3): `1,236,204` total BBs across all modules, `270,198` unique `Ravenswatch.exe` BBs mapped to **`30,294`** unique Ghidra functions (`5` unmapped BBs).
3. **Step 7.3 — Coverage diff (`diff_drcov.py` → `drcov-diff.json`):**
   - **Shared functions:** `30,076` (`99.87%` baseline overlap)
   - **Baseline-only functions:** `39`
   - **Action-only functions (`action - baseline`):** **`218`** (**`0.36%`** of the `60,445`-function binary; **`117`** named + **`101`** `FUN_*`).
   - **Notable Dash / Movement / VFX hits inside the `218` action-only functions:**
     - `0x1402c85b0` (`str_NotADash`) plus surrounding `0x1402c63c0..0x1402ca130` (`11` `FUN_*` functions in the Dash/Ability neighborhood)
     - `0x14073f500` / `0x14073f640` (`oCEntityCpntBasicMove::vf28` / `vf29`)
     - `0x1403d52b0` (`RemoteDamageTarget_OnModifierRemoved`)
     - Dash visual/animation effects: `oCDtSurfaceParticleEmitter::*`, `oCVisualEffectBeam::*`, `oC3dBillboardBeamRenderer::*`, `oCEntityCpntAnimatedBoneController::*`.
4. **Cleanup:** Removed temporary `E:\SteamLibrary\steamapps\common\Ravenswatch\steam_appid.txt` (`update_symbols.py preflight` exited `0`, `OK`). User's stashed mod files & settings remain intact in `game-mod-stash\` and `game-mod-stash-2026-11-27\`.

### Phase 7b: Proposed names for the 101 unnamed functions in the Dash `drcov` diff

**`FUN_` count:** **`30,449`** (down from `30,550`, `-101`; **`29,996`** named out of `60,445` total internal non-thunk functions in `docs\_re\out\symbols.json`; `167` `p_*`).

- **Artifact:** `E:\Documents\Projects\Ravenswatch\RSMM\evidence\2026-11-26-bulk-symbols\proposed-dash-drcov.json` (`101` `p_*` entries covering every remaining `FUN_*` in `drcov-diff.json`, no `"force"`, `data\symbols.json` untouched).
- **Groups named (`101`):** AbilityController & Dash pipeline (`12`), HeroController ability/input/HUD & combat tags (`10`), BasicMove / CollisionGpn / Bullet physics / ZoneAttack / Modifiers / FMOD (`18`), 3D beam & particle VFX + `10` `*_GetMetaClass` getters (`28`), Reward/Sort/RefCount/Switch helpers (`6`), Stormancer / `pplx` async tasks & MSVC EH funclets (`27`).
- **Verification:** `ApplyRsmmKnowledge.java` named `101` (`0` failed, `0` skipped); refreshed `docs\_re\out\symbols.json` (`60,445` total, `30,449` `FUN_`, `29,996` named) and `docs\_re\out\strings.json` (`18,078`); re-ran `diff_drcov.py` → **`Action-only named=218, unnamed FUN_=0`** (`100%` of the Dash diff named).

### Phase 7c: Run 3 — Full Hero Kit + Starting Talent Selection in Hub (`drcov-run3-kit`)

**`FUN_` count:** **`30,311`** (down from `30,449`, `-138`; **`30,134`** named out of `60,445` total internal non-thunk functions in `docs\_re\out\symbols.json`; `305` `p_*`).

- **Capture (`drcov-run3-kit\drcov.Ravenswatch.exe.39744.0000.proc.log`, `11,039,049` bytes, `drcov` v3):** `1,376,671` total BBs across all modules, `273,064` unique `Ravenswatch.exe` BBs mapped to **`30,465`** unique Ghidra functions (`5` unmapped BBs). User played Scarlet in the hub, picked the 4th starting talent to unlock Trait, and cast Attack, Power, Special, Defense, and Trait before exiting cleanly.
- **Cumulative diff (`drcov-diff-run3-kit.json`, `run3 - (baseline | dash)` where `|baseline | dash| = 30,333`):** **`263`** new functions (`125` already named + **`138`** `FUN_*`). Across Runs 1–3 combined, **`30,596`** unique functions have executed (`15,908` named / `14,688` `FUN_*` including baseline).
- **Notable systems hit:**
  - Starting-talent unlock bonus-captured the full talent selection/rolling/UI flow: `p_HeroController_SkillSelectionMenuState`, `p_HeroController_RollSkillChoices`, `p_HeroController_PopulateSkillChoiceCard`, `p_SelectionMenuUiController_OpenChoiceList`, `p_SelectionMenuUiController_PushFocusedUpdateGs`/`PopFocusedUpdateGs`/`ApplySoloPause`, `p_SkillController_CompareChoiceOrder`, `p_SkillUnlock_GetRequiredHeroRank`, `p_SkillController_ComputeUpgradeCost`, `p_SkillUiViewer_*`, `Analytics_EmitSkillProposed`, `Analytics_EmitSkillSelected`, `HeroController_AddSkill`.
  - Combat / Damage / Crit / Healing / AttackCombo: `p_CharacterController_PopulateDamageFromStats`, `p_CharacterController_GetAbilitySlotDamageBonus`, `p_HeroController_ApplyAbilityDamageModifiers`, `p_EntityCpntDamage_GetSlotDamageMultiplierBonus`, `p_EntityCpntDamage_GetSlotCritChanceBonus`, `p_EntityCpntHittable_ProcessIncomingHit`, `p_EntityCpntZoneAttack_ApplyHitsToTargets`, `p_EntityCpntHitPoint_AddHealth`/`RemoveHealth`/`OnGainHealthEvent`, `p_AttackCombo_TickAll`/`StartStep`/`StopStep`, `p_SwissTable_*DamageKey*`.
  - Bullet physics collision & GJK/EPA sweep/penetration solver (`35` functions): `p_btCollisionWorld_ObjectQuerySingleInternal`, `p_btContinuousConvexCollision_ComputeClosestPoints`, `p_btGjkPairDetector_GetClosestPointsNonVirtual`, `p_btGjk_*` (`8`), `p_btGjkEpa_*` (`15`), `p_btConvexShape_*` (`3`), `p_btRigidBody_ApplyImpulse`/`ApplyTorqueImpulse`, `p_BulletRigidBodyOwner_*` (`2`).
- **Artifact:** `E:\Documents\Projects\Ravenswatch\RSMM\evidence\2026-11-26-bulk-symbols\proposed-run3-kit-drcov.json` (`138` `p_*` entries covering every remaining `FUN_*` in `drcov-diff-run3-kit.json`, no `"force"`, `data\symbols.json` untouched).
- **Verification:** `ApplyRsmmKnowledge.java` dry-run & apply on `/Ravenswatch-e04db5c457dd.exe`: `138 named (0 newly defined), 0 kept an existing user name, 0 not a function start, 0 data label(s)`, save succeeded; refreshed `docs\_re\out\symbols.json` (`60,445` total, `30,311` `FUN_`, `30,134` named, `305` `p_*`) and `docs\_re\out\strings.json` (`18,078`); recomputed `drcov-diff-run3-kit.json` → **`run3_only_total=263, run3_only_named=263, run3_only_unnamed_fun=0`** (`100%` of Run 3 diff named).

---

## Summary: `FUN_` Progression (Phases 1–7c)

| Phase | Action | Remaining `FUN_` | Delta | Notes |
|---|---:|---:|---:|---|
| 1 | Snapshot, Ghidra import (`61,150`), knowledge (`k_with_bindiff.json`), vtable structs (`2,840`) | `59,619` | — | Corpus exported (`60,368` internal non-thunk functions) |
| 2 | RTTI PE vtable & ctor/dtor mining + interface slots (`23,430` named) | `36,189` | `-23,430` | `0` failed, `10/10` sample verified |
| 3 | Static library audit (`10` libs) + MSVC CRT/STL FID check | `36,189` | `0` | `382` CRT/STL functions already named by Phase 1.2 analyzer |
| 4 | Registrars (`1,602`), `meta_cb*` (`1,538`), events (`1,332`), `str_*` (`2,014`), thunks (`34`) | `30,616` | `-5,573` | `+1,146` `g_EventId_*` data labels, `+946` new trampoline defs |
| 5 | Read-only reflection probe | `30,616` | `0` | Confirmed `MetaClass_AddMember` (`0x14050a060`) on `144` runtime/settings classes |
| 6 | Proposed `p_*` Hero / Skills / XP / Modifier / Engine names (`66` named) | `30,550` | `-66` | `29,895` named / `60,445` total; `data\symbols.json` untouched |
| 7 | DynamoRIO `drcov` idle vs. `+1 dash` diff (~`144` FPS) | `30,550` | `0` | `30,294 - 30,115` → **`218` action-only functions** (incl. `str_NotADash`) |
| 7b | Proposed `p_*` names for all `101` unnamed Dash-diff functions | `30,449` | `-101` | `218/218` (`100%`) of Dash diff named; `29,996` named / `60,445` total |
| 7c | Run 3 (`drcov-run3-kit`): full Scarlet kit + starting talent diff (`138` named) | **`30,311`** | **`-138`** | **`263/263` (`100%`) of Run 3 diff named**; **`30,134`** named / `60,445` total |
