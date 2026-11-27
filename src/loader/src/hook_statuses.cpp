// Custom status registration — see hook_statuses.h for the rationale.
//
// RE: evidence/2026-11-26-custom-status/README.md (Q1, Q2) and docs/
// 2026-11-26-custom-status.md. Static-only: nothing here has been run in game.
//
// The engine builds a status definition with one helper, ModifierSettings_
// CreateDesc(mgr, id, &name, &category, &default, &str2). It allocates the
// oSModifierValueDesc, appends it to the manager's vector (+0x20/+0x28) and
// THEN inserts it in the id hash (+0x30), so a second registration of one id
// leaves an orphan in the vector. Everything below exists to call it exactly
// once per id, on the game thread, with arguments the engine can't choke on.

#include "hook_statuses.h"
#include "hook_util.h"
#include "symbols.gen.h"  // GENERATED — Sym::
#include "loader.h"
#include "mem_safe.h"

#include "json.hpp"

#include <windows.h>

#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <mutex>
#include <string>
#include <vector>

namespace rsmm {
namespace {

// --- manager / definition layout (README Q2) -----------------------------
constexpr std::size_t kMgrVecData  = 0x20;  // oSModifierValueDesc** (data)
constexpr std::size_t kMgrVecCount = 0x28;  // u32 count
constexpr std::size_t kMgrMinSize  = 0x50;  // through the id hash at +0x30
constexpr std::uint32_t kMgrVecMax = 0x10000;
constexpr std::uint32_t kSectionMax = 256;

constexpr std::size_t kDefMaxStacks = 0x68;  // u32, 0 = unlimited
constexpr std::size_t kDefId        = 0x6c;  // u32
constexpr std::size_t kDefReapply   = 0x70;  // u32
constexpr std::size_t kDefCombine   = 0x74;  // u32
constexpr std::size_t kDefInserted  = 0x79;  // u8, 1 after a successful hash insert
constexpr std::size_t kDefSize      = 0x80;

// AppSettings section vtable slots the engine's own walk uses (d5.c).
constexpr std::size_t kVtType   = 0x00;  // section -> type object
constexpr std::size_t kVtIsKind = 0x58;  // type -> bool IsKindOf(class)

// oCEntityValueUnion (0x20 bytes): +0x08 storage tag (4 = inline), +0x10
// inline data, +0x18 type tag, +0x19 float sub-flag (registrar sets 2).
constexpr std::size_t kUnionSize    = 0x20;
constexpr std::size_t kUnionStorage = 0x08;
constexpr std::size_t kUnionInline  = 0x10;
constexpr std::size_t kUnionType    = 0x18;
constexpr std::size_t kUnionFloatFlag = 0x19;
constexpr std::uint32_t kTypeNumeric = 0;  // float and int
constexpr std::uint32_t kTypeBool    = 2;

constexpr std::uint32_t kLiteralBit = 0x80000000u;
constexpr std::size_t kNameMax = 128;

// The 16-byte engine string descriptor {char*, len | literal bit}.
struct StringDesc {
    const char* ptr;
    std::uint32_t lenflags;
    std::uint32_t pad;
};

using CreateDesc_t = void* (*)(void* mgr, std::uint32_t id, void* name, void* category,
                               void* default_value, void* str2);
using UnionCtor_t     = void* (*)(void* u);
using UnionInitType_t = void (*)(void* u, std::uint32_t type);
using UnionDtor_t     = void (*)(void* u);
using SectionType_t   = void* (*)(void* section);
using IsKindOf_t      = std::uint8_t (*)(void* type, void* cls);
using Register_t      = void (*)();
using StatusInit_t    = void (*)(void* self);

enum class ValueKind { Float, Int, Bool };

struct StatusDef {
    std::string mod;
    std::string key;
    std::string name;
    const char* name_c = nullptr;  // leaked: literal-bit strings are not copied
    std::uint32_t id = 0;
    ValueKind kind = ValueKind::Float;
    std::uint32_t combine = 0;
    std::uint32_t reapply = 0;
    std::uint32_t max_stacks = 0;
};

std::vector<StatusDef> g_defs;
// Registration is attempted until it reaches CreateDesc, which is the first call
// with side effects (a second registration of one id leaves an orphan in the
// manager's vector). A failed manager lookup has none, so it does not consume
// the attempt and the other detour may still register. Both detours run on the
// game thread; the mutex only makes the flag and the one-time log exact.
std::mutex g_register_mu;
bool g_done = false;            // set right before the first CreateDesc call
bool g_logged_no_mgr = false;   // the not-found error is logged once

CreateDesc_t    g_create = nullptr;
UnionCtor_t     g_union_ctor = nullptr;
UnionInitType_t g_union_init = nullptr;
UnionDtor_t     g_union_dtor = nullptr;
Register_t      g_real_register = nullptr;
StatusInit_t    g_real_init = nullptr;
std::uintptr_t  g_register_va = 0;
std::uintptr_t  g_init_va = 0;

// --- helpers -------------------------------------------------------------

std::uintptr_t image_base() {
    HMODULE h = GetModuleHandleA("Ravenswatch.exe");
    if (!h) h = GetModuleHandleA(nullptr);
    return reinterpret_cast<std::uintptr_t>(h);
}

std::uintptr_t reloc(std::uintptr_t va) {
    return va - Sym::kPreferredBase + image_base();
}

std::string hex32(std::uint32_t v) {
    char b[16];
    snprintf(b, sizeof(b), "0x%08x", v);
    return b;
}

// Leak a stable C-string, as hook_skins.cpp does. nullptr on OOM.
const char* dup_cstr(const std::string& s) {
    char* p = static_cast<char*>(malloc(s.size() + 1));
    if (!p) return nullptr;
    std::memcpy(p, s.c_str(), s.size() + 1);
    return p;
}

// Standard reflected CRC-32 (poly 0xEDB88320), the same as zlib.crc32.
std::uint32_t crc32(const std::uint8_t* data, std::size_t n) {
    std::uint32_t c = 0xFFFFFFFFu;
    for (std::size_t i = 0; i < n; ++i) {
        c ^= data[i];
        for (int b = 0; b < 8; ++b) c = (c >> 1) ^ (0xEDB88320u & (0u - (c & 1u)));
    }
    return ~c;
}

// named_id.of(): crc32 of the key, then crc32 of the 8 bytes
// [0,c0, 0,c1, 0,c2, 0,c3] (namespace 0 and the CRC interleaved, little-endian).
std::uint32_t status_id(const std::string& key) {
    const std::uint32_t c = crc32(reinterpret_cast<const std::uint8_t*>(key.data()), key.size());
    const std::uint8_t b[8] = {0, static_cast<std::uint8_t>(c), 0,
                               static_cast<std::uint8_t>(c >> 8), 0,
                               static_cast<std::uint8_t>(c >> 16), 0,
                               static_cast<std::uint8_t>(c >> 24)};
    return crc32(b, sizeof(b));
}

// --- statuses.json loading -----------------------------------------------

bool parse_uint(const nlohmann::json& v, std::uint64_t max, std::uint64_t* out) {
    if (!v.is_number_integer()) return false;
    if (v.is_number_unsigned()) {
        *out = v.get<std::uint64_t>();
    } else {
        const auto s = v.get<std::int64_t>();
        if (s < 0) return false;
        *out = static_cast<std::uint64_t>(s);
    }
    return *out <= max;
}

// One entry. Returns false after logging why it was skipped.
bool parse_entry(const nlohmann::json& it, const std::string& origin, StatusDef* d) {
    const auto bad = [&](const std::string& why) {
        Loader::get().log_err("[status] " + origin + ": skipping entry: " + why);
        return false;
    };
    if (!it.is_object()) return bad("not an object");
    for (const char* f : {"key", "name", "id", "value", "combine", "reapply", "max_stacks"}) {
        if (!it.contains(f)) return bad(std::string("missing '") + f + "'");
    }
    if (!it["key"].is_string() || !it["name"].is_string() || !it["value"].is_string()) {
        return bad("key, name and value must be strings");
    }
    d->key = it["key"].get<std::string>();
    d->name = it["name"].get<std::string>();
    if (d->name.empty() || d->name.size() > kNameMax) return bad("name empty or too long");
    if (d->name.find('\0') != std::string::npos) return bad("name has a NUL byte");

    std::uint64_t id = 0, combine = 0, reapply = 0, stacks = 0;
    if (!parse_uint(it["id"], 0xFFFFFFFFull, &id)) return bad("'" + d->key + "' id is not a u32");
    if (!parse_uint(it["combine"], 6, &combine)) return bad("'" + d->key + "' combine not 0..6");
    if (!parse_uint(it["reapply"], 4, &reapply)) return bad("'" + d->key + "' reapply not 0..4");
    if (!parse_uint(it["max_stacks"], 0x7FFFFFFFull, &stacks)) {
        return bad("'" + d->key + "' max_stacks not an int >= 0");
    }
    const std::string v = it["value"].get<std::string>();
    if (v == "float") d->kind = ValueKind::Float;
    else if (v == "int") d->kind = ValueKind::Int;
    else if (v == "bool") d->kind = ValueKind::Bool;
    else return bad("'" + d->key + "' value must be float, int or bool");

    const std::uint32_t want = status_id(d->key);
    if (want != static_cast<std::uint32_t>(id)) {
        return bad("'" + d->key + "' id " + hex32(static_cast<std::uint32_t>(id))
                   + " does not match the key's hash " + hex32(want));
    }
    d->id = static_cast<std::uint32_t>(id);
    d->combine = static_cast<std::uint32_t>(combine);
    d->reapply = static_cast<std::uint32_t>(reapply);
    d->max_stacks = static_cast<std::uint32_t>(stacks);
    return true;
}

void load_one(const std::filesystem::path& path, const std::string& mod_id) {
    const std::string origin = mod_id + "/statuses.json";
    nlohmann::json j;
    try {
        std::ifstream f(path);
        f >> j;
    } catch (const std::exception& e) {
        Loader::get().log_err("[status] " + origin + " parse error: " + e.what());
        return;
    }
    if (!j.is_array()) {
        Loader::get().log_err("[status] " + origin + " must be a JSON array");
        return;
    }
    for (const auto& it : j) {
        StatusDef d;
        d.mod = mod_id;
        try {
            if (!parse_entry(it, origin, &d)) continue;
        } catch (const std::exception& e) {
            Loader::get().log_err("[status] " + origin + ": skipping entry: " + e.what());
            continue;
        }
        bool dup = false;
        for (const auto& e : g_defs) {
            if (e.id == d.id) {
                Loader::get().log_err("[status] " + origin + ": '" + d.key + "' id "
                                      + hex32(d.id) + " already declared by '" + e.key
                                      + "'; skipping");
                dup = true;
                break;
            }
        }
        if (dup) continue;
        d.name_c = dup_cstr(d.name);
        if (!d.name_c) {
            Loader::get().log_err("[status] out of memory copying '" + d.name + "'; skipping");
            continue;
        }
        g_defs.push_back(std::move(d));
    }
}

bool load_status_defs() {
    std::error_code ec;
    for (const auto& m : Loader::get().mods()) {
        if (!m.enabled) continue;
        const auto p = m.root / "statuses.json";
        if (std::filesystem::exists(p, ec)) load_one(p, m.id);
    }
    if (g_defs.empty()) {
        Loader::get().log("[status] no statuses.json declares a status; disabled");
        return false;
    }
    Loader::get().log("[status] loaded " + std::to_string(g_defs.size()) + " status def(s)");
    return true;
}

// --- registration (game thread only) -------------------------------------

// The manager: the oCModifierApplicationSettingsSection in AppSettings_
// SectionList. The walk is the engine's own (StatusValueDefs_Register and
// StatusSettingsGo_Init both do it): section->vt[0] gives a type object, whose
// vt[+0x58] answers "is this the class". We run inside those two functions, on
// the same thread, over the engine's own list, so the calls are the ones it is
// about to make itself; every pointer is still page-checked before it is used.
void* find_manager() {
    const std::uintptr_t list = reloc(Sym::AppSettings_SectionList);
    std::uintptr_t data = 0;
    std::uint32_t count = 0;
    if (!mem_load(list, &data) || !mem_load(list + 8, &count) || data == 0 || count == 0
        || count > kSectionMax) {
        return nullptr;
    }
    std::uintptr_t cls = 0;
    if (!mem_load(reloc(Sym::ModifierAppSettingsSection_Class), &cls) || cls == 0) return nullptr;

    for (std::uint32_t i = 0; i < count; ++i) {
        std::uintptr_t sec = 0, vt = 0, fn = 0;
        if (!mem_load(data + i * sizeof(void*), &sec) || sec == 0) continue;
        if (!mem_load(sec, &vt) || vt == 0 || !mem_load(vt + kVtType, &fn) || fn == 0) continue;
        if (!mem_readable(reinterpret_cast<void*>(fn), 1)) continue;
        void* type = reinterpret_cast<SectionType_t>(fn)(reinterpret_cast<void*>(sec));
        std::uintptr_t tvt = 0, kfn = 0;
        if (!type || !mem_load(type, &tvt) || tvt == 0 || !mem_load(tvt + kVtIsKind, &kfn)
            || kfn == 0 || !mem_readable(reinterpret_cast<void*>(kfn), 1)) {
            continue;
        }
        if (reinterpret_cast<IsKindOf_t>(kfn)(type, reinterpret_cast<void*>(cls)) != 0) {
            return reinterpret_cast<void*>(sec);
        }
    }
    return nullptr;
}

// True only when the manager's definition vector was read in full and none of
// its entries carries `id`. Every definition, status or not, is in that vector
// (CreateDesc appends before it inserts into the hash), so it answers the same
// question as the hash at +0x30 without needing the hash's layout. Anything
// unreadable counts as "present": registering over a doubt is the worse error.
bool id_absent(void* mgr, std::uint32_t id) {
    const auto base = reinterpret_cast<std::uintptr_t>(mgr);
    std::uintptr_t vec = 0;
    std::uint32_t n = 0;
    if (!mem_load(base + kMgrVecData, &vec) || !mem_load(base + kMgrVecCount, &n)) return false;
    if (n > kMgrVecMax) return false;
    for (std::uint32_t i = 0; i < n; ++i) {
        std::uintptr_t def = 0;
        std::uint32_t did = 0;
        if (!mem_load(vec + i * sizeof(void*), &def) || def == 0
            || !mem_load(def + kDefId, &did)) {
            return false;
        }
        if (did == id) return false;
    }
    return true;
}

// Build the default value the registrar builds (d5.c, FUN_1401d2a90):
// (each: inline ctor, Destruct, then InitAsType; the ctor symbol stands in for the
// inline one)
//   int   local_328: ctor, InitAsType(0), 4 bytes of 0 at the storage.
//   float local_308: the same, then byte +0x19 = 2 (CONCAT11(2, ...)).
//   bool  local_2c8: InitAsType(2), 1 byte of 0 at the storage.
// Storage is the inline slot at +0x10 when +0x08 == 4, else the heap pointer
// with its low bit masked. Returns false (and the caller destructs) when the
// engine did not leave the union in the expected shape.
bool build_default(std::uint8_t* u, ValueKind kind) {
    const std::uint32_t type = kind == ValueKind::Bool ? kTypeBool : kTypeNumeric;
    g_union_ctor(u);
    g_union_dtor(u);
    g_union_init(u, type);
    std::uint64_t storage = 0;
    std::uint8_t tag = 0xFF;
    if (!mem_load(u + kUnionStorage, &storage) || !mem_load(u + kUnionType, &tag)
        || tag != type) {
        return false;
    }
    std::uint8_t* data = storage == 4 ? u + kUnionInline
                                      : reinterpret_cast<std::uint8_t*>(storage & ~1ull);
    const std::size_t width = kind == ValueKind::Bool ? 1 : 4;
    if (!mem_writable(data, width)) return false;
    std::memset(data, 0, width);
    if (kind == ValueKind::Float) u[kUnionFloatFlag] = 2;
    return true;
}

bool register_one(void* mgr, const StatusDef& d, const char* via) {
    if (!id_absent(mgr, d.id)) {
        Loader::get().log_err("[status] '" + d.key + "' id " + hex32(d.id)
                              + " collides with an existing definition (or the manager "
                                "could not be read); skipping");
        return false;
    }
    alignas(16) std::uint8_t u[kUnionSize];
    std::memset(u, 0, sizeof(u));
    const bool built = build_default(u, d.kind);
    void* def = nullptr;
    if (built) {
        g_done = true;  // from here on a retry could register an id twice
        StringDesc name{d.name_c, kLiteralBit | static_cast<std::uint32_t>(d.name.size()), 0};
        def = g_create(mgr, d.id, &name, reinterpret_cast<void*>(reloc(Sym::ValueCategory_Status)),
                       u, reinterpret_cast<void*>(reloc(Sym::EngineString_Empty)));
    }
    g_union_dtor(u);  // CreateDesc copied the value; free the scratch's payload
    if (!built) {
        Loader::get().log_err("[status] '" + d.key + "': the default value did not "
                              "initialise as expected; skipping");
        return false;
    }

    std::uint32_t got_id = 0;
    std::uint8_t inserted = 0;
    if (!def || !mem_writable(def, kDefSize) || !mem_load(reinterpret_cast<std::uintptr_t>(def) + kDefId, &got_id)
        || got_id != d.id
        || !mem_load(reinterpret_cast<std::uintptr_t>(def) + kDefInserted, &inserted)
        || inserted != 1) {
        Loader::get().log_err("[status] '" + d.key + "': CreateDesc returned no usable "
                              "definition; skipping");
        return false;
    }
    const auto dp = reinterpret_cast<std::uintptr_t>(def);
    if (!mem_copy_to(dp + kDefReapply, &d.reapply, sizeof(d.reapply))
        || !mem_copy_to(dp + kDefCombine, &d.combine, sizeof(d.combine))
        || !mem_copy_to(dp + kDefMaxStacks, &d.max_stacks, sizeof(d.max_stacks))) {
        Loader::get().log_err("[status] '" + d.key + "': could not write the definition's "
                              "reapply/combine/max_stacks fields");
        return false;
    }
    Loader::get().log("[status] registered '" + d.key + "' id=" + hex32(d.id) + " via " + via);
    return true;
}

// Both detours land here. Registers once per process, on whichever game-thread
// path first finds the manager.
void register_all(const char* via) {
    std::lock_guard<std::mutex> lock(g_register_mu);
    if (g_done) return;
    void* mgr = nullptr;
    if (Loader::get().va_globals_trusted()) mgr = find_manager();
    if (!mgr || !mem_writable(mgr, kMgrMinSize)) {
        if (!g_logged_no_mgr) {
            g_logged_no_mgr = true;
            Loader::get().log_err("[status] no usable oCModifierApplicationSettingsSection in "
                                  "AppSettings_SectionList (via " + std::string(via)
                                  + "); will retry on the other path");
        }
        return;
    }
    int ok = 0;
    for (const auto& d : g_defs) {
        if (register_one(mgr, d, via)) ++ok;
    }
    // With the manager found, a retry would meet the same definitions and skip
    // the same statuses, so this pass is final. Only a missing manager retries.
    g_done = true;
    Loader::get().log("[status] registration via " + std::string(via) + ": "
                      + std::to_string(ok) + "/" + std::to_string(g_defs.size())
                      + " status(es)");
}

// --- detours -------------------------------------------------------------

void hook_register() {
    g_real_register();  // the 20 vanilla definitions first
    hook_note_fire(g_register_va);
    register_all("StatusValueDefs_Register");
}

void hook_status_init(void* self) {
    hook_note_fire(g_init_va);
    register_all("StatusSettingsGo_Init");  // before the prune reads the manager
    g_real_init(self);
}

} // namespace

bool install_status_hooks() {
    if (!load_status_defs()) return false;
    if (!Loader::get().va_globals_trusted()) {
        Loader::get().log_err("[status] disabled: game build != symbol-map build, so the "
                              "manager and category globals are not trusted");
        return false;
    }
    if (!resolve_checked("status", "create desc", Sym::ModifierSettings_CreateDesc_Pattern, &g_create)
        || !resolve_checked("status", "union ctor", Sym::EntityValueUnion_DefaultCtor_Pattern,
                            &g_union_ctor)
        || !resolve_checked("status", "union init", Sym::EntityValueUnion_InitAsType_Pattern,
                            &g_union_init)
        || !resolve_checked("status", "union dtor", Sym::EntityValueUnion_Destruct_Pattern,
                            &g_union_dtor)) {
        Loader::get().log_err("[status] disabled: helper routines do not resolve to "
                              "function starts on this build");
        return false;
    }
    const bool a = hook_install("status", "status value defs", Sym::StatusValueDefs_Register_Pattern,
                                reinterpret_cast<void*>(&hook_register),
                                reinterpret_cast<void**>(&g_real_register), &g_register_va);
    const bool b = hook_install("status", "status table init", Sym::StatusSettingsGo_Init_Pattern,
                                reinterpret_cast<void*>(&hook_status_init),
                                reinterpret_cast<void**>(&g_real_init), &g_init_va);
    if (!a && !b) {
        Loader::get().log_err("[status] disabled: neither detour installed");
        return false;
    }
    if (!a || !b) {
        Loader::get().log_warn(std::string("[status] only the ")
                               + (a ? "StatusValueDefs_Register" : "StatusSettingsGo_Init")
                               + " detour is live; registration still happens, but only on that path");
    }
    Loader::get().log("[status] " + std::to_string(g_defs.size()) + " status(es) queued");
    return true;
}

} // namespace rsmm
